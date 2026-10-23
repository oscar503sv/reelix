# Contribuir a Reelix

¡Gracias por tu interés! Reelix es un grabador de pantalla nativo para
GNOME/Wayland. Antes de contribuir:

1. Lee [ARCHITECTURE.md](ARCHITECTURE.md) para entender los módulos y la
   máquina de estados.
2. Prepara el entorno con [DEVELOPMENT.md](DEVELOPMENT.md).

## Flujo

1. Abre un *issue* describiendo el bug o la propuesta (para cambios grandes,
   antes de escribir código).
2. Crea una rama desde `main`.
3. Asegúrate de que pasa:
   ```bash
   cargo fmt --check
   cargo clippy --all-targets -- -D warnings
   cargo test
   ```
4. Si tocas captura o pipeline, haz la prueba funcional manual (ver
   DEVELOPMENT.md) e indica en el PR distribución, escritorio, GPU y resultado.
5. Abre el Pull Request explicando *qué* y *por qué*.

## Principios

- **Wayland primero.** No se aceptan soluciones basadas en X11.
- **APIs estándar**: portales XDG, PipeWire, GStreamer. Nada de hacks
  específicos de un compositor sin alternativa documentada.
- **No reinventar multimedia**: si GStreamer lo resuelve, se usa GStreamer.
- **Local y privado**: nada de red ni telemetría.
- **Medir antes de optimizar**: incluye cifras (CPU, RSS, fps) en PRs de
  rendimiento.
- Sin `unwrap()` en producción, sin `unsafe`, errores comprensibles.

## Licencia

Al contribuir aceptas que tu código se publique bajo la licencia MIT del
proyecto.
