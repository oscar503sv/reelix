# Contributing to Reelix

**English** · [Español](CONTRIBUTING.es.md)

Thanks for your interest! Reelix is a native screen recorder for
GNOME/Wayland. Every change, including translations, goes through a **pull
request**: `main` is protected and only accepts merges from reviewed PRs with
green checks.

- [Code](#code)
- [Translations](#translations): add or improve a language
- [Principles](#principles)

## Code

1. Read [ARCHITECTURE.md](ARCHITECTURE.md) (modules, state machine) and set up
   your environment with [DEVELOPMENT.md](DEVELOPMENT.md). Both are in
   Spanish; ask in an issue if anything is unclear.
2. For bugs or big changes, open an issue first so we can agree on the
   approach before you write code.
3. Fork the repository and create a branch from `main`.
4. Make sure these pass (CI runs them too):
   ```bash
   cargo fmt --check
   cargo clippy --all-targets -- -D warnings
   cargo test
   po/update.sh --check
   ```
5. Any new text shown to users must be in English and wrapped in
   `gettext("…")` (see [Translations](#translations)). Then run
   `po/update.sh` and commit the updated `po/` files.
6. If you change capture or the pipeline, do the manual test from
   DEVELOPMENT.md and write your distribution, desktop, GPU and the result in
   the PR.
7. Open the pull request explaining *what* changes and *why*.

## Translations

The interface, the app menu entry (`.desktop`) and the software-center
listing (metainfo) are translated with **gettext**. The strings live in
[`po/`](po/):

| File | What it is |
|---|---|
| `po/reelix.pot` | Template with every English string (generated, don't edit) |
| `po/<lang>.po` | One translation per language, e.g. `po/es.po` |
| `po/LINGUAS` | The languages that are built |
| `po/POTFILES` | Source files with translatable strings |

You need `gettext` (`sudo dnf install gettext` on Fedora) and, ideally, a PO
editor such as [Gtranslator](https://flathub.org/apps/org.gnome.Gtranslator)
or Poedit. A text editor works too.

### Add a new language

1. Fork the repository and create a branch, e.g. `translation-fr`.
2. Create the file from the template, using your language code (`fr`,
   `pt_BR`, `de`…):
   ```bash
   msginit --locale=fr --input=po/reelix.pot --output-file=po/fr.po
   ```
3. Add the code to `po/LINGUAS` (one per line, sorted).
4. Translate every `msgstr` in `po/fr.po`. Read the `#.` comments above a
   string: they explain the context. Keep the `{placeholders}` exactly as
   they are (you can move them), and keep the keyboard shortcuts.
5. Put your name in the `translator-credits` string (shown in *About*).
6. Check it and try it:
   ```bash
   po/update.sh --check            # no errors
   po/update.sh --stats            # how much is translated
   LANGUAGE=fr cargo run           # the app in your language
   ```
   If your system doesn't have that locale, `sudo dnf install
   glibc-langpack-fr`.
7. Commit only `po/fr.po` and `po/LINGUAS`, and open a pull request titled
   *"Add French translation"*.

### Improve an existing translation

Edit `po/<lang>.po`, run `po/update.sh --check`, and open a pull request.
Strings marked `#, fuzzy` are guesses from an old text: review them and
remove the `fuzzy` line.

### When the English text changes

Maintainers (or the author of a code PR) run `po/update.sh`, which
regenerates `po/reelix.pot` and merges it into every `po/<lang>.po`. New
strings show up empty and changed ones as `fuzzy`; untranslated strings are
displayed in English.

## Principles

- **Wayland first.** X11-based solutions are not accepted.
- **Standard APIs**: XDG portals, PipeWire, GStreamer. No compositor-specific
  hacks without a documented alternative.
- **Don't reinvent multimedia**: if GStreamer solves it, use GStreamer.
- **Local and private**: no network access and no telemetry.
- **Measure before optimizing**: include numbers (CPU, RSS, fps) in
  performance PRs.
- No `unwrap()` in production code, no `unsafe` (the only exception is
  `setlocale` in `src/i18n.rs`), and errors users can understand.

## License

By contributing you agree that your work is published under the project's
MIT license.
