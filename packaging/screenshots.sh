#!/bin/bash
# Renders Reelix's screenshots (AppStream/Flathub and README) into
# data/screenshots, from a clean temporary profile: default settings, the
# stock Adwaita theme (no ~/.config/gtk-4.0/gtk.css) and sample recordings.
# Nothing in your real config or recordings folder is read or changed.
#
#   packaging/screenshots.sh
set -euo pipefail

ROOT=$(git -C "$(dirname "$0")" rev-parse --show-toplevel)
cd "$ROOT"
cargo build --release --quiet

PROFILE=$(mktemp -d)
trap 'rm -rf "$PROFILE"' EXIT
VIDEOS="$PROFILE/Videos/Reelix"
mkdir -p "$PROFILE/config/reelix" "$VIDEOS"
cat > "$PROFILE/config/reelix/settings.toml" <<TOML
[audio]
microphone = true
system_audio = true

[camera]
enabled = false

[storage]
recording_dir = "$VIDEOS"
TOML

# Sample library: title, minutes ago, duration (s), size.
sample() {
    local title=$1 ago=$2 duration=$3 size=$4
    local when stamp
    when=$(date -d "-$ago minutes" --iso-8601=seconds)
    stamp=$(date -d "-$ago minutes" +%Y-%m-%d-%H%M%S)
    truncate -s "$size" "$VIDEOS/recording-$stamp.mp4"   # sparse: no disk used
    cat > "$VIDEOS/recording-$stamp.json" <<JSON
{"title": "$title", "created_at": "$when", "duration": $duration,
 "resolution": "1920x1080", "fps": 30, "audio": true, "camera": true,
 "encoder": "vah264lpenc"}
JSON
    touch -d "-$ago minutes" "$VIDEOS/recording-$stamp.mp4" "$VIDEOS/recording-$stamp.json"
}
sample "Demo de la nueva función de exportación" 25 154 21M
sample "Revisión del diseño de la pantalla de inicio" 190 412 48M
sample "Cómo configurar el entorno de desarrollo" 1500 738 86M
sample "Respuesta al reporte de error #42" 2950 95 12M
sample "Presentación del sprint" 8700 1265 132M

XDG_CONFIG_HOME="$PROFILE/config" LANG=es_ES.UTF-8 \
    RUST_LOG=info,zbus=warn,zbus::proxy=error \
    ./target/release/reelix --screenshots "$ROOT/data/screenshots"
ls -la data/screenshots
