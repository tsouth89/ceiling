# WSL Support

CodexBar runs natively inside WSL. The CLI works out of the box; the desktop shell
requires [WSLg](https://github.com/microsoft/wslg) (Windows 11, build 22000+).

## Quick Start

```bash
git clone https://github.com/Finesssee/Win-CodexBar.git
cd Win-CodexBar
./dev.sh
```

This will:
1. Detect your WSL environment
2. Build CodexBar Desktop through Tauri's no-bundle workflow
3. Launch the desktop shell (WSLg) or CLI (no display server detected)

CLI-only mode (no display server needed):
```bash
./dev.sh --cli              # codexbar usage -p all
./dev.sh --release          # optimised build
```

## How It Works

When running inside WSL, CodexBar:

- **Browser cookies**: Reads Windows browser data from `/mnt/c/Users/<you>/AppData/...`.
  Chromium cookies encrypted with DPAPI cannot be decrypted from WSL automatically.
  Use manual cookies (Settings → provider detail → Browser Cookies) or CLI-based provider auth instead.
- **Provider CLIs**: Works with `codex`, `claude`, `gemini` etc. installed inside WSL natively.
- **Desktop shell**: Requires WSLg (Windows 11) or an X server. Falls back to CLI mode automatically.
- **Notifications**: Uses `notify-send` in WSL. Falls back to logging if unavailable.

## Authentication Tips

| Provider | WSL Auth Strategy |
|----------|-------------------|
| Codex | `npm i -g @openai/codex` inside WSL, then `codex login` |
| Claude | `npm i -g @anthropic-ai/claude-code` inside WSL, then `claude login` |
| Gemini | `gcloud auth login` inside WSL (requires gcloud CLI) |
| Cursor / Kimi | Manual cookies — copy from browser DevTools (F12 → Network → Cookie header) |
| Copilot | GitHub Device Flow works natively in WSL |

## Differences from Native Windows

| Feature | Windows | WSL |
|---------|---------|-----|
| Cookie Decryption | DPAPI (automatic) | Manual cookies only |
| Desktop Shell | Native | Via WSLg |
| Notifications | PowerShell toast | notify-send |
