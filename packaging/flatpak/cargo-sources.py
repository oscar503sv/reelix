#!/usr/bin/env python3
"""Generates cargo-sources.json (flatpak-builder sources) from Cargo.lock,
so the Flatpak builds offline: every crates.io package is downloaded by
flatpak-builder and vendored into cargo/vendor. Same output format as
flatpak-builder-tools' flatpak-cargo-generator for crates.io-only locks.

    python3 packaging/flatpak/cargo-sources.py Cargo.lock > packaging/flatpak/cargo-sources.json
"""
import json
import sys
import tomllib

CRATES_IO = "registry+https://github.com/rust-lang/crates.io-index"


def main(lock_path: str) -> None:
    with open(lock_path, "rb") as f:
        lock = tomllib.load(f)
    sources = []
    for package in lock["package"]:
        source = package.get("source")
        if source is None:
            continue  # the workspace itself
        if source != CRATES_IO:
            sys.exit(f"unsupported source for {package['name']}: {source}")
        name, version, checksum = package["name"], package["version"], package["checksum"]
        dest = f"cargo/vendor/{name}-{version}"
        sources.append({
            "type": "archive",
            "archive-type": "tar-gzip",
            "url": f"https://static.crates.io/crates/{name}/{name}-{version}.crate",
            "sha256": checksum,
            "dest": dest,
        })
        sources.append({
            "type": "inline",
            "contents": json.dumps({"package": checksum, "files": {}}),
            "dest": dest,
            "dest-filename": ".cargo-checksum.json",
        })
    sources.append({
        "type": "inline",
        "contents": "[source.vendored-sources]\ndirectory = \"cargo/vendor\"\n\n"
                    "[source.crates-io]\nreplace-with = \"vendored-sources\"\n",
        "dest": "cargo",
        "dest-filename": "config.toml",
    })
    json.dump(sources, sys.stdout, indent=4)
    sys.stdout.write("\n")


if __name__ == "__main__":
    main(sys.argv[1] if len(sys.argv) > 1 else "Cargo.lock")
