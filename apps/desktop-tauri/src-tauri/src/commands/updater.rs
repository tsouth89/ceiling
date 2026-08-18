//! Updater lifecycle commands: check, download, apply, dismiss, and release-page linking.
//!
//! State transitions are mirrored through [`events::emit_update_state_changed`]
//! so the frontend can react without polling.

use std::sync::Mutex;

use tauri::Manager;

use super::open_url_in_browser;
use crate::events;
use crate::state::{AppState, UpdateState, UpdateStatePayload};
use codexbar::updater::{UpdateCheckError, UpdateInfo};

#[tauri::command]
pub fn get_update_state(state: tauri::State<'_, Mutex<AppState>>) -> UpdateStatePayload {
    state
        .lock()
        .map(|guard| guard.update_payload())
        .unwrap_or_else(|_| UpdateState::default().to_payload())
}

#[tauri::command]
pub async fn check_for_updates(
    app: tauri::AppHandle,
    state: tauri::State<'_, Mutex<AppState>>,
) -> Result<UpdateStatePayload, String> {
    // Guard: skip if already checking, downloading, or ready to install.
    // Ready keeps installer_path; a second check that found "still latest"
    // used to map to Idle and hide Install & Restart (SBS-931 leftover).
    {
        let mut guard = state.lock().map_err(|e| e.to_string())?;
        if should_skip_update_check(&guard.update_state) {
            return Ok(guard.update_payload());
        }
        guard.update_state = UpdateState::Checking;
        guard.update_info = None;
        guard.installer_path = None;
    }

    let checking_payload = {
        let guard = state.lock().map_err(|e| e.to_string())?;
        guard.update_payload()
    };
    events::emit_update_state_changed(&app, &checking_payload);

    let settings = codexbar::settings::Settings::load();

    let result = tokio::time::timeout(
        std::time::Duration::from_secs(15),
        codexbar::updater::check_for_updates_with_channel(settings.update_channel),
    )
    .await;

    let (new_state, new_info) = state_from_check_result(result);

    let payload = {
        let mut guard = state.lock().map_err(|e| e.to_string())?;
        guard.update_state = new_state;
        guard.update_info = new_info;
        guard.last_update_check_ms = Some(chrono::Utc::now().timestamp_millis());
        guard.update_payload()
    };
    events::emit_update_state_changed(&app, &payload);

    Ok(payload)
}

fn should_skip_update_check(state: &UpdateState) -> bool {
    matches!(
        state,
        UpdateState::Checking | UpdateState::Downloading(_) | UpdateState::Ready
    )
}

/// Map a completed check onto Idle / Available / Error.
///
/// Idle is only a successful "no newer release". Timeout, transport, HTTP,
/// and parse failures are Error so About cannot say the user is current.
fn state_from_check_result(
    result: Result<Result<Option<UpdateInfo>, UpdateCheckError>, tokio::time::error::Elapsed>,
) -> (UpdateState, Option<UpdateInfo>) {
    match result {
        Ok(Ok(Some(info))) => (UpdateState::Available(info.version.clone()), Some(info)),
        Ok(Ok(None)) => (UpdateState::Idle, None),
        Ok(Err(error)) => (UpdateState::Error(error.user_message()), None),
        Err(_) => (
            UpdateState::Error(codexbar::locale::get_text(
                codexbar::locale::current_language(),
                codexbar::locale::LocaleKey::UpdateErrorTimedOut,
            )),
            None,
        ),
    }
}

#[tauri::command]
pub async fn download_update(
    app: tauri::AppHandle,
    state: tauri::State<'_, Mutex<AppState>>,
) -> Result<UpdateStatePayload, String> {
    let info = match update_info_for_download(&state)? {
        DownloadStart::Ready(info) => info,
        DownloadStart::AlreadyDownloading(payload) => return Ok(payload),
    };

    if !info.supports_auto_download() {
        return Err(
            "This update does not support automatic download. Open the release page instead."
                .to_string(),
        );
    }

    let initial_payload = set_downloading_state(&state)?;
    events::emit_update_state_changed(&app, &initial_payload);
    spawn_download_task(app.clone(), info);

    Ok(initial_payload)
}

enum DownloadStart {
    Ready(UpdateInfo),
    AlreadyDownloading(UpdateStatePayload),
}

fn update_info_for_download(
    state: &tauri::State<'_, Mutex<AppState>>,
) -> Result<DownloadStart, String> {
    let guard = state.lock().map_err(|e| e.to_string())?;
    match &guard.update_state {
        UpdateState::Available(_) | UpdateState::Error(_) => {}
        UpdateState::Downloading(_) => {
            return Ok(DownloadStart::AlreadyDownloading(guard.update_payload()));
        }
        _ => return Err("No update available to download".to_string()),
    }
    guard
        .update_info
        .clone()
        .map(DownloadStart::Ready)
        .ok_or("No update information available".to_string())
}

fn set_downloading_state(
    state: &tauri::State<'_, Mutex<AppState>>,
) -> Result<UpdateStatePayload, String> {
    let mut guard = state.lock().map_err(|e| e.to_string())?;
    guard.update_state = UpdateState::Downloading(0.0);
    Ok(guard.update_payload())
}

fn spawn_download_task(app_handle: tauri::AppHandle, info: UpdateInfo) {
    tokio::spawn(async move {
        let (tx, rx) = tokio::sync::watch::channel(codexbar::updater::UpdateState::Available);
        let progress_handle = spawn_download_progress_task(app_handle.clone(), rx);
        let final_payload = run_download_task(app_handle.clone(), info, tx).await;

        events::emit_update_state_changed(&app_handle, &final_payload);
        progress_handle.abort();
    });
}

fn spawn_download_progress_task(
    app: tauri::AppHandle,
    mut rx: tokio::sync::watch::Receiver<codexbar::updater::UpdateState>,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        while rx.changed().await.is_ok() {
            let backend_state = rx.borrow().clone();
            if let codexbar::updater::UpdateState::Downloading(progress) = backend_state {
                emit_download_progress(&app, progress);
            }
        }
    })
}

fn emit_download_progress(app: &tauri::AppHandle, progress: f32) {
    let st = app.state::<Mutex<AppState>>();
    let payload = {
        let mut guard = st.lock().unwrap();
        guard.update_state = UpdateState::Downloading(progress);
        guard.update_payload()
    };
    events::emit_update_state_changed(app, &payload);
}

async fn run_download_task(
    app: tauri::AppHandle,
    info: UpdateInfo,
    tx: tokio::sync::watch::Sender<codexbar::updater::UpdateState>,
) -> UpdateStatePayload {
    let download_handle =
        tokio::spawn(async move { codexbar::updater::download_update(&info, tx).await });

    match download_handle.await {
        Ok(Ok(path)) => finish_download(&app, UpdateState::Ready, Some(path)),
        Ok(Err(error)) => finish_download(&app, UpdateState::Error(error), None),
        Err(join_err) => finish_download(
            &app,
            UpdateState::Error(format!("Download task failed: {join_err}")),
            None,
        ),
    }
}

fn finish_download(
    app: &tauri::AppHandle,
    update_state: UpdateState,
    installer_path: Option<std::path::PathBuf>,
) -> UpdateStatePayload {
    let st = app.state::<Mutex<AppState>>();
    let mut guard = st.lock().unwrap();
    guard.update_state = update_state;
    guard.installer_path = installer_path;
    guard.update_payload()
}

#[tauri::command]
pub fn apply_update(
    app: tauri::AppHandle,
    state: tauri::State<'_, Mutex<AppState>>,
) -> Result<(), String> {
    apply_ready_update(&app, &state)
}

pub(crate) fn apply_ready_update(
    app: &tauri::AppHandle,
    state: &Mutex<AppState>,
) -> Result<(), String> {
    apply_ready_installer(state).inspect_err(|_| {
        if let Ok(guard) = state.lock() {
            events::emit_update_state_changed(app, &guard.update_payload());
        }
    })
}

fn apply_ready_installer(state: &Mutex<AppState>) -> Result<(), String> {
    let result = (|| {
        let (path, expected_sha256) = {
            let guard = state.lock().map_err(|e| e.to_string())?;
            let path = guard
                .installer_path
                .clone()
                .ok_or("No downloaded update available to apply")?;
            let expected_sha256 = guard
                .update_info
                .as_ref()
                .and_then(|info| info.expected_sha256.clone())
                .ok_or("Missing SHA256 digest for downloaded update")?;
            (path, expected_sha256)
        };
        codexbar::updater::verify_installer_hash(&path, &expected_sha256)?;
        codexbar::updater::apply_update(&path)
    })();

    if let Err(error) = &result {
        let _ = record_apply_failure(state, error);
    }
    result
}

fn record_apply_failure(
    state: &Mutex<AppState>,
    error: &str,
) -> Result<UpdateStatePayload, String> {
    let mut guard = state.lock().map_err(|e| e.to_string())?;
    guard.update_state = UpdateState::Error(error.to_string());
    guard.installer_path = None;
    Ok(guard.update_payload())
}

#[tauri::command]
pub fn dismiss_update(
    app: tauri::AppHandle,
    state: tauri::State<'_, Mutex<AppState>>,
) -> Result<UpdateStatePayload, String> {
    let payload = {
        let mut guard = state.lock().map_err(|e| e.to_string())?;
        guard.update_state = UpdateState::Idle;
        guard.update_info = None;
        guard.installer_path = None;
        guard.update_payload()
    };
    events::emit_update_state_changed(&app, &payload);
    Ok(payload)
}

#[tauri::command]
pub fn open_release_page(state: tauri::State<'_, Mutex<AppState>>) -> Result<(), String> {
    let url = {
        let guard = state.lock().map_err(|e| e.to_string())?;
        guard
            .update_info
            .as_ref()
            .map(|info| info.release_url.clone())
            .ok_or("No update information available")?
    };
    open_url_in_browser(&url)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[cfg(target_os = "windows")]
    use crate::state::AppState;
    use crate::state::UpdateState;
    use codexbar::updater::{UpdateCheckError, UpdateDelivery, UpdateInfo};

    fn sample_info(version: &str) -> UpdateInfo {
        UpdateInfo {
            version: version.to_string(),
            download_url: "https://example.com/Ceiling-Setup.exe".to_string(),
            expected_sha256: Some("a".repeat(64)),
            release_url: "https://example.com/release".to_string(),
            release_notes: String::new(),
            delivery: UpdateDelivery::Installer,
        }
    }

    /// SBS-931: GitHub/network/parse failures must not become Idle.
    #[test]
    fn failed_github_check_is_error_not_idle() {
        for error in [
            UpdateCheckError::Network,
            UpdateCheckError::Http { status: 403 },
            UpdateCheckError::Parse,
            UpdateCheckError::Client,
        ] {
            let (state, info) = state_from_check_result(Ok(Err(error.clone())));
            match state {
                UpdateState::Error(message) => {
                    assert_eq!(message, error.user_message());
                    assert!(
                        !message.to_ascii_lowercase().contains("up to date"),
                        "{message}"
                    );
                }
                other => panic!("expected Error, got {other:?} for {error:?}"),
            }
            assert!(info.is_none());
        }
    }

    /// Only a successful "no newer release" is Idle.
    #[test]
    fn successful_current_release_is_idle() {
        let (state, info) = state_from_check_result(Ok(Ok(None)));
        assert_eq!(state, UpdateState::Idle);
        assert!(info.is_none());
    }

    #[test]
    fn successful_newer_release_is_available() {
        let info = sample_info("v99.0.0");
        let (state, stored) = state_from_check_result(Ok(Ok(Some(info))));
        assert_eq!(state, UpdateState::Available("v99.0.0".to_string()));
        assert_eq!(
            stored.as_ref().map(|item| item.version.as_str()),
            Some("v99.0.0")
        );
    }

    #[test]
    fn ready_check_is_skipped_so_install_affordance_stays() {
        assert!(should_skip_update_check(&UpdateState::Ready));
        assert!(should_skip_update_check(&UpdateState::Checking));
        assert!(should_skip_update_check(&UpdateState::Downloading(0.5)));
        assert!(!should_skip_update_check(&UpdateState::Idle));
        assert!(!should_skip_update_check(&UpdateState::Error(
            "GitHub did not return a release.".to_string()
        )));
        assert!(!should_skip_update_check(&UpdateState::Available(
            "v99.0.0".to_string()
        )));
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn apply_time_signature_reject_does_not_leave_installer_ready() {
        let temp = tempfile::tempdir().expect("temp dir");
        let path = temp.path().join("Ceiling-99.0.0-Setup.exe");
        std::fs::write(&path, b"unsigned installer").expect("write installer");

        let state = Mutex::new(AppState::new());
        {
            let mut guard = state.lock().expect("lock");
            guard.update_state = UpdateState::Ready;
            guard.installer_path = Some(path.clone());
            guard.update_info = Some(UpdateInfo {
                version: "v99.0.0".to_string(),
                download_url: "https://example.com/Ceiling-99.0.0-Setup.exe".to_string(),
                // sha256("unsigned installer")
                expected_sha256: Some(
                    "d27a9d9762922fc761ed69b30eeaf45b03b16931d0f2b2c21c5f02c3dbb1690b".to_string(),
                ),
                release_url: "https://example.com/release".to_string(),
                release_notes: String::new(),
                delivery: UpdateDelivery::Installer,
            });
        }

        let error = apply_ready_installer(&state).unwrap_err();
        assert!(
            error.contains("unsigned")
                || error.contains("invalid or untrusted")
                || error.contains("downloaded file was removed"),
            "{error}"
        );
        assert!(!path.exists(), "rejected installer must be deleted");

        let guard = state.lock().expect("lock");
        match &guard.update_state {
            UpdateState::Error(message) => {
                assert_eq!(message, &error);
            }
            other => panic!("expected Error, got {other:?}"),
        }
        assert!(
            guard.installer_path.is_none(),
            "a deleted installer must not remain applyable"
        );
        drop(guard);

        let retry_error = apply_ready_installer(&state).unwrap_err();
        assert!(
            retry_error.contains("No downloaded update available to apply"),
            "{retry_error}"
        );
        assert!(matches!(
            state.lock().expect("lock").update_state,
            UpdateState::Error(_)
        ));
    }
}
