## Highlights

### Your usage bars now show where you *should* be

Ceiling could already tell whether you were on course to run out early. It just wasn't on the thing you actually look at.

Every weekly and monthly bar — in the Overview and in a provider's detail view — now carries a marker for where usage should be at this point in the window. One rule, everywhere: **the marker is where the bar's edge should be right now.**

- Edge sitting at the marker → you're on pace.
- Edge past it → you're ahead of budget, and the overspend fills in as a striped band so you can see *how far*, not just *which side*.
- Bars set to show remaining capacity mirror the marker, so it means the same thing either way.

It's worked out from elapsed time against the window's own length, which means it needs nothing from the provider and shows up on every long window at once, instead of only the single window a pace prediction was calculated for.

Five-hour session bars are deliberately left plain. Nobody spends a session evenly, so a marker there would drift across the bar all afternoon and tell you nothing.

### Cursor on-demand spend is visible where you look

1.5.23 added the dollars behind Cursor's on-demand lane. The main window was filtering that lane out by name, so the number never made it to the screen most people use.

The **On-demand** row now appears in a provider's detail view with its spend beside it — `$0.00 of $1.00`. On-demand is the only Cursor lane that bills real money, which makes it the row worth showing.

## Installers

- **Ceiling-1.5.24-Setup.exe** - standard installer
- **Ceiling-1.5.24-portable.exe** - portable
- **Ceiling-1.5.24-Store-Setup.exe** - Microsoft Store package (WebView2 bundled)

Portable builds show alerts as banners but do not keep them in the notification center. That requires the Start Menu shortcut installed by the standard or Store build.

---

**Full Changelog**: https://github.com/tsouth89/ceiling/compare/v1.5.23...v1.5.24
