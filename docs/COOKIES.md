# Browser Cookie Extraction

Win-CodexBar can automatically extract browser cookies for providers that use web authentication (Claude, Cursor, Kimi, etc.).

## Supported Browsers

| Browser | Encryption | Status |
|---------|-----------|--------|
| Chrome | DPAPI + AES-256-GCM | ✅ Automatic |
| Edge | DPAPI + AES-256-GCM | ✅ Automatic |
| Brave | DPAPI + AES-256-GCM | ✅ Automatic |
| Firefox | Unencrypted SQLite | ✅ Automatic |

## How It Works

1. CodexBar reads the browser's cookie database from its standard location
2. For Chromium-based browsers, cookies are encrypted with Windows DPAPI — CodexBar decrypts them using the current user's credentials
3. Only cookies for enabled providers are extracted (e.g., `claude.ai`, `cursor.com`)
4. Cookies are stored in-memory and refreshed on each provider poll

## Setting Up Cookie Import

1. Open **Settings** → **Providers** tab
2. Select the provider you want to configure
3. In the provider detail pane, find the **Browser Cookies** section
4. Choose your browser from the dropdown and click **Import**

## Manual Cookies

If automatic extraction fails (e.g., browser is locked, profile is encrypted, or running in WSL):

1. Open your browser and navigate to the provider's website (e.g., `claude.ai`)
2. Open DevTools (F12) → **Network** tab
3. Refresh the page and click any request to the provider
4. Copy the `Cookie` header value from **Request Headers**
5. In CodexBar Settings → provider detail → **Browser Cookies**, paste the value

## Troubleshooting

- **"Cookie decryption failed"**: Close the browser and retry — some browsers lock the cookie database while running
- **Empty cookies**: Make sure you're logged into the provider's web interface in that browser
- **WSL**: Chromium DPAPI cookies cannot be decrypted from WSL. Use manual cookies or CLI-based auth instead
