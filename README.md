# Ceiling

Ceiling is a focused, local-first Windows companion for the AI subscriptions you actually use. It keeps rolling limits, reset times, and stale/error states visible from the system tray or a lightweight capacity strip above the taskbar.

This is an early public fork of [Finesssee/Win-CodexBar](https://github.com/Finesssee/Win-CodexBar). The goal is not another giant provider dashboard. It is a fast, calm way to answer: **how much AI capacity do I have left, and when does it reset?**

## Initial focus

- OpenAI Codex
- Claude
- Cursor
- Gemini / Google AI
- GitHub Copilot

Additional providers remain available from the upstream foundation while Ceiling is narrowed around reliable support for the core five.

## What Ceiling will feel like

- **Taskbar-adjacent capacity strip:** Windows 11 does not support old-style third-party taskbar toolbars, so Ceiling uses a transparent, always-on-top strip that stays just above the taskbar without stealing focus.
- **Tray at a glance:** a compact flyout with each provider's remaining capacity, reset time, source, and freshness.
- **Truthful state:** a visible distinction between live, cached, stale, and failed reads. No fake precision when a provider cannot report a limit cleanly.
- **Local first:** credentials and usage data stay on the machine. Browser cookies, API keys, and login sources remain opt-in.
- **Windows-native:** Tauri, React, and Rust; fast startup, low idle work, and system accent-aware appearance.

## Current status

Ceiling is in its foundation phase. The upstream Windows tray, provider, credential, and capacity-strip architecture is in place; the product is now being rebranded and intentionally simplified around the providers above.

## Development

```powershell
git clone https://github.com/tsouth89/ceiling.git
cd ceiling
pnpm --dir apps/desktop-tauri install --frozen-lockfile
pnpm --dir apps/desktop-tauri tauri:dev
```

The active desktop app lives in `apps/desktop-tauri`. Shared provider and usage logic lives in `rust`.

For the active implementation state and the next work items, see [docs/HANDOFF.md](docs/HANDOFF.md). For the tray and strip visual system, see [docs/CEILING_UI.md](docs/CEILING_UI.md).

## Credits and license

Ceiling is a fork of [Win-CodexBar](https://github.com/Finesssee/Win-CodexBar), which ports the ideas of [CodexBar](https://github.com/steipete/CodexBar) to Windows. It retains the upstream [MIT license](LICENSE) and attribution.
