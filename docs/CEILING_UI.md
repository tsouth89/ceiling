# Ceiling UI

Visual system for the tray flyout and taskbar-adjacent capacity strip (SOU-127).

## Direction

- **Theme:** follow Windows light/dark (`auto`). Light mica is the design baseline; dark is a paired token set.
- **Material:** CSS mica/glass (`backdrop-filter` + translucent tint). Native DWM mica is out of scope.
- **Type:** Windows-first — `Segoe UI Variable`, `Segoe UI`, system-ui.
- **Accent:** Ceiling cyan (`#26b5ce` family) on slate neutrals. No purple dashboard, no cream/terracotta, no glow effects.
- **Brand:** user-visible chrome says **Ceiling**. Internal `codexbar` crate IDs stay unchanged.

## Tokens

| Token | Light | Dark | Role |
|---|---|---|---|
| `--ceiling-accent` | `#1a9bb5` | `#26b5ce` | Brand / focus |
| `--ceiling-glass-bg` | `rgba(245, 247, 250, 0.78)` | `rgba(28, 32, 38, 0.72)` | Panel fill |
| `--ceiling-glass-border` | `rgba(15, 23, 42, 0.08)` | `rgba(255, 255, 255, 0.08)` | Hairline edge |
| `--ceiling-glass-blur` | `18px` | `18px` | Backdrop blur |
| `--text-primary` | `#0f172a` | `#f1f5f9` | Primary copy |
| `--text-secondary` | `#64748b` | `#94a3b8` | Meta / freshness |
| `--state-stale` | `#b45309` | `#f59e0b` | Cached / stale |
| `--state-error` | `#dc2626` | `#f87171` | Failed read |
| `--state-lifted` | `#0f766e` | `#2dd4bf` | Not enforced |
| `--radius-panel` | `12px` | `12px` | Flyout / cards |
| `--radius-pill` | `8px` | `8px` | Strip pills |

Usage bars keep calm slate→cyan progression; warn/crit stay amber/red without neon glow.

## Strip pill anatomy

```
┌─────────────────────────────┐
│ [icon]  42% left   [stale]  │
│         5-hour              │  ← constraining window label (quiet)
└─────────────────────────────┘
```

- One glance: provider mark + **constraining** remaining/used %.
- State chip only when not live: `stale` | `error` | `lifted`.
- Live pills omit the chip.
- Optional reset countdown stays secondary, not a second percentage.

## Flyout card anatomy

```
┌──────────────────────────────────────┐
│ Cursor          you@…        Pro     │
│ web · updated 2m ago                 │
│──────────────────────────────────────│
│ Monthly     ████████░░  62%   12d    │
│ Auto        ████░░░░░░  38%   12d    │
│ API         ██░░░░░░░░  12%   12d    │
│ Promotional ░░░░░░░░░░   0%   12d    │
│ 5-hour · Not currently enforced      │  ← inactive text row
│──────────────────────────────────────│
│ cost / charts (detail only)          │
└──────────────────────────────────────┘
```

Overview and detail both list **every** measured window plus inactive rows. Do not truncate to two metrics.

## State chip rules

| State | Strip | Flyout |
|---|---|---|
| Live | Normal pill, no chip | Quiet freshness text |
| Cached / stale | Muted pill + `stale` chip | Dim metrics + stale label |
| Error | Crit outline + `error` chip | Error line, no fake bars |
| Not enforced | Hollow / dashed `lifted` chip | Text row via `inactiveRateWindows` |

Never invent `0%` or `100%` for an inactive window.

## Screenshot checklist

Light and dark, with at least one live Codex/Cursor account:

1. Tray overview showing multiple windows + one inactive row.
2. Tray provider detail with cost/charts if available.
3. Capacity strip with a live pill and a stale/error/lifted pill.
4. Settings / About showing Ceiling branding.
