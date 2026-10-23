# Arquitectura de Reelix

## Visión general

```
┌───────────────────────────────┐
│ GTK4 / libadwaita  (ui/)      │  hilo principal (GLib main loop)
└──────────────┬────────────────┘
               │ acciones / ControllerObserver
┌──────────────▼────────────────┐
│ Controller   (app/controller) │  coordina; nunca bloquea
│ RecorderState (app/state)     │  única fuente de verdad
└──────┬───────────────┬────────┘
       │               │
┌──────▼──────┐  ┌─────▼─────────────────────┐   ┌──────────────┐
│ capture/    │  │ recording/pipeline        │──▶│ storage/     │
│ portal.rs   │  │ GStreamer (hilos propios) │   │ mp4 + json   │
│ (ashpd/D-Bus)│ └─────▲─────────────────────┘   └──────────────┘
└──────┬──────┘        │ fd + node id
       ▼               │
 xdg-desktop-portal ──▶ PipeWire ──▶ pipewiresrc
 (diálogo de GNOME)
```

## Módulos

| Módulo | Responsabilidad | Depende de GTK/GStreamer |
|---|---|---|
| `branding` | Nombre, APP_ID, directorios. Único sitio a cambiar para renombrar. | No |
| `error` | `AppError` (thiserror): mensaje técnico para logs + `user_message()` para diálogos. | No |
| `config` | `Settings` en TOML, valores por defecto, saneado. | No |
| `app::state` | Máquina de estados pura `(estado, evento) → Result<estado>`. | No |
| `app::controller` | Orquesta portal, pipeline, almacenamiento y estado. | GLib |
| `capture::portal` | Sesión ScreenCast con `ashpd`. | ashpd |
| `capture::camera` | Acceso a la cámara por el portal Camera y enumeración de cámaras. | ashpd, GStreamer |
| `capture::audio` | Enumeración de micrófonos (DeviceMonitor) y traducción de ajustes a entradas de audio. | GStreamer |
| `recording::encoder` | Detección y configuración del encoder H.264. | GStreamer |
| `recording::camera` | Composición de la cámara: geometría de la burbuja, máscara circular, compositor. | GStreamer |
| `recording::audio` | Rama de audio: fuentes, mezcla, limitador, AAC, medidor de nivel. | GStreamer |
| `recording::pipeline` | Construcción y control del pipeline, eventos del bus. | GStreamer |
| `storage` | Nombres de archivo, metadatos JSON, biblioteca (listar/renombrar/borrar). | No |
| `ui` | Ventana, acciones, atajos. | GTK |

El núcleo sin dependencias gráficas se prueba con tests unitarios; `tests/`
contiene tests de integración (biblioteca en disco y codificación real con
`videotestsrc`).

## Máquina de estados

```
Idle → SelectingSource → Preparing → Recording ⇄ Paused
                                        ↓
                                     Stopping → Processing → Completed → (Reset) Idle
Cualquier estado activo --Error--> Failed      --Cancel--> Idle
```

Las transiciones inválidas devuelven `AppError::InvalidTransition` y se
registran en el log; nunca se corrompe el estado en silencio. Las tareas
asíncronas (diálogo del portal, timeout de finalización) llevan un número de
"generación": si el usuario cancela mientras esperan, su resultado se
descarta.

## Pipeline de vídeo

```
pipewiresrc fd=<portal> path=<node> do-timestamp keepalive-time=1000
  → videoconvert   BGRx/BGRA → I420 (NV12 con encoder VA), n-threads≤4, sin dithering
  → videoscale + capsfilter   tamaño fijado al primer frame (pares)
  → videorate max-rate=fps     solo descarta, nunca duplica
  → queue leaky=downstream max-size-buffers=5
  → vah264lpenc | vah264enc | x264enc | openh264enc
  → h264parse → mp4mux (índice reservado, actualizado cada 10 s) → filesink
```

### Decisiones

**GStreamer en proceso, no `ffmpeg` externo.** `pipewiresrc` consume
directamente el fd del portal (sin copiar frames entre procesos); el control
(EOS, errores, renegociación) es nativo; y en las fases 2–3 `audiomixer` y
`compositor` resuelven audio y cámara en el mismo grafo.

**`ashpd` para los portales.** Es la librería Rust mantenida para
xdg-desktop-portal (zbus), con integración GTK4/Wayland para que el diálogo
quede asociado a nuestra ventana. Se habilitó solo `gtk4_wayland` (no X11).

**MP4/H.264, no WebM.** En Fedora `openh264enc` (Cisco, preinstalado) y
`mp4mux` funcionan sin repositorios extra. MP4 es lo más compatible para
compartir. Si en el futuro se necesitara un formato libre de patentes, el
diseño permite añadir VP9/AV1 + WebM cambiando encoder y muxer.

**Encoder elegido en tiempo de ejecución.** Orden: `vah264lpenc` (bloque fijo
de la GPU Intel, menor consumo) → `vah264enc` → `x264enc` → `openh264enc`.
Fedora no trae H.264 por VA-API (driver *free*), así que en una instalación
estándar se usa openh264. Instalar `intel-media-driver` de RPM Fusion activa
el hardware sin recompilar.

**Salida VFR.** GNOME solo envía frames cuando la pantalla cambia (más un
keep-alive de 1 s). `videorate max-rate` limita a `fps` sin duplicar frames,
así una pantalla estática casi no consume CPU. Medido: 16,6 fps de media en
una grabación de escritorio con límite de 30.

**Tamaño fijo.** Una sonda en el pad de `pipewiresrc` lee las primeras caps,
redondea a dimensiones pares (H.264 4:2:0 lo exige) y fija el capsfilter. Si
la ventana capturada cambia de tamaño, `videoscale` la encaja con bordes en
lugar de cambiar las caps del MP4 a mitad de archivo. El bitrate se calcula
con ese tamaño real.

**Backpressure.** La `queue` antes del encoder está acotada y descarta frames
antiguos si el encoder no da abasto: la RAM nunca crece sin límite y la
latencia no se acumula.

**Parada robusta.** El EOS se inyecta en la `queue`, no en la fuente, de modo
que la finalización funciona aunque PipeWire ya haya cerrado el stream (por
ejemplo, si se detuvo desde el indicador de GNOME). Si el EOS no llega en
10 s se fuerza el cierre; el índice reservado de `mp4mux` mantiene el archivo
reproducible.

## Pipeline de audio (Fase 2)

```
pulsesrc [device=<node.name> | @DEFAULT_MONITOR@] provide-clock=false
  → audioconvert → audioresample → 48 kHz estéreo → volume [→ level (VU)]
  ─┬─ (una fuente) ──────────────────────────────────────────────┐
   └─ (varias) → audiomixer → capsfilter 48 kHz estéreo ──────────┤
                                     audiodynamic (limitador) ←───┘
                                       → queue (1 s, leaky) → fdkaacenc | avenc_aac | voaacenc → mp4mux
```

### Decisiones

**`pulsesrc` (capa PulseAudio de PipeWire) en lugar de `pipewiresrc`.** Es
la ruta más madura de GStreamer para audio (esclavizado de reloj, latencia),
y es como Flatpak da acceso al micrófono (`--socket=pulseaudio`): PipeWire
no tiene todavía un portal de micrófono. Los dispositivos se enumeran con
`gst::DeviceMonitor`, que en Fedora usa el proveedor de PipeWire, y se
identifican por su `node.name`, que también es el nombre de la fuente
PulseAudio.

**Audio del sistema = monitor del sink.** En Linux no existe un "loopback"
como en Windows: PipeWire expone la salida de cada sink como fuente
capturable. `@DEFAULT_MONITOR@` sigue al dispositivo de salida por defecto.

**Un único reloj.** El pipeline usa `gst::SystemClock` y las fuentes de
audio no proporcionan reloj (`provide-clock=false`), sino que se esclavizan
a él. Vídeo y audio comparten la misma línea de tiempo. Medido en grabaciones
reales: los inicios de pista difieren 5–29 ms y las duraciones 30–90 ms.

**Mezcla.** `audiomixer` negocia su salida con el encoder (que preferiría
mono) y convierte internamente; un capsfilter fija la salida a estéreo
48 kHz. Al mezclar con el micrófono, el audio del sistema se atenúa al 60 %
(≈ −4,4 dB).

**Limitador.** En grabaciones reales el 3–4 % de los bloques superaba 0 dBFS
(voz fuerte, o voz + música). `audiodynamic` en modo compresor *hard-knee*
(umbral 0,6, ratio 0,15) baja el pico de una mezcla real de +6,0 a −1,8 dBFS
reduciendo el RMS solo 0,4 dB. Se descartó *soft-knee*: la curva de
`audiodynamic` no comprime muestras que ya superan 1,0 (pico +1,6 dBFS en la
misma prueba).

**Degradación elegante.** Sin micrófono, con el micrófono configurado
desconectado o sin encoder AAC, se graba igualmente (sin ese audio, o con el
micrófono predeterminado) y se avisa con un toast. Si una fuente de audio
falla a mitad de la grabación se finaliza el archivo con lo grabado.

**Parada.** El EOS se inyecta en la cola de cada rama (vídeo y audio);
`mp4mux` finaliza cuando todas lo han recibido.

**Pruebas del pipeline real.** `PipelineConfig` acepta fuentes inyectables
(`VideoInput::Element`, `AudioSourceElement::Element`), lo que permite que
`tests/recording_pipeline.rs` ejecute el `RecordingPipeline` de producción con
`videotestsrc`/`audiotestsrc` en vivo y verifique pistas, duraciones,
estéreo, medidor de nivel y ausencia de saturación.

## Cámara (Fase 3)

```
pantalla → videoconvert → queue ─────────────────────────────────────┐ sink_0
                                                                      ├→ compositor (fps fijo) → videoconvert → … encoder
portal Camera → pipewiresrc → raw → aspectratiocrop (4:3 | 1:1)       │
   → videoflip → videoscale → videoconvert → 640×480 | 480×480 ──────┘ sink_1 (xpos/ypos/width/height)
                                 (círculo: AYUV + máscara alfa)
```

### Decisiones

**Portal Camera + `pipewiresrc`.** El portal entrega un remoto de PipeWire
que solo expone cámaras; funciona igual en el host y en Flatpak. Las cámaras
se identifican por `node.name` (estable entre reinicios, a diferencia del
número de serie).

**Composición en GStreamer (`compositor`).** La burbuja se calcula
(`CameraLayout::bubble_rect`: tamaño relativo a la altura, margen del 3 %,
dimensiones pares, siempre dentro del cuadro) cuando se conoce el tamaño de
la pantalla, y se aplica a las propiedades del pad del compositor, que escala
la cámara.

**Forma y coste medido.** El compositor rechaza entradas con alfa si su
salida es I420, así que el círculo obliga a componer en AYUV:

| | gst-launch 1080p30 (sobre 70 % sin cámara) | App real, sesión completa |
|---|---|---|
| Rectángulo (I420) | 78 % (+8 %) | ≈ 100 % de un núcleo |
| Círculo (AYUV + máscara) | 98 % (+28 %) | ≈ 145 % de un núcleo |

Por eso el rectángulo es el valor por defecto. La rama de la cámara en sí
cuesta un 3 %: el formato YUY2 640×480 raw evita decodificar MJPEG. La
máscara circular (antialiasing de 1 px) se precalcula una vez y se escribe en
el canal alfa de cada frame (480×480) desde una sonda.

**Negociación aislada.** `pipewiresrc` convierte las caps que acepta su peer
en una petición de formato PipeWire, y esa conversión falla en cuanto se
acumulan restricciones (framerate del compositor + listas de tamaños/formatos
del recorte y escalado: `error during negotiation`). Una sonda responde a su
consulta de caps solo con formatos raw comunes; la cámara elige su modo
nativo y el resto de la cadena se adapta. Detectado en la prueba real: con
`videotestsrc` no ocurre, de ahí el test `real_camera_composition`
(`#[ignore]`, se ejecuta con `cargo test -- --ignored real_camera`).

**Degradación elegante.** Sin cámara, con acceso denegado o con la cámara
elegida desconectada, se graba sin cámara (o con la predeterminada) y se
avisa.

**Composición solo cuando hace falta.** Con la burbuja en pantalla (Fase 4),
una grabación de pantalla completa ya la captura, así que no se compone (y se
ahorra el coste del compositor). Se compone al grabar una sola ventana, que
no incluiría la burbuja.

## Burbuja de cámara (Fase 4)

```
portal Camera → pipewiresrc → raw → aspectratiocrop → [videoflip] → videoscale
  → BGRA 2× + máscara alfa (círculo | rectángulo redondeado) → queue → gtk4paintablesink → gtk::Picture
```

Ventana GTK sin decoración, arrastrable (`gdk::Toplevel::begin_move`), con un
borde "camaleón" animado por CSS (`@keyframes` sobre `border-color`, lo
dibuja GTK). Al pasar el ratón durante la grabación muestra ● REC, el tiempo
y un botón Detener. La ventana principal se minimiza 600 ms después de
empezar (antes, GNOME la restauraba al cerrar el diálogo modal del portal).

Lecciones de la prueba real, cada una con su test:

- **GTK no recorta el vídeo al borde redondeado** (`overflow: hidden` no
  aplica el radio al contenido), así que la forma se recorta en los propios
  frames con la misma máscara alfa del compositor. En BGRA el color se
  premultiplica: los píxeles transparentes quedan a cero sea cual sea la
  interpretación del consumidor.
- **`gtk4paintablesink` rellena las filas** (stride > ancho × 4): la máscara
  se aplica fila a fila según `VideoMeta`, o deduciendo el stride del tamaño
  del buffer.
- **`pipewiresrc` anuncia caps ANY** y `aspectratiocrop` no enlaza desde ahí:
  un capsfilter `video/x-raw` va primero (`build_preview_chain`, cubierto con
  `videotestsrc` y con la cámara real).
- **Temas de usuario.** `~/.config/gtk-4.0/gtk.css` se aplica con prioridad
  USER, por encima de la de la app; un tema con `window { background-color }`
  translúcido pintaba la burbuja. Su CSS se carga con prioridad USER + 1 (solo
  afecta a la burbuja). Diagnosticado renderizando la ventana fuera de
  pantalla y leyendo el alfa de las esquinas.
- **Extensiones de GNOME Shell** (Blur my Shell) dibujan un rectángulo
  difuminado detrás de cada ventana; una app no puede evitarlo: se documenta
  cómo excluir Reelix.
- **Wayland**: la app no puede poner la burbuja "siempre encima", en todos
  los escritorios ni en una posición concreta; el usuario lo hace desde el
  menú de la ventana (Alt+Espacio). Automatizarlo requeriría una extensión de
  GNOME Shell.

## UX (Fase 4)

**Pausa.** `PAUSED` en un pipeline en vivo detiene las fuentes y el tiempo de
ejecución; al volver a `PLAYING` continúa donde se quedó, así que el MP4 no
tiene huecos y audio/vídeo siguen alineados (test con dos pausas: archivo de
2,0 s para 2,0 s grabados). En `PAUSED` el sink espera datos y retendría el
EOS: detener estando en pausa reanuda primero.

**Cuenta atrás.** El pipeline se construye antes (los errores salen antes de
contar) pero solo arranca al llegar a cero. El número se oculta y la captura
espera 150 ms para que GNOME repinte la burbuja (si no, el "1" aparecía en
los primeros frames). Ocurre dentro del estado `Preparing`; Esc lo cancela.

**Biblioteca.** Filas compartidas por la lista de recientes y el diálogo
*Mis grabaciones*. "Eliminar" mueve a la papelera (`gio::File::trash`,
recuperable); renombrar cambia el título en el JSON y conserva el nombre del
archivo. "Copiar" pone un `GdkFileList` en el portapapeles.

**Preferencias.** `AdwPreferencesDialog`; cada cambio se guarda al momento en
`settings.toml` y aplica a la siguiente grabación.

**Atajos globales.** Portal `GlobalShortcuts`: la app propone
`CTRL+SHIFT+r/p` y GNOME pide confirmación (opcional, desde Preferencias,
para no mostrar un diálogo al arrancar). Las apps no-Flatpak se identifican
antes con el portal `org.freedesktop.host.portal.Registry`. Mientras están
activos se quitan los aceleradores internos equivalentes para no disparar
dos veces.

## Optimización (Fase 5)

Se midió antes de optimizar, con dos herramientas:

- **Estadísticas del pipeline** (`recording::stats`): frames en la cola del
  encoder, codificados y descartados, y latencia de codificación por frame
  (emparejados por timestamp; el desplazamiento constante que aplican los
  encoders VA-API se aprende del primer frame). Se registran al guardar y se
  avisa si se descarta más del 2 %.
- **Benchmark** (`examples/bench.rs`): el `RecordingPipeline` real con
  fuentes sintéticas 1080p30 a frames constantes (peor caso). Generar la
  escena de prueba cuesta por sí solo ~42 % de un núcleo; se resta al
  interpretar las cifras.

### Hallazgos y cambios

1. **El "bitrate excesivo" era relleno del contenedor.** El índice reservado
   de `mp4mux` (para sobrevivir a cierres inesperados) añadía 7,9 MB de
   `free` por pista a cada archivo: el 83 % de una grabación de 17 s. El
   encoder producía 1,5 Mbit/s, por debajo del objetivo. Ahora se graba **MP4
   fragmentado** (cada segundo autocontenido, sin relleno) y en *Processing*
   se **remuxa** a MP4 normal con el índice delante (`qtdemux → h264parse /
   aacparse → mp4mux faststart`, sin recodificar; los parsers son
   necesarios: enlazar demuxer y muxer directamente se bloquea). 36 s de
   grabación: remux en 48 ms. Si el remux falla se conserva el fragmentado,
   que es válido.
2. **GPU Intel (con `intel-media-driver` de RPM Fusion nonfree).** El encoder
   Low Power de Tiger Lake solo admite *rate control* CQP: se usa VBR donde
   exista y si no CQP con un QP derivado de la calidad (Baja 30, Media 26,
   Alta 22). Sin B-frames (latencia y emparejamiento de estadísticas).
   Las propiedades del encoder se fijan con setters que no hacen pánico
   (`recording::props`): `set_property_from_str` con un valor no admitido
   abortaba la app al empezar a grabar.
3. **Conversión de color en la GPU.** Con encoder VA y `vapostproc`
   disponibles, la conversión BGRx→NV12 y el escalado pasan a la GPU y el
   encoder recibe superficies `memory:VAMemory`. Con cámara compuesta, ambas
   formas componen en BGRA (I420 obligaría a convertir la pantalla en CPU;
   `vapostproc` no acepta AYUV).

### Resultados

Benchmark, 1080p30 a frames constantes (incluye el 42 % de generar la escena):

| | openh264 (CPU) | vah264lpenc + vapostproc (GPU) |
|---|---|---|
| Pantalla | 104 % | 50 % |
| + cámara rectángulo | 140 % | 88 % |
| + cámara círculo | 131 % | 80 % |
| Latencia de codificación media | 13–16 ms | 4–6 ms |
| RAM máxima (sin GTK) | 79–101 MB | 57–88 MB |

App real (release, PipeWire, burbuja + micrófono, 36 s): CPU 13 % en reposo
con la burbuja visible y **16 % grabando** (pico 25 %); RAM máxima 177 MB;
0 frames descartados; archivo de 7,3 MB. Antes de la Fase 5 (build debug y
software): ~90 % y 232 MB.

Lo que queda del coste es sobre todo la **burbuja** (vista previa a 30 fps,
conversión a BGRA y máscara en CPU, animación del borde). Posible mejora:
convertir en la GPU y entregar DMA-BUF a `gtk4paintablesink`, aplicando la
forma con un recorte de GTK en vez de la máscara.

## Investigación de plataforma (Fedora 44, GNOME 50, Intel Tiger Lake)

| Tema | Conclusión |
|---|---|
| ScreenCast portal | v5 en GNOME; pantalla y ventana; cursor embebido/oculto/metadata. Restore tokens disponibles (Fase 4). |
| PipeWire | 1.6; `pipewiresrc` negocia BGRA/BGRx en memoria compartida; DMA-BUF posible con elementos que lo acepten (VA). |
| GStreamer | 1.28; plugin `va` (nuevo, sustituye a `gstreamer-vaapi`, que está obsoleto). |
| VA-API Intel | Driver iHD. La versión de Fedora solo expone JPEG encode; H.264/HEVC encode requieren `intel-media-driver` de RPM Fusion **nonfree** (no free). En Tiger Lake el encoder LP solo ofrece CQP. |
| Cámara | Implementado (Fase 3): portal `Camera` + `pipewiresrc`; ver arriba. |
| Audio | Implementado (Fase 2) con `pulsesrc` sobre pipewire-pulse; ver arriba. `webrtcdsp` está disponible para supresión de ruido/AGC (pendiente). |
| Ventana siempre encima | **Wayland no permite que un cliente se ponga "always on top".** Opciones: el usuario lo activa desde el menú de la ventana (Alt+Espacio), una extensión de GNOME Shell, o dibujar la burbuja de cámara dentro del vídeo (composición). Decisión para la Fase 4. |
| Atajos globales | Implementado (Fase 4) con el portal `GlobalShortcuts` v1 (GNOME 48+) + `Registry` para apps del host. |
| Flatpak | Necesitará `--socket=wayland`, `--socket=pulseaudio` (micrófono y monitor) y `--filesystem=xdg-videos/Reelix:create`; la pantalla llega por el portal sin permisos extra. Pendiente. |

## Rendimiento medido

### Fase 4 (burbuja en pantalla, sin compositor)

Grabación de pantalla completa con la burbuja visible: ≈ 90 % de un núcleo
de media (frente a ≈ 100 % con la cámara compuesta en rectángulo y ≈ 145 % en
círculo).

### Fase 3 (pantalla + micrófono + cámara)

Build debug, 1920×1080, `openh264enc`, muestreo de CPU por segundo durante
la grabación: rectángulo ≈ 100 %, círculo ≈ 145 % de un núcleo; RSS máximo
232 MB. Con cámara la salida pasa a 30 fps constantes (el compositor genera
frames aunque la pantalla no cambie).

### Fase 2 (pantalla + micrófono + audio del sistema)

Sesión de 4:50 con tres grabaciones (18 s, 45 s, 25 s) de pantalla completa
1920×1080, `openh264enc` + `fdkaacenc`, build debug: 40 s de CPU de usuario
en total y RSS máximo de ≈ 212 MB (+12 MB respecto a solo vídeo).

### Fase 1

Grabación de 17,5 s de pantalla completa 1920×1080, `openh264enc`, build debug:

| Métrica | Valor |
|---|---|
| CPU total del proceso | 8,6 s en toda la sesión (≈ 0,5 núcleos durante la grabación) |
| RSS máximo | ≈ 200 MB (incluye GTK/libadwaita) |
| Frames | 290 (16,6 fps de media, VFR) |
| Bitrate real | 8,8 Mbit/s (objetivo 5 Mbit/s, ver problemas conocidos) |
| Finalización tras Detener | ≈ 20 ms |
