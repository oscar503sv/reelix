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
  su propio `glib::MainContext` (los tests corren en paralelo).

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

## Instalación local

```bash
cargo build --release
install -Dm755 target/release/reelix ~/.local/bin/reelix
install -Dm644 data/io.github.oscar503sv.Reelix.desktop ~/.local/share/applications/io.github.oscar503sv.Reelix.desktop
install -Dm644 data/icons/hicolor/scalable/apps/io.github.oscar503sv.Reelix.svg ~/.local/share/icons/hicolor/scalable/apps/io.github.oscar503sv.Reelix.svg
install -Dm644 data/icons/hicolor/symbolic/apps/io.github.oscar503sv.Reelix-symbolic.svg ~/.local/share/icons/hicolor/symbolic/apps/io.github.oscar503sv.Reelix-symbolic.svg
update-desktop-database ~/.local/share/applications
```

Validar metadatos:

```bash
desktop-file-validate data/*.desktop
appstreamcli validate --no-net data/*.metainfo.xml
```

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
