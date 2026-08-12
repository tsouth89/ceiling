// Placeholder emitters for vertical slices — suppress dead-code until wired.
#![allow(dead_code)]

use serde::Serialize;
use tauri::{AppHandle, Emitter};

use crate::commands::ProviderUsageSnapshot;
use crate::proof_harness::ProofStatePayload;
use crate::state::UpdateStatePayload;
use crate::surface::SurfaceMode;
use crate::surface_target::SurfaceTarget;

// ── Event name constants ─────────────────────────────────────────────

pub const SURFACE_MODE_CHANGED: &str = "surface-mode-changed";
pub const PROVIDER_UPDATED: &str = "provider-updated";
pub const REFRESH_STARTED: &str = "refresh-started";
pub const REFRESH_COMPLETE: &str = "refresh-complete";
pub const UPDATE_STATE_CHANGED: &str = "update-state-changed";
pub const LOGIN_PHASE_CHANGED: &str = "login-phase-changed";
pub const PROOF_STATE_CHANGED: &str = "proof-state-changed";
pub const LOCALE_CHANGED: &str = "locale-changed";
pub const SETTINGS_CHANGED: &str = "settings-changed";
pub const CAPACITY_EVENT: &str = "capacity-event";
pub const TASKBAR_WIDGET_STATUS_CHANGED: &str = "taskbar-widget-status-changed";

// ── Payloads ─────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SurfaceModePayload {
    pub mode: &'static str,
    pub previous: &'static str,
    pub target: SurfaceTarget,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RefreshCompletePayload {
    pub provider_count: usize,
    pub error_count: usize,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RefreshStartedPayload {
    pub provider_ids: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LoginPhaseChangedPayload<'a> {
    pub provider_id: &'a str,
    pub phase: &'a str,
    /// The device-flow user code to display (e.g. GitHub's device
    /// authorization code), when the phase has one to show.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub code: Option<&'a str>,
    /// The verification URL the user must open to enter `code`, for when
    /// the app could not open it in a browser automatically.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub url: Option<&'a str>,
}

// ── Emit helpers ─────────────────────────────────────────────────────

pub fn emit_surface_mode_changed(
    app: &AppHandle,
    from: SurfaceMode,
    to: SurfaceMode,
    target: SurfaceTarget,
) {
    let _ = app.emit(
        SURFACE_MODE_CHANGED,
        SurfaceModePayload {
            mode: to.as_str(),
            previous: from.as_str(),
            target,
        },
    );
}

pub fn emit_provider_updated(app: &AppHandle, snapshot: &ProviderUsageSnapshot) {
    let mut snapshot = snapshot.clone();
    crate::commands::filter_hidden_codex_spark_rows(
        &mut snapshot,
        codexbar::settings::Settings::load().codex_spark_usage_visible(),
    );
    let _ = app.emit(PROVIDER_UPDATED, snapshot);
}

pub fn emit_refresh_started(app: &AppHandle, provider_ids: Vec<String>) {
    let _ = app.emit(REFRESH_STARTED, RefreshStartedPayload { provider_ids });
}

pub fn emit_refresh_complete(app: &AppHandle, provider_count: usize, error_count: usize) {
    let _ = app.emit(
        REFRESH_COMPLETE,
        RefreshCompletePayload {
            provider_count,
            error_count,
        },
    );
}

pub fn emit_update_state_changed(app: &AppHandle, payload: &UpdateStatePayload) {
    let _ = app.emit(UPDATE_STATE_CHANGED, payload);
}

pub fn emit_login_phase_changed(app: &AppHandle, provider_id: &str, phase: &str) {
    let _ = app.emit(
        LOGIN_PHASE_CHANGED,
        LoginPhaseChangedPayload {
            provider_id,
            phase,
            code: None,
            url: None,
        },
    );
}

/// Same as [`emit_login_phase_changed`], but also carries a device-flow user
/// code and the verification URL it must be entered at, for the frontend to
/// display (e.g. GitHub's device authorization code and
/// `https://github.com/login/device`) in case the browser did not open
/// automatically.
pub fn emit_login_phase_changed_with_code(
    app: &AppHandle,
    provider_id: &str,
    phase: &str,
    code: &str,
    url: &str,
) {
    let _ = app.emit(
        LOGIN_PHASE_CHANGED,
        LoginPhaseChangedPayload {
            provider_id,
            phase,
            code: Some(code),
            url: Some(url),
        },
    );
}

pub fn emit_proof_state_changed(app: &AppHandle, payload: &ProofStatePayload) {
    let _ = app.emit(PROOF_STATE_CHANGED, payload);
}

/// Broadcast to every window that persisted settings changed, so surfaces in
/// other windows (e.g. the PopOut dashboard) re-read settings and re-render —
/// the detached Settings window and the main window are separate webviews and
/// do not share React state. Payload-less; listeners re-fetch the snapshot.
pub fn emit_settings_changed(app: &AppHandle) {
    let _ = app.emit(SETTINGS_CHANGED, ());
}

/// Broadcast when the native taskbar widget's visibility status changes
/// (shown, or hidden and why), so the Settings row updates live instead of
/// polling. Payload-less; listeners re-fetch via `get_taskbar_widget_status`.
pub fn emit_taskbar_widget_status_changed(app: &AppHandle) {
    let _ = app.emit(TASKBAR_WIDGET_STATUS_CHANGED, ());
}

pub fn emit_capacity_event(
    app: &AppHandle,
    payload: &crate::capacity_events::CapacityEventPayload,
) {
    let _ = app.emit(CAPACITY_EVENT, payload);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn login_phase_payload_omits_code_and_url_when_absent() {
        let payload = LoginPhaseChangedPayload {
            provider_id: "copilot",
            phase: "requesting",
            code: None,
            url: None,
        };
        let json = serde_json::to_value(&payload).unwrap();
        assert!(json.get("code").is_none());
        assert!(json.get("url").is_none());
    }

    #[test]
    fn login_phase_payload_includes_code_and_url_when_present() {
        let payload = LoginPhaseChangedPayload {
            provider_id: "copilot",
            phase: "waitingBrowser",
            code: Some("ABCD-1234"),
            url: Some("https://github.com/login/device"),
        };
        let json = serde_json::to_value(&payload).unwrap();
        assert_eq!(json["code"], "ABCD-1234");
        assert_eq!(json["url"], "https://github.com/login/device");
    }
}
