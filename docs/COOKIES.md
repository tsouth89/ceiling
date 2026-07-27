# Browser Cookie Extraction

Some providers authenticate through their website (Claude, Cursor, OpenCode Go, Kimi, and similar). Ceiling can try to read those cookies from browsers installed on this PC.

## Cookie source defaults

- **Cursor** defaults to **Automatic** — Ceiling reads the signed-in Cursor IDE session, so a cookie import is usually unnecessary.
- **Every other cookie-based provider** defaults to **Manual**. You paste the cookie header once and Ceiling stores it encrypted. You can still switch to **Automatic** if you prefer live browser reads.

Change a provider's source in **Settings → Providers → provider detail → Browser Cookies**.

## Automatic import: what actually works on Windows

Ceiling scans browsers in this order:

1. Firefox family (Firefox, Developer Edition, LibreWolf, Floorp, Waterfox)
2. Microsoft Edge / Edge Beta / Edge Dev
3. Brave
4. Google Chrome / Chrome Beta / Canary
5. Arc, Chromium

For each browser it walks every profile (from `Local State` plus folder scan), copies the cookie database with shared file locks (including SQLite `-wal` / `-shm` sidecars), and extracts only cookies for the provider domain.

| Browser | Reality on current Windows |
|---------|----------------------------|
| Chrome / Edge / Brave / Arc | **App-Bound Encryption** (Chrome/Edge 127+, cookie prefix `v20`) blocks third-party decrypt. Ceiling will report ABE clearly instead of a silent "no cookies". |
| Firefox family | Cookies are stored unencrypted. Automatic import works when you are signed in to the provider site **in Firefox**. |

When automatic import succeeds, Ceiling extracts only cookies for the enabled provider domains (for example `claude.ai`, `opencode.ai`, `cursor.com`).

### Practical recommendation

- Prefer **Firefox** for any provider you want fully automatic.
- Or stay on Chromium browsers and use **manual Cookie header paste** (most reliable for Chrome/Edge/Brave).
- For **Cursor**, keep Automatic and rely on the IDE session when possible.

## Manual cookies (recommended for Chromium providers)

1. Open the provider's website in your browser (e.g. `claude.ai` or `opencode.ai`) and make sure you are logged in.
2. Open DevTools (F12) → **Network** tab, refresh the page, and click any request to the provider.
3. Copy the `Cookie` header value from **Request Headers**.
4. In Ceiling → **Settings → Providers → provider detail → Browser Cookies**, paste the value.

Manual cookies are saved to the `ManualCookies` store and reused across restarts, not held only in memory. On **Windows** (the shipped app) they are encrypted with DPAPI and locked to your user with a user-only file ACL, under `%APPDATA%\Ceiling`. On other platforms — the Linux CLI build of the shared crate — they are written with owner-only (`0600`) file permissions rather than encrypted.

## Troubleshooting

- **"App-Bound Encryption is blocking automatic browser import"**: expected on current Chrome/Edge/Brave. Use Firefox for automatic, or paste manual cookies.
- **"No cookies for … in scanned browsers"**: you are not signed in to that site in a readable browser (often Firefox is empty while Chromium has the session but ABE blocks it).
- **Database locked**: close the browser fully and retry; Ceiling uses shared-mode copy but exclusive locks can still win.
- **WSL**: Chromium DPAPI cookies cannot be decrypted from WSL. Use manual cookies or CLI-based provider auth instead.
