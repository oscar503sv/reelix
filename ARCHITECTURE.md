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
| `capture::audio` | Enumeración de micrófonos (DeviceMonitor) y traducción de ajustes a entradas de audio. | GStreamer |
| `recording::encoder` | Detección y configuración del encoder H.264. | GStreamer |
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

## Investigación de plataforma (Fedora 44, GNOME 50, Intel Tiger Lake)

| Tema | Conclusión |
|---|---|
| ScreenCast portal | v5 en GNOME; pantalla y ventana; cursor embebido/oculto/metadata. Restore tokens disponibles (Fase 4). |
| PipeWire | 1.6; `pipewiresrc` negocia BGRA/BGRx en memoria compartida; DMA-BUF posible con elementos que lo acepten (VA). |
| GStreamer | 1.28; plugin `va` (nuevo, sustituye a `gstreamer-vaapi`, que está obsoleto). |
| VA-API Intel | Driver iHD 26.x. La versión de Fedora solo expone JPEG encode; H.264/HEVC/VP9/AV1 encode requieren RPM Fusion. |
| Cámara | Portal `Camera` + `pipewiresrc` (funciona en Flatpak). Fase 3. |
| Audio | Implementado (Fase 2) con `pulsesrc` sobre pipewire-pulse; ver arriba. `webrtcdsp` está disponible para supresión de ruido/AGC (pendiente). |
| Ventana siempre encima | **Wayland no permite que un cliente se ponga "always on top".** Opciones: el usuario lo activa desde el menú de la ventana (Alt+Espacio), una extensión de GNOME Shell, o dibujar la burbuja de cámara dentro del vídeo (composición). Decisión para la Fase 4. |
| Atajos globales | Portal `GlobalShortcuts` (implementado en GNOME 48+). Requiere confirmación del usuario. Fase 4. Por ahora los atajos solo funcionan con la ventana enfocada. |
| Flatpak | Necesitará `--socket=wayland`, `--socket=pulseaudio` (micrófono y monitor) y `--filesystem=xdg-videos/Reelix:create`; la pantalla llega por el portal sin permisos extra. Pendiente. |

## Rendimiento medido

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
