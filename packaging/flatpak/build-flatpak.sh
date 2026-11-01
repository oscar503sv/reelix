#!/bin/bash
# Builds Reelix as a Flatpak from the working tree, installs it for the
# current user and writes a single-file bundle (reelix.flatpak).
#
#   packaging/flatpak/build-flatpak.sh            # build + install + bundle
#   packaging/flatpak/build-flatpak.sh --no-install
#
# Needs: flatpak-builder, org.gnome.Platform//51, org.gnome.Sdk//51 and
# org.freedesktop.Sdk.Extension.rust-stable//26.08 (from Flathub).
set -euo pipefail

ROOT=$(git -C "$(dirname "$0")" rev-parse --show-toplevel)
cd "$ROOT"
APP_ID=io.github.oscar503sv.Reelix
MANIFEST=packaging/flatpak/$APP_ID.yml
OUT=${FLATPAK_OUT:-$ROOT/target/flatpak}
mkdir -p "$OUT"

# Keep the offline crate list in sync with Cargo.lock.
python3 -I packaging/flatpak/cargo-sources.py Cargo.lock > packaging/flatpak/cargo-sources.json

INSTALL=(--user --install)
[ "${1:-}" = "--no-install" ] && INSTALL=()
flatpak-builder --force-clean --state-dir="$OUT/state" --repo="$OUT/repo" \
    "${INSTALL[@]}" "$OUT/build" "$MANIFEST"
flatpak build-bundle --runtime-repo=https://dl.flathub.org/repo/flathub.flatpakrepo \
    "$OUT/repo" "$OUT/reelix.flatpak" "$APP_ID"
echo "Bundle: $OUT/reelix.flatpak"
