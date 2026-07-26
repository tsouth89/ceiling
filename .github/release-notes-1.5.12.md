## Highlights

### Antigravity works again on Windows

Ceiling now finds both:

- **Antigravity desktop** language servers that advertise `--https_server_port 0` (current 2.3+ builds)
- **Antigravity CLI** (`agy`), which serves the same local quota API without a CSRF token

If you saw "language server not running" while Antigravity or `agy` was open and signed in, install 1.5.12, leave the app/CLI running, and refresh.

Gemini stays its own provider (Gemini CLI OAuth / daily pool). Antigravity is separate (local multi-model quotas).

### One card per account

Registering the signed-in Codex or Claude home no longer leaves a ghost ambient reading beside it. Overview shows each seat once.

### Cleaner taskbar strip

- Providers that are enabled but not ready yet no longer appear as blank dash pills
- Gemini gets a real star glyph instead of an empty ring
- Antigravity gets a strip mark too

## Installers

- **Ceiling-1.5.12-Setup.exe** – standard installer
- **Ceiling-1.5.12-portable.exe** – portable
- **Ceiling-1.5.12-Store-Setup.exe** – Microsoft Store package (WebView2 bundled)

---

Patch on **1.5.11**.

**Full Changelog**: https://github.com/tsouth89/ceiling/compare/v1.5.11...v1.5.12
