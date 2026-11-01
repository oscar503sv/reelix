# Desarrollo

## Dependencias (Fedora)

```bash
sudo dnf install rust cargo gcc pkgconf-pkg-config \
    gtk4-devel libadwaita-devel gstreamer1-devel gstreamer1-plugins-base-devel \
    pipewire-gstreamer gstreamer1-plugins-good gstreamer1-plugins-bad-free \
    gstreamer1-plugin-openh264 fdk-aac-free pipewire-pulseaudio
```

Rust stable ≥ 1.85 (edición 2024).

## Compilar, probar, ejecutar

```bash
cargo build                    # debug
cargo build --release          # optimizado (LTO thin)
cargo test                     # unitarios + integración
cargo clippy --all-targets     # debe salir sin avisos
cargo fmt

RUST_LOG=reelix=debug cargo run
RUST_LOG=reelix=debug GST_DEBUG=pipewiresrc:4,mp4mux:4 cargo run -- --record
cargo run -- --list
```

Tests:

- `src/**` (`#[cfg(test)]`): máquina de estados, configuración, nombres,
  metadatos, selección de encoder.
- `tests/library.rs`: biblioteca en un directorio temporal (listar,
  renombrar, borrar, metadatos corruptos).
- `tests/encoding.rs`: codifica un clip real con el encoder detectado y lo
  valida con `qtdemux`. Se omite si GStreamer no está disponible.
- `tests/recording_pipeline.rs`: ejecuta el `RecordingPipeline` de
  producción con fuentes de prueba en vivo (vídeo + dos de audio): parada por
  EOS, duraciones de audio/vídeo alineadas, pista estéreo, mensajes de nivel
  y que una mezcla a +6 dBFS sale limitada por debajo de 0 dBFS. Cada test usa
  su propio `glib::MainContext` (los tests corren en paralelo). También
  compone una "cámara" verde sobre una "pantalla" roja y comprueba píxel a
  píxel posición, tamaño y máscara circular.
- La cadena de vista previa de la burbuja (`build_preview_chain`) se prueba
  con `videotestsrc`: por CPU entrega BGRx sin escalar; por GPU negocia un
  DMA-BUF de `vapostproc` (se omite sin VA-API). El dibujo (recorte, espejo,
  borde) son funciones puras con tests en `ui::bubble_video`.
- `max_height_downscales_the_recording` graba con resolución máxima y
  comprueba el tamaño del MP4.
- Con cámara real (no corre en CI): `cargo test -- --ignored real_camera`
  (composición y vista previa).

La captura vía portal no se puede automatizar (GNOME siempre pide
confirmación), así que la prueba funcional es manual:

1. `RUST_LOG=reelix=debug cargo run`
2. Grabar pantalla completa ~15 s, Detener.
3. Comprobar: `ffprobe ~/Vídeos/Reelix/recording-*.mp4` y que
   `ffmpeg -v error -i <archivo> -f null -` no muestra errores.
4. Repetir con una ventana, y deteniendo desde el indicador de GNOME.
5. Audio: grabar hablando y con música sonando ("Audio del sistema"
   activado). Comprobar con
   `ffprobe -show_entries stream=codec_type,channels,start_time,duration`
   que hay pista AAC estéreo alineada con el vídeo, y escuchar que la voz
   coincide con la imagen.
6. Cámara: grabar con rectángulo y con círculo; comprobar posición, bordes
   y sincronía labios-voz.
7. Burbuja: activar la cámara, arrastrarla, cambiar forma/tamaño, grabar la
   pantalla completa y detener desde la burbuja; comprobar en el vídeo que
   no hay fondo alrededor del círculo. Si lo hay, revisar el tema GTK y
   Blur my Shell (ver README → Troubleshooting).
8. Fase 4: cuenta atrás (el número no debe salir en el vídeo), pausar y
   reanudar (sin hueco: duración del archivo = tiempo grabado), biblioteca
   (renombrar, detalles, carpeta, papelera), preferencias, y atajos globales
   desde Preferencias → Atajos.

## Medir rendimiento

```bash
cargo run --release --example bench -- --seconds 10                 # encoder automático
cargo run --release --example bench -- --encoder software --audio
cargo run --release --example bench -- --camera circle --content motion
```

Muestra CPU del proceso, RAM máxima, frames descartados, latencia de
codificación y bitrate real. Generar la escena sintética de 1080p cuesta
~42 % de un núcleo; réstalo al comparar con la app real. En la app, cada
grabación deja en el log `frames`, `dropped` y `avg_encode_ms`.

## Instalación local

```bash
cargo build --release
install -Dm755 target/release/reelix ~/.local/bin/reelix
install -Dm644 data/io.github.oscar503sv.Reelix.desktop ~/.local/share/applications/io.github.oscar503sv.Reelix.desktop
cp -r data/icons ~/.local/share/
update-desktop-database ~/.local/share/applications
```

Validar metadatos:

```bash
desktop-file-validate data/*.desktop
appstreamcli validate --no-net data/*.metainfo.xml
```

## Empaquetado

Ambos paquetes se construyen **sin red** durante la compilación: las
dependencias de Rust van vendorizadas (RPM) o descritas en
`packaging/flatpak/cargo-sources.json` (Flatpak).

### RPM

```bash
sudo dnf install rpm-build rpmlint cargo-rpm-macros
packaging/rpm/build-rpm.sh            # RPM binario + SRPM en ~/rpmbuild
packaging/rpm/build-rpm.sh --srpm     # solo SRPM (para COPR o mock)
rpmlint ~/rpmbuild/RPMS/x86_64/reelix-*.rpm packaging/rpm/reelix.spec
```

El script empaqueta **el último commit** (`git archive HEAD`) y crea
`reelix-VERSION-vendor.tar.xz` con `cargo vendor`. `%check` valida el
`.desktop` y el metainfo y corre los tests. El campo `License` incluye las
licencias de los crates enlazados (`%cargo_license_summary` en el log).

### Flatpak

```bash
sudo dnf install flatpak-builder
flatpak install --user flathub org.gnome.Platform//50 org.gnome.Sdk//50 \
    org.freedesktop.Sdk.Extension.rust-stable//25.08
packaging/flatpak/build-flatpak.sh    # construye, instala (usuario) y crea target/flatpak/reelix.flatpak
```

Construye desde el árbol de trabajo (incluye cambios sin commit).
`cargo-sources.py` regenera `cargo-sources.json` a partir de `Cargo.lock`
(mismo formato que `flatpak-cargo-generator`). Para Flathub, cambia la fuente
`dir` del manifiesto por la etiqueta `git` de la release.

El runtime trae GStreamer 1.26 con VA-API (extensión Intel), x264
(extensión codecs-extra), AAC de libav y `gtk4paintablesink`; Reelix detecta
los elementos en ejecución.

## Capturas de pantalla

```bash
packaging/screenshots.sh    # regenera data/screenshots/*.png
```

Arranca la app con un perfil temporal (ajustes por defecto, tema Adwaita sin
el `gtk.css` del usuario y grabaciones de ejemplo) y la opción oculta
`--screenshots DIR`, que renderiza cada vista con GTK directamente a PNG a
escala 2×. El metainfo y los README enlazan esas imágenes (en el metainfo,
desde `raw.githubusercontent.com/…/main/`).

## Al publicar una versión

1. Subir la versión en `Cargo.toml`, `packaging/rpm/reelix.spec` (Version y
   `%changelog`) y añadir `<release>` en el metainfo.
2. `cargo test`, commit y etiqueta `vX.Y.Z`.
3. Si cambió la interfaz, `packaging/screenshots.sh`.
4. `packaging/rpm/build-rpm.sh` y `packaging/flatpak/build-flatpak.sh`;
   adjuntar a la release de GitHub el `.rpm`, el `.flatpak` y el
   `reelix-X.Y.Z-vendor.tar.xz`.

## Renombrar el proyecto

1. `src/branding.rs` (APP_ID, APP_NAME, DIR_NAME, PROJECT_DIRS).
2. `Cargo.toml` (`name` del paquete, binario y librería).
3. `data/` (nombres de archivo e IDs dentro de .desktop/.metainfo.xml).

Cambiar `APP_ID` hace que GNOME trate la app como otra distinta (se pierden
los permisos de portal concedidos).

## Convenciones

- Nada de `unwrap()`/`expect()` en código de producción; `Result<T, AppError>`.
  En tests se permite `expect("motivo")`.
- Sin `unsafe`.
- Logs con `tracing` y campos estructurados (`tracing::info!(path = %p, "...")`).
- El hilo de UI nunca bloquea: el trabajo multimedia vive en hilos de
  GStreamer y las llamadas D-Bus son `async` sobre el main loop de GLib.
- Textos de la UI en español por ahora; la internacionalización (gettext)
  está pendiente.
