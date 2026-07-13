# Ceiling handoff

Use this as the starting point for the next implementation session.

## Current state

- Repository: `C:\projects\personal\ceiling`
- Branch: `main`, clean and pushed to `origin` (`tsouth89/ceiling`)
- Latest commit: `535f693a Model inactive provider limit windows`
- Upstream remote: `Finesssee/Win-CodexBar`
- Public project tracker: [Ceiling in Linear](https://linear.app/southforge-ai/project/ceiling-6615aa2c9e6b)

Ceiling is a focused Windows companion for AI-subscription capacity. The initial user-facing scope is Codex/ChatGPT, Claude, Cursor, Gemini, and GitHub Copilot. It is intentionally built on the mature Win-CodexBar Tauri/Rust foundation instead of rebuilding tray, auth, or provider infrastructure.

## What is implemented

The first product-contract slice is complete and tested.

- `UsageSnapshot` now supports `inactive_rate_windows` in addition to ordinary measured rate windows.
- A Codex API response that reports a weekly meter but omits the five-hour meter now exposes:
  - the real weekly meter, and
  - `5-hour · Not currently enforced by OpenAI`.
- This is explicitly **not** shown as `100% left`, a zero-use bar, or a permanent unlimited entitlement.
- The Rust-to-Tauri-to-React bridge carries the inactive-window state.
- `MenuCard` renders inactive windows as text rather than a fabricated progress bar.

Relevant files:

- `rust/src/core/usage_snapshot.rs` — common usage and inactive-window models.
- `rust/src/providers/codex/api.rs` — Codex response parsing and regression fixture.
- `apps/desktop-tauri/src-tauri/src/commands/bridge.rs` — Tauri bridge shape.
- `apps/desktop-tauri/src/components/MenuCard.tsx` — expanded tray card rendering.
- `apps/desktop-tauri/src/types/bridge.ts` — TypeScript contract.

## Product rules

1. The provider-reported meter is authoritative. Local token/session logs are contextual activity data, never a substitute for a subscription limit.
2. Show every reported window separately: five-hour, weekly, monthly, model-specific, and named extra capacity.
3. An absent known window is a state, not a percentage:
   - `tracked` — provider reports a real meter;
   - `not currently enforced` — provider returns a valid snapshot but omits that known meter;
   - `unavailable` — no trustworthy reading is available.
4. Never infer a user entitlement or remaining allowance from plan names, spend, or local token totals.
5. Reset/change alerts need fresh provider evidence, source/account isolation, and de-duplication. Do not alert on normal expected resets.

## Next implementation order

### 1. Cursor allowances — `SOU-124`

Inspect `rust/src/providers/cursor/api.rs` and map provider-reported values into named windows:

- base monthly allowance and renewal;
- extra usage or included/free promotional capacity;
- separately reported overage/cost data.

Only render a value when Cursor returns it. Use `extra_rate_windows` for measurable extra pools and `inactive_rate_windows` when a previously known meter is absent in an otherwise-valid response. Add representative JSON fixtures before touching UI.

### 2. Capacity-event observer — `SOU-125`

Build a small local persistent observation store, separate from provider fetchers. Its comparison key must include provider, account identity, source, and named window.

Recognize only confirmed events:

- a reset materially earlier than the previous `resets_at`;
- a significant reset-time shift;
- window lifted/restored;
- newly granted named extra allowance.

Require a second consistent fresh read unless the provider explicitly supplies the transition. Reuse the existing `NotificationManager` only after the detector is independently testable.

### 3. Notifications — `SOU-126`

Add a separate preference for unexpected-capacity events. Update the Windows toast AUMID from the inherited CodexBar identity to Ceiling as part of this work. Notifications must name the provider/window and explain the change, but never announce routine resets.

### 4. Ceiling presentation — `SOU-127`

Keep the taskbar-adjacent capacity strip compact; use the tray flyout for full detail.

- strip: the constraining window plus a brief lifted/not-enforced state;
- flyout: every measured or inactive named window;
- visually distinguish live, cached/stale, error, and not-enforced states.

The current float bar is already a transparent, always-on-top, non-activating Windows window. Windows 11 does not support the legacy third-party taskbar-toolbar model, so do not try to inject into the taskbar itself.

## Linear tracking

- [SOU-122 — Model explicit limit-window enforcement states](https://linear.app/southforge-ai/issue/SOU-122/model-explicit-limit-window-enforcement-states) — in progress; initial data path is committed.
- [SOU-123 — Codex five-hour, weekly, and lifted-window state](https://linear.app/southforge-ai/issue/SOU-123/track-codex-five-hour-weekly-and-lifted-window-state)
- [SOU-124 — Cursor monthly, extra usage, and promotional capacity](https://linear.app/southforge-ai/issue/SOU-124/track-cursor-monthly-allowance-extra-usage-and-promotional-capacity)
- [SOU-125 — Unexpected resets and capacity changes](https://linear.app/southforge-ai/issue/SOU-125/detect-unexpected-resets-and-capacity-changes)
- [SOU-126 — Unexpected-capacity notifications](https://linear.app/southforge-ai/issue/SOU-126/send-useful-unexpected-capacity-notifications)
- [SOU-127 — Multi-window tray and capacity strip](https://linear.app/southforge-ai/issue/SOU-127/build-the-ceiling-multi-window-tray-and-capacity-strip)

## Verification

From the repository root:

```powershell
pnpm --dir apps/desktop-tauri install --frozen-lockfile
pnpm --dir apps/desktop-tauri test
cargo test --manifest-path apps/desktop-tauri/src-tauri/Cargo.toml
cargo test -p codexbar
```

The focused regression command for the implemented Codex behavior is:

```powershell
cargo test -p codexbar preserves_a_lifted_five_hour_window_when_only_weekly_is_reported
```

Use `pnpm --dir apps/desktop-tauri tauri:dev` for the Windows shell. Preserve the MIT license and the upstream attribution; do not rename internal `codexbar` crate identifiers as part of feature work.
