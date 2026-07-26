## Highlights

### Antigravity works on current Windows builds

Ceiling now detects Google Antigravity when the language server advertises `--https_server_port 0` and does not set `--extension_server_port` (Antigravity 2.3+). If you had "language server not running" while Antigravity was open and signed in, refresh after updating.

### One card per Codex (or Claude) account

Registering the signed-in home no longer leaves a ghost ambient reading beside it. Overview shows each seat once; removing an account still drops its card.

### Taskbar stays capacity-only

Enabling a provider that is not installed or not signed in no longer adds a blank dash pill to the strip. Setup failures stay on Overview and Settings. This also stops Antigravity error placeholders from looking like a broken "Claude" tile (its quota window is named Claude).

## Installers

- **Ceiling-1.5.12-Setup.exe** - standard installer
- **Ceiling-1.5.12-portable.exe** - portable
- **Ceiling-1.5.12-Store-Setup.exe** - Microsoft Store package (WebView2 bundled)

---

Patch on **1.5.11**.

**Full Changelog**: https://github.com/tsouth89/ceiling/compare/v1.5.11...v1.5.12
