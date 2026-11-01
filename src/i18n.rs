//! Interface translations (gettext).
//!
//! Source strings are in English and wrapped in [`gettext`] (or marked with
//! [`N_`] when they live in a `const` and are translated where they are
//! shown). Translations are `po/<lang>.po`; see `CONTRIBUTING.md`.
//!
//! Placeholders use `{name}` and are filled with [`fill`], so translators
//! can reorder them.

use std::fmt::Display;
use std::path::{Path, PathBuf};

use gettextrs::LocaleCategory;
pub use gettextrs::{gettext, ngettext, pgettext};

/// Gettext domain: `reelix.mo` files.
pub const DOMAIN: &str = "reelix";

/// Selects the user's language and loads the catalog. Call once at the
/// start of `main`, before any other thread exists.
pub fn init() {
    // SAFETY: setlocale is not thread-safe; this runs on the main thread
    // before GTK, GStreamer or any other thread is started.
    unsafe {
        gettextrs::setlocale(LocaleCategory::LcAll, "");
    }

    let dir = locale_dir();
    let result = gettextrs::bindtextdomain(DOMAIN, dir.as_path())
        .and_then(|_| gettextrs::bind_textdomain_codeset(DOMAIN, "UTF-8"))
        .and_then(|_| gettextrs::textdomain(DOMAIN));
    match result {
        Ok(_) => tracing::debug!(dir = %dir.display(), "translations loaded"),
        Err(e) => tracing::warn!(error = %e, "translations unavailable"),
    }
}

/// Marks a string for translation without translating it (for `const`
/// tables); pass it through [`gettext`] where it is displayed.
#[allow(non_snake_case)]
pub const fn N_(msgid: &str) -> &str {
    msgid
}

/// Like [`N_`] with a context that tells apart equal English words
/// ("Medium" size vs quality); translate with [`pgettext`] and the same
/// context.
#[allow(non_snake_case)]
pub const fn NC_<'a>(_context: &str, msgid: &'a str) -> &'a str {
    msgid
}

/// Replaces each `{name}` in a translated template with its value.
pub fn fill(template: &str, args: &[(&str, &dyn Display)]) -> String {
    args.iter()
        .fold(template.to_owned(), |text, (name, value)| {
            text.replace(&format!("{{{name}}}"), &value.to_string())
        })
}

/// Date and time in the translated short format, e.g. "11/01/2026 14:05".
pub fn date_time<Tz: chrono::TimeZone>(dt: &chrono::DateTime<Tz>) -> String
where
    Tz::Offset: Display,
{
    // Translators: date and time format (chrono strftime). Use numbers
    // only: month and day names are always in English.
    strftime(dt, &gettext("%m/%d/%Y %H:%M"), "%Y-%m-%d %H:%M")
}

/// Like [`date_time`] with seconds.
pub fn date_time_seconds<Tz: chrono::TimeZone>(dt: &chrono::DateTime<Tz>) -> String
where
    Tz::Offset: Display,
{
    // Translators: date and time format with seconds (chrono strftime).
    // Use numbers only.
    strftime(dt, &gettext("%m/%d/%Y %H:%M:%S"), "%Y-%m-%d %H:%M:%S")
}

/// Formats with a translated pattern, falling back to `fallback` if a
/// translation contains an invalid specifier (which would panic).
fn strftime<Tz: chrono::TimeZone>(
    dt: &chrono::DateTime<Tz>,
    pattern: &str,
    fallback: &str,
) -> String
where
    Tz::Offset: Display,
{
    use chrono::format::{Item, StrftimeItems};
    let valid = !StrftimeItems::new(pattern).any(|item| matches!(item, Item::Error));
    dt.format(if valid { pattern } else { fallback })
        .to_string()
}

/// Where the `.mo` files are: `$REELIX_LOCALEDIR`, then
/// `<prefix>/share/locale` next to the executable (RPM `/usr`, Flatpak
/// `/app`), then the catalogs `build.rs` compiled (running from source).
fn locale_dir() -> PathBuf {
    if let Some(dir) = std::env::var_os("REELIX_LOCALEDIR") {
        return dir.into();
    }
    let installed = std::env::current_exe()
        .ok()
        .and_then(|exe| Some(exe.parent()?.parent()?.join("share/locale")));
    let built = option_env!("REELIX_BUILD_LOCALEDIR").map(PathBuf::from);
    installed
        .into_iter()
        .chain(built)
        .find(|dir| has_catalogs(dir))
        .unwrap_or_else(|| PathBuf::from("/usr/share/locale"))
}

fn has_catalogs(dir: &Path) -> bool {
    std::fs::read_dir(dir).is_ok_and(|langs| {
        langs.flatten().any(|lang| {
            lang.path()
                .join("LC_MESSAGES")
                .join(format!("{DOMAIN}.mo"))
                .is_file()
        })
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fill_replaces_named_placeholders_in_any_order() {
        let text = fill("{b} then {a}, {a} again", &[("a", &1), ("b", &"two")]);
        assert_eq!(text, "two then 1, 1 again");
    }

    #[test]
    fn invalid_translated_date_format_falls_back() {
        let dt = chrono::DateTime::parse_from_rfc3339("2026-11-01T14:05:09+00:00").unwrap();
        assert_eq!(strftime(&dt, "%d.%m.%Y", "-"), "01.11.2026");
        assert_eq!(strftime(&dt, "%Q bad", "%Y-%m-%d"), "2026-11-01");
    }

    #[test]
    fn untranslated_text_is_returned_unchanged() {
        // A variable, so xgettext does not extract it.
        let msgid = "Not in any catalog";
        assert_eq!(gettext(msgid), msgid);
    }
}
