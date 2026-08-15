//! Pure mapping from a local process path + window title to a provider id.

const SELF_EXES: &[&str] = &[
    "ceiling",
    "codexbar",
    "codexbar-desktop-tauri",
    "codexbar-desktop",
];

const TERMINAL_EXES: &[&str] = &[
    "windowsterminal",
    "windows terminal",
    "cmd",
    "powershell",
    "pwsh",
    "conhost",
    "wezterm",
    "wezterm-gui",
    "alacritty",
    "mintty",
    "tabby",
    "fluent-terminal",
    "windowsterminal.exe",
];

const DESKTOP_APPS: &[(&str, &str)] = &[
    ("cursor", "cursor"),
    ("cursor-nightly", "cursor"),
    ("claude", "claude"),
    ("codex", "codex"),
    ("chatgpt", "openaiapi"),
    ("grok", "grok"),
    ("gemini", "gemini"),
    ("antigravity", "antigravity"),
    ("windsurf", "windsurf"),
    ("zed", "zed"),
    ("warp", "warp"),
    ("copilot", "copilot"),
    ("factory", "factory"),
    ("droid", "factory"),
    ("kiro", "kiro"),
    ("augment", "augment"),
    ("opencode", "opencode"),
    ("opencode-go", "opencode-go"),
];

const TITLE_HINTS: &[(&str, &str)] = &[
    ("cursor", "cursor"),
    ("claude", "claude"),
    ("codex", "codex"),
    ("grok", "grok"),
    ("gemini", "gemini"),
    ("copilot", "copilot"),
    ("windsurf", "windsurf"),
    ("opencode", "opencode"),
];

/// Map a foreground process to a Ceiling provider id.
///
/// `exe` may be a full path. `title` is the top-level window title. Returns
/// `None` for Ceiling itself, unknown apps, and terminals with no agent hint.
pub fn match_foreground_provider(exe: &str, title: &str) -> Option<&'static str> {
    let exe_name = exe_stem(exe);
    if exe_name.is_empty() {
        return None;
    }
    if SELF_EXES.contains(&exe_name.as_str()) {
        return None;
    }
    if let Some(provider) = match_desktop_app(&exe_name) {
        return Some(provider);
    }
    if is_terminal(&exe_name) {
        return match_title_hint(title);
    }
    None
}

fn exe_stem(path: &str) -> String {
    let file = path.rsplit(['/', '\\']).next().unwrap_or(path).trim();
    let without_ext = file
        .strip_suffix(".exe")
        .or_else(|| file.strip_suffix(".EXE"))
        .unwrap_or(file);
    without_ext.to_ascii_lowercase()
}

fn match_desktop_app(exe_name: &str) -> Option<&'static str> {
    DESKTOP_APPS
        .iter()
        .find(|(name, _)| *name == exe_name)
        .map(|(_, provider)| *provider)
}

fn is_terminal(exe_name: &str) -> bool {
    TERMINAL_EXES.contains(&exe_name)
}

/// Match a terminal tab title to an agent.
///
/// Only the command side of the title is considered (text before ` — `,
/// ` | `, or ` - `). Path-only titles and later path tokens such as a
/// folder named `cursor` do not count. If several agent names appear in
/// that prefix, the leftmost one wins.
fn match_title_hint(title: &str) -> Option<&'static str> {
    let prefix = command_prefix(title);
    if looks_like_path(prefix) {
        return None;
    }
    title_tokens(prefix).find_map(provider_for_title_token)
}

fn command_prefix(title: &str) -> &str {
    for sep in [" — ", " – ", " | ", " - "] {
        if let Some((left, _)) = title.split_once(sep) {
            return left.trim();
        }
    }
    title.trim()
}

fn looks_like_path(text: &str) -> bool {
    let text = text.trim();
    text.starts_with('~')
        || text.starts_with('/')
        || text.starts_with('\\')
        || text
            .as_bytes()
            .get(..2)
            .is_some_and(|bytes| bytes[0].is_ascii_alphabetic() && bytes[1] == b':')
}

fn title_tokens(title: &str) -> impl Iterator<Item = &str> {
    title
        .split(|ch: char| !ch.is_ascii_alphanumeric() && ch != '-' && ch != '_')
        .filter(|token| !token.is_empty())
}

fn provider_for_title_token(token: &str) -> Option<&'static str> {
    let lowered = token.to_ascii_lowercase();
    TITLE_HINTS
        .iter()
        .find(|(hint, _)| *hint == lowered)
        .map(|(_, provider)| *provider)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn desktop_apps_map_by_exe_name() {
        assert_eq!(
            match_foreground_provider(r"C:\Users\a\AppData\Local\Programs\cursor\Cursor.exe", ""),
            Some("cursor")
        );
        assert_eq!(
            match_foreground_provider("Claude.exe", "Claude"),
            Some("claude")
        );
        assert_eq!(match_foreground_provider("codex.exe", ""), Some("codex"));
        assert_eq!(match_foreground_provider("Warp.exe", ""), Some("warp"));
        assert_eq!(match_foreground_provider("Zed.exe", ""), Some("zed"));
        assert_eq!(
            match_foreground_provider("Windsurf.exe", ""),
            Some("windsurf")
        );
    }

    #[test]
    fn vs_code_is_not_assumed_to_be_copilot() {
        assert_eq!(
            match_foreground_provider("Code.exe", "main.rs — ceiling"),
            None
        );
        assert_eq!(
            match_foreground_provider("Code - Insiders.exe", "copilot"),
            None
        );
    }

    #[test]
    fn ceiling_windows_do_not_replace_the_active_provider() {
        assert_eq!(match_foreground_provider("Ceiling.exe", "Ceiling"), None);
        assert_eq!(
            match_foreground_provider("codexbar-desktop-tauri.exe", "Settings"),
            None
        );
    }

    #[test]
    fn unrelated_apps_do_not_match() {
        assert_eq!(
            match_foreground_provider(r"C:\Program Files\Google\Chrome\Application\chrome.exe", ""),
            None
        );
        assert_eq!(match_foreground_provider("explorer.exe", ""), None);
    }

    #[test]
    fn terminal_titles_map_known_agents() {
        assert_eq!(
            match_foreground_provider("WindowsTerminal.exe", "claude — ~/src/app"),
            Some("claude")
        );
        assert_eq!(
            match_foreground_provider("pwsh.exe", "codex"),
            Some("codex")
        );
        assert_eq!(
            match_foreground_provider("WindowsTerminal.exe", "PowerShell"),
            None
        );
    }

    #[test]
    fn title_hints_do_not_match_partial_words() {
        assert_eq!(
            match_foreground_provider("WindowsTerminal.exe", "encoding"),
            None
        );
        assert_eq!(
            match_foreground_provider("WindowsTerminal.exe", "my-codex-notes"),
            None
        );
    }

    #[test]
    fn terminal_titles_ignore_path_tokens_and_prefer_the_command() {
        assert_eq!(
            match_foreground_provider("WindowsTerminal.exe", "claude — ~/src/cursor"),
            Some("claude")
        );
        assert_eq!(
            match_foreground_provider("WindowsTerminal.exe", r"C:\Users\a\projects\cursor"),
            None
        );
        assert_eq!(
            match_foreground_provider("WindowsTerminal.exe", "~/src/codex"),
            None
        );
        assert_eq!(
            match_foreground_provider("WindowsTerminal.exe", "claude cursor"),
            Some("claude")
        );
    }
}
