# Reelix

Grabador de pantalla **nativo para Linux**, inspirado en Loom, diseñado para
**Fedora + GNOME + Wayland**. Escrito en Rust con GTK4/libadwaita, el portal
ScreenCast de XDG, PipeWire y GStreamer.

- 100 % local: sin cuentas, sin nube, sin telemetría.
- Sin X11: la captura pasa por `xdg-desktop-portal` y PipeWire.
- Ligero: el procesamiento de vídeo lo hace GStreamer (C, multihilo); Rust
  coordina, gestiona estado y UI.

> Estado: **Fase 4 (UX) completada.** Graba pantalla o ventana con
> micrófono, audio del sistema y cámara a MP4 (H.264 + AAC), con burbuja de
> cámara, pausa, cuenta atrás, biblioteca y preferencias. Siguiente: Fase 5
> (optimización). Ver [Hoja de ruta](#hoja-de-ruta).

## Qué hace

- Botón **Grabar pantalla** → selector nativo de GNOME (pantalla o ventana).
- Graba a `~/Vídeos/Reelix/recording-AAAA-MM-DD-HHMMSS.mp4` (H.264 + AAC)
  más un `.json` con metadatos (título, fecha, duración, resolución, fps,
  audio, encoder).
- **Micrófono** (predeterminado o uno concreto) y **audio del sistema** (lo
  que suena en los altavoces), por separado o mezclados, con medidor de nivel
  en vivo. Al mezclar, el audio del sistema se baja al 60 % para que la voz
  se entienda, y un limitador evita la saturación.
- **Burbuja de cámara en pantalla**, como Loom: una ventana flotante con tu
  cámara en vivo (círculo o rectángulo redondeado, tres tamaños, efecto
  espejo, borde "camaleón" que cambia de color) que puedes arrastrar. Al
  grabar la pantalla completa se graba tal cual se ve; al grabar una sola
  ventana, la cámara se compone dentro del vídeo en la esquina elegida.
- Durante la grabación la ventana principal se minimiza y la burbuja muestra
  **● REC + tiempo**, **Pausar/Reanudar** y **Detener** al pasar el ratón.
- **Pausa** sin huecos en el archivo y **cuenta atrás** configurable (no
  aparece en el vídeo; Esc la cancela).
- **Mis grabaciones**: reproducir, renombrar, mostrar en la carpeta, copiar
  el archivo al portapapeles, ver detalles y mover a la papelera.
- **Preferencias**: cuenta atrás, fps, calidad, codificador (muestra el que
  se usa), puntero, volumen del micrófono, carpeta y atajos.
- **Atajos globales** opcionales (portal GlobalShortcuts): funcionan sin que
  Reelix tenga el foco.
- Cronómetro en vivo, botón **Detener**, **Esc** cancela y descarta.
- Detener desde el indicador "compartiendo pantalla" de GNOME también
  finaliza el archivo correctamente.
- Lista de grabaciones recientes; clic para reproducir.
- Evita que la sesión se suspenda mientras graba.
- Si la app se cierra de golpe, el MP4 sigue siendo reproducible hasta los
  últimos ~10 s (índice MP4 reservado y actualizado periódicamente).

## Requisitos

Fedora 40+ con GNOME en sesión Wayland (probado en Fedora 44, GNOME 50.6,
GStreamer 1.28, PipeWire 1.6).

Ejecución (en Fedora Workstation casi todo viene preinstalado):

```bash
sudo dnf install pipewire-gstreamer pipewire-pulseaudio gstreamer1-plugins-good \
    gstreamer1-plugins-bad-free gstreamer1-plugin-openh264 fdk-aac-free \
    xdg-desktop-portal xdg-desktop-portal-gnome
```

Compilación (además de lo anterior):

```bash
sudo dnf install rust cargo gcc gtk4-devel libadwaita-devel \
    gstreamer1-devel gstreamer1-plugins-base-devel
```

### Aceleración por hardware (Intel, opcional)

El `libva-intel-media-driver` de Fedora **no incluye codificación H.264**
(por patentes). Sin él, Reelix usa `openh264enc` (software). Para codificar
con la GPU Intel instala la versión de RPM Fusion:

```bash
sudo dnf install https://mirrors.rpmfusion.org/free/fedora/rpmfusion-free-release-$(rpm -E %fedora).noarch.rpm
sudo dnf swap libva-intel-media-driver intel-media-driver --allowerasing
vainfo | grep -i "H264.*Enc"     # debe listar VAEntrypointEncSliceLP
gst-inspect-1.0 vah264lpenc      # debe existir
```

Reelix detecta el encoder en tiempo de ejecución; no hay que recompilar.

## Instalación y ejecución

```bash
git clone https://github.com/oscar503sv/reelix && cd reelix
cargo build --release
./target/release/reelix
```

CLI:

```bash
reelix              # abre la ventana
reelix --record     # abre la ventana y lanza el selector directamente
reelix --list       # lista las grabaciones y sale
reelix --version
RUST_LOG=debug reelix   # logs detallados
```

Atajos (con la ventana enfocada):

| Atajo | Acción |
|---|---|
| Ctrl+Mayús+R | Empezar/detener grabación |
| Ctrl+Mayús+P | Pausar/reanudar |
| Ctrl+, | Preferencias |
| Esc | Cancelar (descarta la grabación) |
| Ctrl+Q | Salir |

Por defecto funcionan con la ventana de Reelix enfocada. En *Preferencias →
Atajos* puedes activar los **atajos globales**: GNOME pide confirmarlos y
desde entonces Ctrl+Mayús+R/P funcionan en todo el sistema.

Instalación local del lanzador e iconos: ver [DEVELOPMENT.md](DEVELOPMENT.md#instalación-local).

## Wayland, PipeWire y permisos

En Wayland ninguna aplicación puede leer la pantalla directamente. Reelix
pide la captura a **xdg-desktop-portal** (`org.freedesktop.portal.ScreenCast`);
GNOME muestra su propio diálogo y, si aceptas, entrega un descriptor de
PipeWire limitado a la pantalla/ventana elegida. Por eso:

- El diálogo de GNOME aparece en cada grabación (la Fase 4 podrá recordar la
  elección con *restore tokens*).
- Mientras graba, GNOME muestra un indicador en la barra superior; pulsarlo
  detiene la grabación.
- Reelix nunca se ejecuta como root (se niega a hacerlo) y no necesita
  permisos especiales.

## Configuración

`~/.config/reelix/settings.toml` (opcional; todo tiene valores por defecto):

```toml
[video]
fps = 30                 # 5–60, límite superior (la salida es VFR)
quality = "medium"       # low | medium | high
encoder = "auto"         # auto | hardware | software
show_cursor = true

[audio]
microphone = true
# microphone_device = "alsa_input.…"   # node.name; ver `wpctl status` / `pw-cli ls Node`
microphone_volume = 1.0  # 0.0–4.0
system_audio = false

[camera]
enabled = false
# device = "v4l2_input.…"  # node.name
size = "medium"          # small | medium | large (18 / 25 / 33 % de la altura)
shape = "rectangle"      # rectangle | circle (el círculo usa más CPU)
corner = "bottom-right"  # top-left | top-right | bottom-left | bottom-right
mirror = false

[storage]
# recording_dir = "/home/usuario/Vídeos/Reelix"
```

## Troubleshooting

| Síntoma | Causa / solución |
|---|---|
| "No se pudo acceder a la pantalla" | Comprueba que la sesión es Wayland (`echo $XDG_SESSION_TYPE`) y que `xdg-desktop-portal-gnome` está instalado. `systemctl --user restart xdg-desktop-portal`. |
| "Faltan componentes multimedia" | Falta `pipewire-gstreamer` (`gst-inspect-1.0 pipewiresrc`) u otro plugin. |
| "No hay codificador de vídeo disponible" | `sudo dnf install gstreamer1-plugin-openh264`. |
| CPU alta al grabar | Estás usando codificación por software; instala `intel-media-driver` (ver arriba). |
| Aviso "No se encontró ningún micrófono" | Revisa Configuración → Sonido → Entrada de GNOME y `wpctl status`. La grabación continúa sin micrófono. |
| La barra de nivel no se mueve | El micrófono está silenciado o con volumen 0 en GNOME, o se eligió otro dispositivo. |
| La burbuja tiene un cuadrado oscuro o difuminado alrededor | Lo pinta algo externo a Reelix: (1) un tema GTK con `window { background-color: … }` en `~/.config/gtk-4.0/gtk.css` (Reelix ya lo anula para la burbuja); (2) **Blur my Shell** con difuminado de aplicaciones: añade `io.github.oscar503sv.Reelix` a *Applications → Blacklist*. |
| La burbuja no se queda encima o no sigue al cambiar de escritorio | Wayland no permite que la app lo decida: con el ratón sobre la burbuja, Alt+Espacio → «Siempre encima» y «Siempre en el área de trabajo visible». |
| Aviso "No hay cámara disponible" | Comprueba que la cámara aparece en `wpctl status` (sección Video) y que no la usa otra app. |
| Acceso a la cámara denegado | Configuración → Privacidad → Cámara. |
| El audio del sistema no se graba | Necesita `pipewire-pulseaudio` (capa PulseAudio de PipeWire): `pactl info` debe responder. |
| Log con `stream error: target not found` al parar | Normal: el portal cerró el stream después de finalizar el archivo. |

Para depurar: `RUST_LOG=reelix=debug GST_DEBUG=pipewiresrc:4,pulsesrc:4,mp4mux:4 reelix`.

## Arquitectura

```
GTK4/libadwaita (UI) → Controller + máquina de estados → Portal ScreenCast (ashpd)
   → PipeWire fd → GStreamer: pipewiresrc → videoconvert → encoder H.264 ─┐
   micrófono / monitor del sistema → pulsesrc → audiomixer → limitador → AAC ─┴→ mp4mux → archivo
   (con cámara: portal Camera → pipewiresrc → recorte/escala ─→ compositor antes del encoder)
```

Detalles y decisiones en [ARCHITECTURE.md](ARCHITECTURE.md).

## Hoja de ruta

- [x] **Fase 1**: GUI → portal → PipeWire → GStreamer → MP4.
- [x] **Fase 2**: micrófono y audio del sistema (PipeWire) + AAC en el mismo MP4.
- [x] **Fase 3**: cámara (portal Camera) + composición en el pipeline.
- [x] **Fase 4**: burbuja/overlay, pausa, cuenta atrás, biblioteca, preferencias, atajos globales (pendientes de verificar en uso real).
- [ ] **Fase 5**: medición y optimización (VA-API sin copias, DMA-BUF).
- [ ] Empaquetado RPM y Flatpak.

## Licencia

MIT. Ver [LICENSE](LICENSE).
