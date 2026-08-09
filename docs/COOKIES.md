# Manual Cookie Setup

Some providers authenticate through their website (Claude, Cursor, Kimi, and similar). Modern browsers protect those sessions with mechanisms such as Chromium App-Bound Encryption, so Ceiling does not scan browser cookie databases. You explicitly copy and save only the cookie header you want Ceiling to use.

## Cookie source defaults

- **Cursor** defaults to **Automatic** — Ceiling reads the signed-in Cursor IDE session, so a cookie is usually unnecessary.
- **Every other cookie-based provider** defaults to **Manual**. You paste the cookie header once and Ceiling stores it encrypted.

Change a provider's source in **Settings → Providers → provider detail → Browser Cookies**.

## Copy a cookie header

1. Open the provider's website in your browser (e.g. `claude.ai`) and make sure you are logged in.
2. Open DevTools (F12) → **Network** tab, refresh the page, and click any request to the provider.
3. Copy the `Cookie` header value from **Request Headers**.
4. In Ceiling → **Settings → Providers → provider detail → Browser Cookies**, paste the value.

Manual cookies are saved to the `ManualCookies` store and reused across restarts, not held only in memory. On **Windows** (the shipped app) they are encrypted with DPAPI and locked to your user with a user-only file ACL, under `%APPDATA%\Ceiling`. On other platforms — the Linux CLI build of the shared crate — they are written with owner-only (`0600`) file permissions rather than encrypted.

## Troubleshooting

- If the provider reports that authentication is required, repeat the steps above and copy the complete `Cookie` request-header value from a successful authenticated request.
- Browser sessions expire. If usage stops refreshing later, replace the saved value with a fresh cookie header.
- Never send a cookie header to support, paste it into an issue, or include it in diagnostics.
