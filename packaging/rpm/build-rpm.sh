#!/bin/bash
# Builds the Reelix RPM offline: packs the committed tree (git archive) and
# the vendored Rust crates, then runs rpmbuild. Output in ~/rpmbuild/RPMS.
#
#   packaging/rpm/build-rpm.sh            # binary + source RPM
#   packaging/rpm/build-rpm.sh --srpm     # source RPM only (for COPR/mock)
set -euo pipefail

ROOT=$(git -C "$(dirname "$0")" rev-parse --show-toplevel)
cd "$ROOT"
VERSION=$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -1)
SPEC_VERSION=$(sed -n 's/^Version: *//p' packaging/rpm/reelix.spec)
if [ "$VERSION" != "$SPEC_VERSION" ]; then
    echo "Cargo.toml ($VERSION) and reelix.spec ($SPEC_VERSION) disagree" >&2
    exit 1
fi
if [ -n "$(git status --porcelain)" ]; then
    echo "warning: uncommitted changes are NOT included (git archive of HEAD)" >&2
fi

TOP=$(rpm --eval '%{_topdir}')
mkdir -p "$TOP"/{SOURCES,SPECS}
NAME=reelix-$VERSION

git archive --format=tar.gz --prefix="$NAME/" -o "$TOP/SOURCES/$NAME.tar.gz" HEAD

# Vendor into a scratch dir so the working tree stays clean.
WORK=$(mktemp -d)
trap 'rm -rf "$WORK"' EXIT
tar -xzf "$TOP/SOURCES/$NAME.tar.gz" -C "$WORK"
(cd "$WORK/$NAME" && cargo vendor --locked --versioned-dirs vendor >/dev/null)
tar -cJf "$TOP/SOURCES/$NAME-vendor.tar.xz" -C "$WORK/$NAME" vendor

cp packaging/rpm/reelix.spec "$TOP/SPECS/"
if [ "${1:-}" = "--srpm" ]; then
    rpmbuild -bs "$TOP/SPECS/reelix.spec"
else
    rpmbuild -ba "$TOP/SPECS/reelix.spec"
fi
