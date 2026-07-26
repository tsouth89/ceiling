## Highlights

### Local cost matches ccusage speed tiers

Ceiling now prices Codex the same way default ccusage does when `service_tier` is set in `~/.codex/config.toml`. Priority/fast is 2x list rates. Override with `codexbar cost --codex-speed auto|standard|fast`. JSON includes `cost.codex_speed` and `cost.codex_service_tier` so A/B audits are unambiguous.

### Claude day windows match Codex

`--days N` for Claude is an inclusive local calendar window (same as Codex), not a rolling UTC duration that used to label N+1 days.

## Fixes

- Codex always used standard list rates even when config was priority/fast, so side-by-side with default ccusage looked about half.
- Claude cost period labels and cutoffs aligned with Codex / ccusage calendar semantics.
