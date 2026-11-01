#!/bin/bash
# Renders Reelix's screenshots (AppStream/Flathub and README) into
# data/screenshots (one set per language), from a clean temporary
# profile: default settings, the stock Adwaita theme (no
# ~/.config/gtk-4.0/gtk.css) and sample recordings.
# Nothing in your real config or recordings folder is read or changed.
#
#   packaging/screenshots.sh
set -euo pipefail

ROOT=$(git -C "$(dirname "$0")" rev-parse --show-toplevel)
cd "$ROOT"
cargo build --release --quiet

# One set per language: English (the default, data/screenshots) and each
# translation listed here (data/screenshots/<lang>), with sample titles in
# that language. The metainfo lists them with xml:lang.
#   lang  locale       output
LANGS=(
    "en   en_US.UTF-8  data/screenshots"
    "es   es_ES.UTF-8  data/screenshots/es"
)

# Sample titles per language, in the order of the samples below.
titles_en=(
    "Demo of the new export feature"
    "Home screen design review"
    "How to set up the development environment"
    "Reply to bug report #42"
    "Sprint presentation"
)
titles_es=(
    "Demo de la nueva función de exportación"
    "Revisión del diseño de la pantalla de inicio"
    "Cómo configurar el entorno de desarrollo"
    "Respuesta al reporte de error #42"
    "Presentación del sprint"
)

render() {
    local lang=$1 locale=$2 out=$3
    local PROFILE VIDEOS
    PROFILE=$(mktemp -d)
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
    local -n titles="titles_$lang"
    sample "${titles[0]}" 25 154 21M
    sample "${titles[1]}" 190 412 48M
    sample "${titles[2]}" 1500 738 86M
    sample "${titles[3]}" 2950 95 12M
    sample "${titles[4]}" 8700 1265 132M

    mkdir -p "$out"
    XDG_CONFIG_HOME="$PROFILE/config" LC_ALL="$locale" LANGUAGE="$lang" \
        RUST_LOG=info,zbus=warn,zbus::proxy=error \
        ./target/release/reelix --screenshots "$ROOT/$out"
    rm -rf "$PROFILE"
}

for entry in "${LANGS[@]}"; do
    read -r lang locale out <<<"$entry"
    render "$lang" "$locale" "$out"
done
ls -la data/screenshots data/screenshots/*/
