# Contribuir a Reelix

[English](CONTRIBUTING.md) · **Español**

¡Gracias por tu interés! Reelix es un grabador de pantalla nativo para
GNOME/Wayland. Todo cambio, incluidas las traducciones, entra mediante un
**pull request**: `main` está protegida y solo acepta fusiones de PRs
revisados con las comprobaciones en verde.

- [Código](#código)
- [Traducciones](#traducciones): añadir o mejorar un idioma
- [Principios](#principios)

## Código

1. Lee [ARCHITECTURE.md](ARCHITECTURE.md) (módulos, máquina de estados) y
   prepara el entorno con [DEVELOPMENT.md](DEVELOPMENT.md).
2. Para bugs o cambios grandes, abre antes un *issue* para acordar el
   enfoque.
3. Haz un fork y crea una rama desde `main`.
4. Comprueba que pasa (la CI también lo ejecuta):
   ```bash
   cargo fmt --check
   cargo clippy --all-targets -- -D warnings
   cargo test
   po/update.sh --check
   ```
5. Todo texto nuevo visible para el usuario va en inglés y dentro de
   `gettext("…")` (ver [Traducciones](#traducciones)). Después ejecuta
   `po/update.sh` y haz commit de los archivos de `po/` actualizados.
6. Si tocas la captura o el pipeline, haz la prueba manual de
   DEVELOPMENT.md e indica en el PR distribución, escritorio, GPU y
   resultado.
7. Abre el pull request explicando *qué* cambia y *por qué*.

## Traducciones

La interfaz, la entrada del menú de aplicaciones (`.desktop`) y la ficha del
centro de software (metainfo) se traducen con **gettext**. Los textos están
en [`po/`](po/):

| Archivo | Qué es |
|---|---|
| `po/reelix.pot` | Plantilla con todos los textos en inglés (generada, no editar) |
| `po/<idioma>.po` | Una traducción por idioma, p. ej. `po/es.po` |
| `po/LINGUAS` | Los idiomas que se compilan |
| `po/POTFILES` | Archivos fuente con textos traducibles |

Necesitas `gettext` (`sudo dnf install gettext` en Fedora) y, a ser posible,
un editor de PO como [Gtranslator](https://flathub.org/apps/org.gnome.Gtranslator)
o Poedit. También sirve un editor de texto.

### Añadir un idioma nuevo

1. Haz un fork y crea una rama, p. ej. `translation-fr`.
2. Crea el archivo desde la plantilla con el código de tu idioma (`fr`,
   `pt_BR`, `de`…):
   ```bash
   msginit --locale=fr --input=po/reelix.pot --output-file=po/fr.po
   ```
3. Añade el código a `po/LINGUAS` (uno por línea, en orden).
4. Traduce cada `msgstr` de `po/fr.po`. Lee los comentarios `#.` encima de
   cada texto: explican el contexto. Mantén los `{marcadores}` tal cual
   (puedes moverlos) y los atajos de teclado.
5. Pon tu nombre en el texto `translator-credits` (sale en *Acerca de*).
6. Compruébalo y pruébalo:
   ```bash
   po/update.sh --check            # sin errores
   po/update.sh --stats            # cuánto está traducido
   LANGUAGE=fr cargo run           # la app en tu idioma
   ```
   Si tu sistema no tiene ese idioma: `sudo dnf install glibc-langpack-fr`.
7. Haz commit solo de `po/fr.po` y `po/LINGUAS` y abre un pull request
   titulado *"Add French translation"*.

### Mejorar una traducción

Edita `po/<idioma>.po`, ejecuta `po/update.sh --check` y abre un pull
request. Los textos marcados `#, fuzzy` son suposiciones a partir de un
texto antiguo: revísalos y quita la línea `fuzzy`.

### Cuando cambia el texto en inglés

Quien mantiene el proyecto (o quien hace el PR de código) ejecuta
`po/update.sh`, que regenera `po/reelix.pot` y lo fusiona en cada
`po/<idioma>.po`. Los textos nuevos aparecen vacíos y los cambiados como
`fuzzy`; lo no traducido se muestra en inglés.

## Principios

- **Wayland primero.** No se aceptan soluciones basadas en X11.
- **APIs estándar**: portales XDG, PipeWire, GStreamer. Nada de hacks
  específicos de un compositor sin alternativa documentada.
- **No reinventar multimedia**: si GStreamer lo resuelve, se usa GStreamer.
- **Local y privado**: nada de red ni telemetría.
- **Medir antes de optimizar**: incluye cifras (CPU, RSS, fps) en PRs de
  rendimiento.
- Sin `unwrap()` en producción, sin `unsafe` (la única excepción es
  `setlocale` en `src/i18n.rs`), errores comprensibles.

## Licencia

Al contribuir aceptas que tu trabajo se publique bajo la licencia MIT del
proyecto.
