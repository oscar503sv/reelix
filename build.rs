//! Compiles the translations in `po/` so that `cargo run` is translated.
//! Packages install them with `packaging/install-data.sh` instead.

use std::path::PathBuf;
use std::process::Command;

fn main() {
    println!("cargo:rerun-if-changed=po");
    let out = PathBuf::from(std::env::var_os("OUT_DIR").unwrap()).join("locale");
    let linguas = std::fs::read_to_string("po/LINGUAS").unwrap_or_default();
    for lang in linguas
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
    {
        let dir = out.join(lang).join("LC_MESSAGES");
        std::fs::create_dir_all(&dir).unwrap();
        let status = Command::new("msgfmt")
            .arg("--check")
            .arg("-o")
            .arg(dir.join("reelix.mo"))
            .arg(format!("po/{lang}.po"))
            .status();
        match status {
            Ok(s) if s.success() => {}
            Ok(_) => panic!("po/{lang}.po has errors (msgfmt --check)"),
            Err(e) => {
                println!("cargo:warning=msgfmt not found ({e}); the UI will be in English");
                return;
            }
        }
    }
    println!("cargo:rustc-env=REELIX_BUILD_LOCALEDIR={}", out.display());
}
