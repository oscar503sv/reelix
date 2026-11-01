#!/usr/bin/env bash
# Installs everything except the binary under PREFIX: translations (.mo),
# the translated .desktop and metainfo files, and the icons. Used by the
# RPM spec (PREFIX=%{buildroot}/usr) and the Flatpak manifest (PREFIX=/app).
#
#   packaging/install-data.sh PREFIX
set -euo pipefail
prefix=${1:?usage: $0 PREFIX}
cd "$(dirname "$0")/.."

app_id=io.github.oscar503sv.Reelix
linguas=$(sed '/^#/d;/^[[:space:]]*$/d' po/LINGUAS)

for lang in $linguas; do
    install -d "$prefix/share/locale/$lang/LC_MESSAGES"
    msgfmt --check -o "$prefix/share/locale/$lang/LC_MESSAGES/reelix.mo" "po/$lang.po"
done

install -d "$prefix/share/applications" "$prefix/share/metainfo"
msgfmt --desktop -d po --template "data/$app_id.desktop.in" \
    -o "$prefix/share/applications/$app_id.desktop"
msgfmt --xml -d po --template "data/$app_id.metainfo.xml.in" \
    -o "$prefix/share/metainfo/$app_id.metainfo.xml"

cp -r data/icons "$prefix/share/"
