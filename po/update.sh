#!/usr/bin/env bash
# Translation maintenance (needs gettext: sudo dnf install gettext).
#
#   po/update.sh           regenerate po/reelix.pot from the sources and
#                          merge it into every po/<lang>.po
#   po/update.sh --check   fail if po/reelix.pot is out of date, a file with
#                          translatable strings is missing from POTFILES, or
#                          a translation has errors (used by CI)
#   po/update.sh --stats   show how complete each translation is
set -euo pipefail
cd "$(dirname "$0")/.."

mode=${1:-update}
version=$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -1)
linguas=$(sed '/^#/d;/^[[:space:]]*$/d' po/LINGUAS)

extract() {
    xgettext --from-code=UTF-8 --add-comments=Translators \
        --keyword=gettext --keyword=N_ --keyword=ngettext:1,2 --keyword=pgettext:1c,2 --keyword=NC_:1c,2 \
        --add-location=file --sort-by-file \
        --package-name=reelix --package-version="$version" \
        --msgid-bugs-address=https://github.com/oscar503sv/reelix/issues \
        --copyright-holder="The Reelix authors" \
        --files-from=<(grep -v '^#' po/POTFILES) --output="$1"
}

case $mode in
update)
    extract po/reelix.pot
    for lang in $linguas; do
        msgmerge --quiet --update --backup=none --add-location=file --sort-by-file \
            "po/$lang.po" po/reelix.pot
    done
    echo "Updated po/reelix.pot and: $linguas"
    ;;
--check)
    status=0
    missing=$(comm -23 \
        <(grep -rlE '\b(gettext|ngettext|pgettext|N_|NC_)\(' src | sort) \
        <(grep '^src/' po/POTFILES | sort))
    if [ -n "$missing" ]; then
        echo "Missing from po/POTFILES:"; echo "$missing"; status=1
    fi
    tmp=$(mktemp); trap 'rm -f "$tmp"' EXIT
    extract "$tmp"
    if ! diff -q <(grep -v '^"POT-Creation-Date' po/reelix.pot) \
                 <(grep -v '^"POT-Creation-Date' "$tmp") >/dev/null; then
        echo "po/reelix.pot is out of date: run po/update.sh and commit the result."
        status=1
    fi
    for lang in $linguas; do
        [ -f "po/$lang.po" ] || { echo "po/LINGUAS lists $lang but po/$lang.po is missing"; status=1; continue; }
        msgfmt --check --output-file=/dev/null "po/$lang.po" || status=1
    done
    for po in po/*.po; do
        lang=$(basename "$po" .po)
        grep -qx "$lang" <(echo "$linguas") || { echo "$po is not listed in po/LINGUAS"; status=1; }
    done
    exit $status
    ;;
--stats)
    for lang in $linguas; do
        printf '%-8s' "$lang"; msgfmt --statistics --output-file=/dev/null "po/$lang.po"
    done
    ;;
*)
    sed -n '2,10p' "$0"; exit 2
    ;;
esac
