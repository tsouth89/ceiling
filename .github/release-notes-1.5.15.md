## Highlights

### Antigravity quotas match Settings

Ceiling now reads Antigravity's shared model-group pools (`RetrieveUserQuotaSummary`) instead of only per-model `remainingFraction` on `GetUserStatus`. Weekly and five-hour group limits line up with Antigravity Settings → Models.

### Claude sign-in vs Charts

Missing Claude CLI sign-in no longer collapses into a one-line OAuth message that hides other sources. Errors point at `claude` login for live capacity and note that Charts can still show local session spend without it.

### Custom date range on Estimated API value

Charts → Estimated API value gains a **Custom** period with From/To pickers (inclusive local calendar days, up to 366 days), on top of Today / Yesterday / 30 days.

## Fixes

- Antigravity: prefer group weekly / five-hour quota summary over per-model zeros (#163, #167)
- Claude: clearer multi-source capacity errors; charts-without-sign-in note (#165, #166)
- Charts: custom inclusive date range for Estimated API value (#164, #168)
