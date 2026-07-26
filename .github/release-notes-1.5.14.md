## Highlights

### Weekly window dollars match priority/fast

After 1.5.13, Codex local cost uses priority/fast (2x) when `service_tier` is set. A disk chart cache could still show the old standard-rate dollars on the Weekly window card while the API-value ring used a fresh scan. 1.5.14 drops that cache so both surfaces agree.

## Fixes

- Chart data cache version 6 → 7 (invalidates stale localUsage costs after pricing change)
- Regression: reset windows price at 2x under fast cost speed
