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
| `recording::encoder` | Detección y configuración del encoder H.264. | GStreamer |
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

## Pipeline de vídeo (Fase 1)

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

## Investigación de plataforma (Fedora 44, GNOME 50, Intel Tiger Lake)

| Tema | Conclusión |
|---|---|
| ScreenCast portal | v5 en GNOME; pantalla y ventana; cursor embebido/oculto/metadata. Restore tokens disponibles (Fase 4). |
| PipeWire | 1.6; `pipewiresrc` negocia BGRA/BGRx en memoria compartida; DMA-BUF posible con elementos que lo acepten (VA). |
| GStreamer | 1.28; plugin `va` (nuevo, sustituye a `gstreamer-vaapi`, que está obsoleto). |
| VA-API Intel | Driver iHD 26.x. La versión de Fedora solo expone JPEG encode; H.264/HEVC/VP9/AV1 encode requieren RPM Fusion. |
| Cámara | Portal `Camera` + `pipewiresrc` (funciona en Flatpak). Fase 3. |
| Audio | Micrófono: `pipewiresrc`/`pulsesrc` contra el nodo de entrada. Audio del sistema: capturar el *monitor* del sink por PipeWire (no existe "loopback" como en Windows). Fase 2. |
| Ventana siempre encima | **Wayland no permite que un cliente se ponga "always on top".** Opciones: el usuario lo activa desde el menú de la ventana (Alt+Espacio), una extensión de GNOME Shell, o dibujar la burbuja de cámara dentro del vídeo (composición). Decisión para la Fase 4. |
| Atajos globales | Portal `GlobalShortcuts` (implementado en GNOME 48+). Requiere confirmación del usuario. Fase 4. Por ahora los atajos solo funcionan con la ventana enfocada. |
| Flatpak | Necesita `--socket=wayland`, `--filesystem=xdg-videos/Reelix:create`, `--talk-name` no requerido para portales; PipeWire vía portal. Pendiente. |

## Rendimiento medido (Fase 1)

Grabación de 17,5 s de pantalla completa 1920×1080, `openh264enc`, build debug:

| Métrica | Valor |
|---|---|
| CPU total del proceso | 8,6 s en toda la sesión (≈ 0,5 núcleos durante la grabación) |
| RSS máximo | ≈ 200 MB (incluye GTK/libadwaita) |
| Frames | 290 (16,6 fps de media, VFR) |
| Bitrate real | 8,8 Mbit/s (objetivo 5 Mbit/s, ver problemas conocidos) |
| Finalización tras Detener | ≈ 20 ms |
