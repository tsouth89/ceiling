## Highlights

### Notification history actually works now

1.5.17 fixed Windows discarding Ceiling's alerts instead of keeping them in the notification center. On a clean install that fix did nothing.

Windows only keeps notifications for an app it can identify, and it reads that identity from a Start Menu shortcut. The installer was not putting it there, so a fresh 1.5.17 install still lost every alert seconds after it appeared. It only looked correct on machines that happened to have an older shortcut left over.

**If you are on 1.5.17, install this one to pick up the corrected shortcut.**

### Updates no longer loop forever

If you ran Ceiling from anywhere other than the installed folder, a local build or an install from older packaging, updating went in a circle: the installer succeeded somewhere else, the copy you were running stayed on the old version, and the same update was offered again on the next check, with nothing on screen explaining why.

Ceiling now notices that case and tells you which copy is running and which one the installer updates, instead of quietly repeating itself.

Downloaded installers are also cleaned up once they are superseded. They were kept forever; one machine had accumulated 305 MB of them.

### Hide Personal Info hides personal info

The setting only ever reached the Accounts list. The Overview, taskbar flyout, plan cards, activity timeline and provider detail all carried on showing the full address.

All of them mask now, including account labels, which are filled in with your address by default and were a second copy of it. The domain stays visible so you can still tell your own accounts apart.

## Also fixed

- Switching between accounts on the Providers page works. Clicking the second account did nothing at all, because the chosen account was never sent to the backend.
- Enabled providers sort to the top of the Providers list, with the rest by name. A provider you had configured could otherwise sit far down among ones you do not use.

## Installers

- **Ceiling-1.5.18-Setup.exe** - standard installer
- **Ceiling-1.5.18-portable.exe** - portable
- **Ceiling-1.5.18-Store-Setup.exe** - Microsoft Store package (WebView2 bundled)

Portable builds show alerts as banners but do not keep them in the notification center. That needs a Start Menu shortcut, and Ceiling will not add one to a machine where you chose a portable build.

---

**Full Changelog**: https://github.com/tsouth89/ceiling/compare/v1.5.17...v1.5.18
