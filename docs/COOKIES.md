# Browser Cookie Extraction

Some providers authenticate through their website (Claude, Cursor, Kimi, and similar). Ceiling can read those cookies from your browser, but modern Windows locks most Chromium cookies behind App-Bound Encryption, so **manual cookies are the default and most reliable path** for those providers.

## Cookie source defaults

- **Cursor** defaults to **Automatic** — Ceiling reads the signed-in Cursor IDE session, so a cookie import is usually unnecessary.
- **Every other cookie-based provider** defaults to **Manual**. You paste the cookie header once and Ceiling stores it encrypted.

Change a provider's source in **Settings → Providers → provider detail → Browser Cookies**.

## Automatic import and App-Bound Encryption

| Browser | Reality on current Windows |
|---------|----------------------------|
| Chrome / Edge / Brave | App-Bound Encryption (Chrome/Edge 127+, `v20` cookies) blocks automatic import for most profiles. When it does, Ceiling reports "App-Bound Encryption is blocking automatic browser import" and you should switch that provider to manual cookies. |
| Firefox | Cookies are stored unencrypted, so automatic import works. |

When automatic import does succeed, Ceiling reads the browser's cookie database, decrypts Chromium cookies with the current user's Windows DPAPI key, and extracts only the cookies for enabled providers (e.g. `claude.ai`, `cursor.com`).

## Manual cookies (recommended for Chromium providers)

1. Open the provider's website in your browser (e.g. `claude.ai`) and make sure you are logged in.
2. Open DevTools (F12) → **Network** tab, refresh the page, and click any request to the provider.
3. Copy the `Cookie` header value from **Request Headers**.
4. In Ceiling → **Settings → Providers → provider detail → Browser Cookies**, paste the value.

Manual cookies are saved to the `ManualCookies` store and reused across restarts, not held only in memory. On **Windows** (the shipped app) they are encrypted with DPAPI and locked to your user with a user-only file ACL, under `%APPDATA%\Ceiling`. On other platforms — the Linux CLI build of the shared crate — they are written with owner-only (`0600`) file permissions rather than encrypted.

## Troubleshooting

- **"App-Bound Encryption is blocking automatic browser import"**: expected on current Chrome/Edge/Brave. Switch that provider to manual cookies.
- **"Cookie decryption failed" or empty cookies**: close the browser (it can lock the cookie database while running) and confirm you are logged into the provider's website in that browser.
- **WSL**: Chromium DPAPI cookies cannot be decrypted from WSL. Use manual cookies or CLI-based provider auth instead.
