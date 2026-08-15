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

- One glance: provider mark + remaining/used %. Cursor headlines its account-wide
  **Total** meter; other providers use the constraining window.
- State chip only when not live: `stale` | `error`.
- Live pills omit the chip.
- Optional reset countdown stays secondary, not a second percentage.

## Flyout overview anatomy (plan-status cards)

```
┌──────────────────────────────────────────────┐
│ [logo]  Cursor                    Pro        │
│         62% left · Monthly        resets 12d │
│         ████████░░                           │
│         Auto 90%  ████████████░░  (if hot)   │
└──────────────────────────────────────────────┘
```

- Overview cards lead with **primary plan pool** (not Auto alone).
- Optional **companion** lane only when hot (≥ ~70% used, or hotter than the pool).
- Clicking a card opens detail — it does not toggle which meter is shown.
- Provider switcher uses brand-tinted icons + status dots; bar = constraining %.

## Flyout detail anatomy

```
┌──────────────────────────────────────┐
│ [logo] Cursor   you@…        Pro     │
│ web · updated 2m ago                 │
│──────────────────────────────────────│
│ Monthly     ████████░░  62%   12d    │
│ Auto        ████░░░░░░  38%   12d    │
│ API         ██░░░░░░░░  12%   12d    │
│ Promotional ░░░░░░░░░░   0%   12d    │
│ 5-hour · Not currently enforced      │  ← inactive text row
│──────────────────────────────────────│
│ token history / limits (detail only) │
└──────────────────────────────────────┘
```

Detail lists **every** measured window plus inactive rows. Do not truncate to two metrics.

## State chip rules

| State | Strip | Flyout |
|---|---|---|
| Live | Normal pill, no chip | Quiet freshness text |
| Cached / stale | Muted pill + `stale` chip | Dim metrics + stale label |
| Error | Crit outline + `error` chip | Error line, no fake bars |
| Not enforced | No chip from inactive windows; the active window headlines and normal/stale/error freshness still controls the pill | Quiet named text row via `inactiveRateWindows` (`state: "notEnforced"`) |
| Unavailable | Same quiet treatment, in a caution (amber) tone; the window was tracked but dropped out of a successful response | Named row via `inactiveRateWindows` (`state: "unavailable"`), labeled "Unavailable" |

`inactiveRateWindows[].state` carries the explicit enforcement state: `notEnforced` (provider reported no active limit) vs `unavailable` (a first-class provider's window vanished from an otherwise-successful response, detected by the cross-snapshot enforcement tracker). Never invent `0%` or `100%` for either — surface the named state instead.

## Promo signals

Temporary provider-reported promotions use `promoSignals`:

| Kind | Strip | Overview |
|---|---|---|
| `boost` | Soft cyan edge + chip | Hidden; detail surfaces only |
| `inclusion` | Hidden | Hidden; detail surfaces only |

Account identity is also hidden from overview cards. Provider settings and the
Accounts section remain the intentional places for email/source details.

Never invent promos from marketing copy. Claude omelette / Cursor bonus pools only.

## Motion contract

The floating bar is persistent desktop furniture, so its resting state is static.
Do not add ambient shimmer, breathing, glow loops, or continuously moving gradients.

- Hover: a one-pixel lift and slightly clearer glass edge.
- Scheduled reset: one calm cyan sweep, then briefly show the replenished value.
- Confirmed surprise reset or restored capacity: a slightly brighter cyan sweep plus
  a short success halo around only the affected provider segment.
- Capacity loss or error: one restrained amber/red edge pulse; never loop.
- Windows toast and bar animation come from the same confirmed capacity event.
- Confirmed events cover scheduled and surprise resets, significant reset-time
  moves, lifted/restored windows, and newly reported extra allowance.
- Cursor On-demand is billed spend. It never emits capacity events or OS
  alerts. Do not invent a Promotional meter — Cursor does not report a grant
  to meter bonus consumption against.
- The dedicated **Reset and Capacity Alerts** preference controls OS alerts;
  turning it off does not disable the in-bar event treatment.
- Respect the global animation preference and `prefers-reduced-motion`.

Reset animation must consume the authoritative capacity-event observer output from
SOU-125. Do not infer resets independently in React from a single percentage drop.
Legacy threshold and session-transition alerts likewise require two consecutive
provider readings before notifying.

## Native controls

Three parts of a form field are painted by the engine, not by our CSS: the
number-input spinner arrows, the text caret, and the selection highlight.

- **Spinner arrows and caret** are pinned on the controls themselves —
  `color-scheme` and `caret-color` on `.select`, `.number-input`, `.text-input`,
  and `textarea`, not only on the root element. `color-scheme` is what the
  engine draws the arrows from, and the app theme can differ from the Windows
  theme, so inheriting it risks a light spinner on a dark field.
- **Selection** is one document-wide `::selection` rule, not a per-control pin.
  That is deliberate: a selected run should look the same in a form field, in
  About copy, and in the setup command block.
- Selection is a **solid** accent fill with its own ink (`--selection-ink`),
  never a translucent wash. The number that matters is the selection against
  the *unselected* field, not the text against the selection: an accent at 38%
  over a white field is about 1.7:1 and reads as no change at all. The shipped
  pair measures 5.98:1 dark and 4.56:1 light against the field, with the glyph
  colour changing in both.
- Dropdowns, checkboxes, sliders, and scrollbars already carry per-theme styling.

Once `color-scheme` is pinned, the engine-drawn parts follow the **app** theme.
The Windows theme still needs checking because it drives surrounding chrome and
because a regression here shows up as the two disagreeing.

## Screenshot checklist

Light and dark, with at least one live Codex/Cursor account:

1. Tray overview showing multiple windows + one inactive row.
2. Tray provider detail with token history/limits if available.
3. Capacity strip with a live pill and a stale/error pill.
4. Settings / About showing Ceiling branding.
5. Settings form controls — dropdown, checkbox, number input with its spinner,
   slider, scrollbar, focus ring, and selected text inside a `.text-input` and a
   `.number-input`. Capture these on a light-themed **and** a dark-themed
   Windows for each app theme. The two are independent settings, and this pass
   is what catches the engine-drawn parts following the wrong one.
