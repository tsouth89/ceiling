//! Global keyboard shortcut registration for opening the primary window.
//!
//! Reads the persisted `global_shortcut` setting (e.g. `"Ctrl+Shift+U"`)
//! and registers it through the Tauri global-shortcut plugin. The shortcut
//! opens/focuses the native PopOut dashboard via the surface state machine.

use tauri::AppHandle;
use tauri_plugin_global_shortcut::{Code, GlobalShortcutExt, Modifiers, Shortcut, ShortcutState};

use crate::shell;

const KEY_ALIASES: &[(&str, Code)] = &[
    ("a", Code::KeyA),
    ("b", Code::KeyB),
    ("c", Code::KeyC),
    ("d", Code::KeyD),
    ("e", Code::KeyE),
    ("f", Code::KeyF),
    ("g", Code::KeyG),
    ("h", Code::KeyH),
    ("i", Code::KeyI),
    ("j", Code::KeyJ),
    ("k", Code::KeyK),
    ("l", Code::KeyL),
    ("m", Code::KeyM),
    ("n", Code::KeyN),
    ("o", Code::KeyO),
    ("p", Code::KeyP),
    ("q", Code::KeyQ),
    ("r", Code::KeyR),
    ("s", Code::KeyS),
    ("t", Code::KeyT),
    ("u", Code::KeyU),
    ("v", Code::KeyV),
    ("w", Code::KeyW),
    ("x", Code::KeyX),
    ("y", Code::KeyY),
    ("z", Code::KeyZ),
    ("0", Code::Digit0),
    ("1", Code::Digit1),
    ("2", Code::Digit2),
    ("3", Code::Digit3),
    ("4", Code::Digit4),
    ("5", Code::Digit5),
    ("6", Code::Digit6),
    ("7", Code::Digit7),
    ("8", Code::Digit8),
    ("9", Code::Digit9),
    ("f1", Code::F1),
    ("f2", Code::F2),
    ("f3", Code::F3),
    ("f4", Code::F4),
    ("f5", Code::F5),
    ("f6", Code::F6),
    ("f7", Code::F7),
    ("f8", Code::F8),
    ("f9", Code::F9),
    ("f10", Code::F10),
    ("f11", Code::F11),
    ("f12", Code::F12),
    ("space", Code::Space),
    ("enter", Code::Enter),
    ("return", Code::Enter),
    ("escape", Code::Escape),
    ("esc", Code::Escape),
    ("tab", Code::Tab),
];

/// Parse a settings shortcut string (e.g. `"Ctrl+Shift+U"`) into a Tauri `Shortcut`.
///
/// Public so that callers (e.g. the settings command) can validate a shortcut
/// string before persisting it.
pub fn parse_shortcut(s: &str) -> Option<Shortcut> {
    let mut mods = Modifiers::empty();
    let mut key_code: Option<Code> = None;

    for part in s.split('+').map(str::trim).filter(|part| !part.is_empty()) {
        if let Some(parsed_modifier) = parse_modifier(part) {
            mods |= parsed_modifier;
        } else if let Some(parsed_key) = parse_key(part) {
            key_code = Some(parsed_key);
        }
    }

    let key = key_code?;
    let m = if mods.is_empty() { None } else { Some(mods) };
    Some(Shortcut::new(m, key))
}

fn parse_modifier(token: &str) -> Option<Modifiers> {
    match token.to_ascii_lowercase().as_str() {
        "ctrl" | "control" => Some(Modifiers::CONTROL),
        "shift" => Some(Modifiers::SHIFT),
        "alt" => Some(Modifiers::ALT),
        "super" | "win" | "meta" => Some(Modifiers::SUPER),
        _ => None,
    }
}

fn parse_key(token: &str) -> Option<Code> {
    let normalized = token.to_ascii_lowercase();
    KEY_ALIASES
        .iter()
        .find_map(|(alias, code)| (*alias == normalized).then_some(*code))
}

/// Build the Tauri global-shortcut plugin. Individual shortcuts get focused
/// handlers when they are registered so the taskbar toggle never needs to
/// activate a webview just to hide the strip.
pub fn plugin() -> tauri::plugin::TauriPlugin<tauri::Wry> {
    tauri_plugin_global_shortcut::Builder::new().build()
}

#[derive(Clone, Copy)]
enum ShortcutAction {
    OpenDashboard,
    ToggleTaskbarStrip,
}

#[derive(Default)]
struct ShortcutPair {
    dashboard: Option<Shortcut>,
    taskbar_toggle: Option<Shortcut>,
}

fn configured_shortcuts(dashboard: &str, taskbar_toggle: &str) -> Result<ShortcutPair, String> {
    let parse_optional = |value: &str, label: &str| {
        if value.trim().is_empty() {
            Ok(None)
        } else {
            parse_shortcut(value)
                .map(Some)
                .ok_or_else(|| format!("Invalid {label} shortcut \"{value}\"."))
        }
    };

    let shortcuts = ShortcutPair {
        dashboard: parse_optional(dashboard, "dashboard")?,
        taskbar_toggle: parse_optional(taskbar_toggle, "taskbar strip")?,
    };

    if shortcuts
        .dashboard
        .as_ref()
        .zip(shortcuts.taskbar_toggle.as_ref())
        .is_some_and(|(dashboard, taskbar_toggle)| dashboard.id() == taskbar_toggle.id())
    {
        return Err("The dashboard and taskbar strip shortcuts must be different.".into());
    }

    Ok(shortcuts)
}

fn register_action(
    app: &AppHandle,
    shortcut: Shortcut,
    action: ShortcutAction,
) -> Result<(), String> {
    match action {
        ShortcutAction::OpenDashboard => {
            app.global_shortcut()
                .on_shortcut(shortcut, |app, _shortcut, event| {
                    if event.state == ShortcutState::Pressed {
                        let _ = shell::reopen_to_target(
                            app,
                            crate::surface::SurfaceMode::PopOut,
                            crate::surface_target::SurfaceTarget::Dashboard,
                            None,
                        );
                    }
                })
        }
        ShortcutAction::ToggleTaskbarStrip => {
            app.global_shortcut()
                .on_shortcut(shortcut, |app, _shortcut, event| {
                    if event.state == ShortcutState::Pressed {
                        crate::floatbar::toggle(app);
                    }
                })
        }
    }
    .map_err(|error| format!("Failed to register shortcut: {error}"))
}

fn unregister_pair(app: &AppHandle, shortcuts: &ShortcutPair) {
    for shortcut in [shortcuts.dashboard, shortcuts.taskbar_toggle]
        .into_iter()
        .flatten()
    {
        let _ = app.global_shortcut().unregister(shortcut);
    }
}

fn register_pair(app: &AppHandle, shortcuts: &ShortcutPair) -> Result<(), String> {
    if let Some(shortcut) = shortcuts.dashboard {
        register_action(app, shortcut, ShortcutAction::OpenDashboard)?;
    }
    if let Some(shortcut) = shortcuts.taskbar_toggle
        && let Err(error) = register_action(app, shortcut, ShortcutAction::ToggleTaskbarStrip)
    {
        if let Some(dashboard) = shortcuts.dashboard {
            let _ = app.global_shortcut().unregister(dashboard);
        }
        return Err(error);
    }
    Ok(())
}

/// Register the persisted global shortcuts from settings.
///
/// Call this in the Tauri `setup` closure after the plugin is initialised.
pub fn register(app: &AppHandle) {
    let settings = codexbar::settings::Settings::load();
    let shortcuts =
        match configured_shortcuts(&settings.global_shortcut, &settings.taskbar_toggle_shortcut) {
            Ok(shortcuts) => shortcuts,
            Err(error) => {
                tracing::warn!(%error, "Could not parse configured global shortcuts");
                return;
            }
        };

    if let Err(error) = register_pair(app, &shortcuts) {
        tracing::warn!(%error, "Could not register configured global shortcuts");
    }
}

/// Live-swap both persisted global shortcuts without leaving the app with a
/// half-registered pair when one of the new accelerators is unavailable.
pub fn reregister_shortcuts(
    app: &AppHandle,
    old_dashboard: &str,
    old_taskbar_toggle: &str,
    new_dashboard: &str,
    new_taskbar_toggle: &str,
) -> Result<(), String> {
    let next = configured_shortcuts(new_dashboard, new_taskbar_toggle)?;
    let previous =
        configured_shortcuts(old_dashboard, old_taskbar_toggle).unwrap_or_else(|error| {
            tracing::warn!(%error, "Ignoring invalid previously saved global shortcut");
            ShortcutPair::default()
        });

    unregister_pair(app, &previous);
    if let Err(error) = register_pair(app, &next) {
        unregister_pair(app, &next);
        if let Err(restore_error) = register_pair(app, &previous) {
            tracing::error!(%restore_error, "Could not restore previous global shortcuts");
        }
        return Err(error);
    }

    Ok(())
}

/// Verify that an accelerator is syntactically valid and can be reserved by
/// Windows before Settings asks the bridge to persist it. The probe is removed
/// immediately; `reregister_shortcuts` performs the real atomic swap.
pub fn validate_shortcut(app: &AppHandle, accelerator: &str) -> Result<(), String> {
    let shortcut = parse_shortcut(accelerator)
        .ok_or_else(|| format!("Invalid shortcut \"{accelerator}\". Use e.g. Ctrl+Shift+H."))?;
    app.global_shortcut()
        .register(shortcut)
        .map_err(|error| format!("Failed to register shortcut \"{accelerator}\": {error}"))?;
    let _ = app.global_shortcut().unregister(shortcut);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_ctrl_shift_u() {
        let s = parse_shortcut("Ctrl+Shift+U").unwrap();
        assert_eq!(s.key, Code::KeyU);
        assert!(s.mods.contains(Modifiers::CONTROL));
        assert!(s.mods.contains(Modifiers::SHIFT));
    }

    #[test]
    fn parse_alt_f1() {
        let s = parse_shortcut("Alt+F1").unwrap();
        assert_eq!(s.key, Code::F1);
        assert!(s.mods.contains(Modifiers::ALT));
    }

    #[test]
    fn parse_single_letter_no_mods() {
        let s = parse_shortcut("A").unwrap();
        assert_eq!(s.key, Code::KeyA);
        assert!(s.mods.is_empty());
    }

    #[test]
    fn parse_empty_returns_none() {
        assert!(parse_shortcut("").is_none());
    }

    #[test]
    fn configured_shortcuts_allow_an_unbound_taskbar_toggle() {
        let shortcuts = configured_shortcuts("Ctrl+Shift+U", "").unwrap();
        assert!(shortcuts.dashboard.is_some());
        assert!(shortcuts.taskbar_toggle.is_none());
    }

    #[test]
    fn configured_shortcuts_reject_duplicate_accelerators() {
        let result = configured_shortcuts("Ctrl+Shift+U", "Ctrl+Shift+U");
        assert!(result.is_err());
        assert!(result.err().unwrap().contains("must be different"));
    }

    #[test]
    fn parse_invalid_returns_none() {
        assert!(parse_shortcut("Ctrl+???").is_none());
    }

    #[test]
    fn parse_digit() {
        let s = parse_shortcut("Ctrl+5").unwrap();
        assert_eq!(s.key, Code::Digit5);
        assert!(s.mods.contains(Modifiers::CONTROL));
    }

    #[test]
    fn parse_super_key() {
        let s = parse_shortcut("Super+A").unwrap();
        assert_eq!(s.key, Code::KeyA);
        assert!(s.mods.contains(Modifiers::SUPER));
    }

    #[test]
    fn parse_win_alias() {
        let s = parse_shortcut("Win+Z").unwrap();
        assert_eq!(s.key, Code::KeyZ);
        assert!(s.mods.contains(Modifiers::SUPER));
    }

    #[test]
    fn parse_meta_alias() {
        let s = parse_shortcut("Meta+F12").unwrap();
        assert_eq!(s.key, Code::F12);
        assert!(s.mods.contains(Modifiers::SUPER));
    }

    #[test]
    fn parse_special_keys() {
        assert_eq!(parse_shortcut("Ctrl+Space").unwrap().key, Code::Space);
        assert_eq!(parse_shortcut("Alt+Enter").unwrap().key, Code::Enter);
        assert_eq!(parse_shortcut("Ctrl+Tab").unwrap().key, Code::Tab);
        assert_eq!(parse_shortcut("Ctrl+Escape").unwrap().key, Code::Escape);
    }

    #[test]
    fn parse_return_alias() {
        assert_eq!(parse_shortcut("Ctrl+Return").unwrap().key, Code::Enter);
    }

    #[test]
    fn parse_esc_alias() {
        assert_eq!(parse_shortcut("Ctrl+Esc").unwrap().key, Code::Escape);
    }

    #[test]
    fn parse_control_alias() {
        let s = parse_shortcut("Control+Shift+B").unwrap();
        assert!(s.mods.contains(Modifiers::CONTROL));
        assert!(s.mods.contains(Modifiers::SHIFT));
        assert_eq!(s.key, Code::KeyB);
    }

    #[test]
    fn parse_all_function_keys() {
        for i in 1..=12u8 {
            let input = format!("F{i}");
            let s = parse_shortcut(&input);
            assert!(s.is_some(), "F{i} should parse");
        }
    }

    #[test]
    fn parse_no_key_returns_none() {
        // Only modifiers, no actual key
        assert!(parse_shortcut("Ctrl+Shift").is_none());
    }

    #[test]
    fn parse_whitespace_tolerant() {
        let s = parse_shortcut("Ctrl + Shift + U").unwrap();
        assert_eq!(s.key, Code::KeyU);
        assert!(s.mods.contains(Modifiers::CONTROL));
        assert!(s.mods.contains(Modifiers::SHIFT));
    }
}
