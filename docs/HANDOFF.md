# Ceiling handoff

Use this as the implementation starting point. When it disagrees with the app, trust the active Tauri sources in `apps/desktop-tauri` and shared Rust sources in `rust/src`.

## Current state

- Repository: `C:\projects\personal\ceiling`
- Trunk: protected `main`; create short-lived `codex/...` branches for new work.
- Public release: [Ceiling 0.43.2](https://github.com/tsouth89/ceiling/releases/tag/v0.43.2).
- Website: [ceiling.win](https://ceiling.win).
- Tracker: [Ceiling in Linear](https://linear.app/southforge-ai/project/ceiling-6615aa2c9e6b).

Ceiling is a local-first Windows companion for AI-subscription capacity. Its focused providers are Codex/ChatGPT, Claude, Cursor, Gemini, and GitHub Copilot. The app is built on the mature Win-CodexBar Tauri/Rust foundation while keeping Ceiling's Windows UI, release identity, and product behavior distinct.

## Implemented product contract

- Separate provider-reported windows for session, weekly, monthly, model-specific, and named extra capacity.
- Explicit inactive/not-enforced states instead of fabricated zero-use or unlimited meters.
- Overview, Activity, Accounts, and Charts dashboard surfaces.
- A compact taskbar-adjacent floating bar plus detailed tray/dashboard views.
- Persistent local quota history and processed-token summaries from local logs.
- Credential discovery and setup paths for the five focused providers.
- A persistent capacity-event observer keyed by provider, account, source, and semantic window identity.
- Startup re-baselining, confirmation for surprising changes, alert de-duplication, and a toast burst circuit breaker.
- Windows notifications only for confirmed scheduled or surprise resets, plus opt-in high-usage pace warnings.
- First-party website analytics and a private `/admin` dashboard backed by PostHog and GitHub metrics.

## Product rules

1. Provider-reported meters are authoritative. Local logs are contextual activity data, never a substitute for a subscription limit.
2. Keep providers, accounts, sources, and named windows isolated from one another.
3. Treat an absent known window as a state: tracked, not currently enforced, or unavailable.
4. Never infer entitlements, remaining allowance, or subscription spend from plan names or local token totals.
5. Re-baseline after startup. Do not replay changes that happened while Ceiling was closed.
6. Require fresh, consistent evidence for surprising changes. Notify only for resets and intentional high-usage warnings.
7. Prefer no notification over an ambiguous or duplicate notification.

## Next coherent work

1. Prepare the next patch release so the post-0.43.2 notification and chart fixes reach installed users.
2. Add deliberate multi-monitor support with one managed floating bar per selected display, not unlimited windows.
3. Make floating-bar appearance configurable through a small set of polished built-in themes before considering arbitrary user themes.
4. Continue provider-specific reliability work with deterministic fixtures whenever an upstream response changes.

## Relevant implementation areas

- `apps/desktop-tauri/src-tauri/src/capacity_events.rs` — reset/capacity observation and confirmation.
- `apps/desktop-tauri/src-tauri/src/commands/providers.rs` — refresh integration and notification eligibility.
- `apps/desktop-tauri/src-tauri/src/usage_history.rs` — local quota history.
- `apps/desktop-tauri/src-tauri/src/floatbar/` — Windows floating-bar lifecycle and positioning.
- `apps/desktop-tauri/src/surfaces/` — dashboard surfaces.
- `rust/src/providers/` — provider-specific fetch, auth, and parsing logic.
- `rust/src/notifications.rs` — Windows notification manager and burst protection.
- `worker.mjs` and `site/` — public site, analytics capture, and private dashboard.

## Verification

From the repository root:

```powershell
node --test worker.test.mjs
pnpm --dir apps/desktop-tauri install --frozen-lockfile
pnpm --dir apps/desktop-tauri test
pnpm --dir apps/desktop-tauri run build
cargo fmt --all --check
cargo test --manifest-path rust/Cargo.toml
cargo test --manifest-path apps/desktop-tauri/src-tauri/Cargo.toml
cargo clippy --manifest-path rust/Cargo.toml --all-targets -- -D warnings
cargo clippy --manifest-path apps/desktop-tauri/src-tauri/Cargo.toml --all-targets -- -D warnings
```

Use `pnpm --dir apps/desktop-tauri tauri:dev` for Windows behavior. Preserve the MIT license and upstream attribution, and do not rename the internal `codexbar` crate as part of feature work.
