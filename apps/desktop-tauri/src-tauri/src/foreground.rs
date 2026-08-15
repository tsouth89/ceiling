//! Local foreground-app → provider mapping for floatbar selection modes.
//!
//! Detection stays on this machine. It never calls a provider API. A missing
//! match keeps the last known provider so an unrelated app does not blank
//! the bar.

mod matching;

use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use serde::Serialize;
use tauri::{AppHandle, Emitter};

pub use matching::match_foreground_provider;

pub const FOREGROUND_PROVIDER_CHANGED_EVENT: &str = "foreground-provider-changed";

const POLL_INTERVAL: Duration = Duration::from_millis(750);

static WATCH_GENERATION: AtomicU64 = AtomicU64::new(0);
static LAST_ACTIVE: Mutex<Option<String>> = Mutex::new(None);

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ForegroundProviderSnapshot {
    pub provider_id: Option<String>,
    pub last_active_provider_id: Option<String>,
    pub exe: Option<String>,
    pub title: Option<String>,
}

pub fn last_active_provider() -> Option<String> {
    LAST_ACTIVE.lock().ok().and_then(|guard| guard.clone())
}

pub fn should_watch_foreground(settings: &codexbar::settings::Settings) -> bool {
    settings.float_bar_enabled
        && settings.float_bar_foreground_detection
        && matches!(
            settings.float_bar_selection_mode.as_str(),
            "active" | "activePlusCritical"
        )
}

pub fn apply_watch(app: &AppHandle, settings: &codexbar::settings::Settings) {
    let generation = WATCH_GENERATION.fetch_add(1, Ordering::SeqCst) + 1;
    if !should_watch_foreground(settings) {
        return;
    }
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        let mut interval = tokio::time::interval(POLL_INTERVAL);
        loop {
            interval.tick().await;
            if WATCH_GENERATION.load(Ordering::SeqCst) != generation {
                break;
            }
            tick(&app);
        }
    });
}

#[tauri::command]
pub fn get_foreground_provider() -> ForegroundProviderSnapshot {
    snapshot_now()
}

fn tick(app: &AppHandle) {
    let snapshot = snapshot_now();
    let _ = app.emit(FOREGROUND_PROVIDER_CHANGED_EVENT, &snapshot);
}

fn snapshot_now() -> ForegroundProviderSnapshot {
    let observed = read_foreground();
    let matched = observed
        .as_ref()
        .and_then(|(exe, title)| match_foreground_provider(exe, title).map(str::to_string));
    if let Some(provider_id) = matched.as_ref() {
        if let Ok(mut guard) = LAST_ACTIVE.lock() {
            *guard = Some(provider_id.clone());
        }
    }
    let last_active = last_active_provider();
    ForegroundProviderSnapshot {
        provider_id: matched,
        last_active_provider_id: last_active,
        exe: observed.as_ref().map(|(exe, _)| exe.clone()),
        title: observed.as_ref().map(|(_, title)| title.clone()),
    }
}

#[cfg(windows)]
fn read_foreground() -> Option<(String, String)> {
    use windows::Win32::Foundation::{CloseHandle, MAX_PATH};
    use windows::Win32::System::Threading::{
        OpenProcess, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION,
        QueryFullProcessImageNameW,
    };
    use windows::Win32::UI::WindowsAndMessaging::{
        GetForegroundWindow, GetWindowTextW, GetWindowThreadProcessId,
    };
    use windows::core::PWSTR;

    let hwnd = unsafe { GetForegroundWindow() };
    if hwnd.0.is_null() {
        return None;
    }

    let mut pid = 0_u32;
    unsafe { GetWindowThreadProcessId(hwnd, Some(&mut pid)) };
    if pid == 0 {
        return None;
    }

    let process = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) }.ok()?;
    let exe = (|| {
        let mut buf = [0u16; MAX_PATH as usize];
        let mut size = buf.len() as u32;
        unsafe {
            QueryFullProcessImageNameW(
                process,
                PROCESS_NAME_WIN32,
                PWSTR(buf.as_mut_ptr()),
                &mut size,
            )
        }
        .ok()?;
        Some(String::from_utf16_lossy(&buf[..size as usize]))
    })();
    unsafe {
        let _ = CloseHandle(process);
    }
    let exe = exe?;

    let mut title_buf = [0u16; 512];
    let title_len = unsafe { GetWindowTextW(hwnd, &mut title_buf) };
    let title = if title_len > 0 {
        String::from_utf16_lossy(&title_buf[..title_len as usize])
    } else {
        String::new()
    };

    Some((exe, title))
}

#[cfg(not(windows))]
fn read_foreground() -> Option<(String, String)> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn should_watch_only_when_floatbar_uses_an_active_mode() {
        let mut settings = codexbar::settings::Settings::default();
        settings.float_bar_enabled = true;
        settings.float_bar_foreground_detection = true;
        settings.float_bar_selection_mode = "pinned".into();
        assert!(!should_watch_foreground(&settings));

        settings.float_bar_selection_mode = "active".into();
        assert!(should_watch_foreground(&settings));

        settings.float_bar_foreground_detection = false;
        assert!(!should_watch_foreground(&settings));

        settings.float_bar_foreground_detection = true;
        settings.float_bar_enabled = false;
        assert!(!should_watch_foreground(&settings));
    }
}
