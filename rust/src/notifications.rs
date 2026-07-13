//! System notifications for CodexBar
//!
//! Provides Windows toast notifications for usage alerts

#![allow(dead_code)]

use crate::core::ProviderId;
use crate::core::{RateWindow, UsagePace};
use crate::locale::{self, LocaleKey};
use crate::settings::Settings;
use crate::sound::{AlertSound, play_alert};
use chrono::{DateTime, Utc};

/// Notification types
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum NotificationType {
    /// Usage is approaching limit (high threshold)
    HighUsage,
    /// Usage is critical (critical threshold)
    CriticalUsage,
    /// Usage limit exhausted
    Exhausted,
    /// Provider status issue
    StatusIssue,
    /// Session quota depleted (at 100% usage)
    SessionDepleted,
    /// Session quota restored (back from 100%)
    SessionRestored,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PredictiveWarningWindow {
    Session,
    Weekly,
}

impl PredictiveWarningWindow {
    fn localized_label(self, language: crate::settings::Language) -> String {
        locale::get_text(
            language,
            match self {
                Self::Session => LocaleKey::ProviderSession,
                Self::Weekly => LocaleKey::ProviderWeekly,
            },
        )
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct PredictiveResetWindow {
    window_minutes: Option<u32>,
    resets_at: DateTime<Utc>,
}

impl PredictiveResetWindow {
    fn belongs_to_same_cycle(&self, other: &Self) -> bool {
        if self.window_minutes != other.window_minutes {
            return false;
        }
        let tolerance_secs = self
            .window_minutes
            .map(|minutes| i64::from(minutes) * 30)
            .unwrap_or(300)
            .max(300);
        (self.resets_at - other.resets_at).num_seconds().abs() < tolerance_secs
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct PredictiveWarningKey {
    provider: ProviderId,
    identity: String,
    window: PredictiveWarningWindow,
    reset: PredictiveResetWindow,
}

impl NotificationType {
    pub fn title(&self) -> &'static str {
        match self {
            NotificationType::HighUsage => "High Usage Warning",
            NotificationType::CriticalUsage => "Critical Usage Alert",
            NotificationType::Exhausted => "Usage Limit Reached",
            NotificationType::StatusIssue => "Provider Status Issue",
            NotificationType::SessionDepleted => "Session Depleted",
            NotificationType::SessionRestored => "Session Restored",
        }
    }

    pub fn icon(&self) -> &'static str {
        match self {
            NotificationType::HighUsage => "⚠️",
            NotificationType::CriticalUsage => "🔴",
            NotificationType::Exhausted => "🚫",
            NotificationType::StatusIssue => "⚡",
            NotificationType::SessionDepleted => "🔴",
            NotificationType::SessionRestored => "✅",
        }
    }
}

/// How long after process start to suppress toasts for already-high usage.
/// We still record alert state so pre-existing high usage does not flood after
/// the quiet period ends.
const STARTUP_QUIET_PERIOD: std::time::Duration = std::time::Duration::from_secs(20);

/// Drop below this offset under the high threshold before re-arming alerts.
/// Avoids flicker when a meter hovers around the boundary across refreshes.
const REARM_HYSTERESIS: f64 = 3.0;

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct ThresholdAlertKey {
    provider: ProviderId,
    window: String,
    kind: NotificationType,
}

/// Notification manager
pub struct NotificationManager {
    /// Track which threshold notifications have been sent to avoid spam.
    /// Keyed by provider + window so session/weekly cannot clear each other.
    sent_notifications: std::collections::HashSet<ThresholdAlertKey>,
    /// Track previous session percent for depleted/restored transitions.
    /// Missing entry means we have not observed this provider yet (baseline).
    previous_session_percent: std::collections::HashMap<ProviderId, f64>,
    predictive_warning_keys: std::collections::HashSet<PredictiveWarningKey>,
    started_at: std::time::Instant,
    #[cfg(test)]
    pub(crate) toasts_shown: usize,
}

impl NotificationManager {
    pub fn new() -> Self {
        Self::with_started_at(std::time::Instant::now())
    }

    fn with_started_at(started_at: std::time::Instant) -> Self {
        Self {
            sent_notifications: std::collections::HashSet::new(),
            previous_session_percent: std::collections::HashMap::new(),
            predictive_warning_keys: std::collections::HashSet::new(),
            started_at,
            #[cfg(test)]
            toasts_shown: 0,
        }
    }

    #[cfg(test)]
    fn new_past_quiet_period() -> Self {
        Self::with_started_at(
            std::time::Instant::now()
                .checked_sub(STARTUP_QUIET_PERIOD + std::time::Duration::from_secs(1))
                .unwrap_or_else(std::time::Instant::now),
        )
    }

    fn in_startup_quiet_period(&self) -> bool {
        self.started_at.elapsed() < STARTUP_QUIET_PERIOD
    }

    fn emit_toast(&mut self, title: &str, body: &str) {
        if self.in_startup_quiet_period() {
            tracing::debug!(
                "Suppressing toast during startup quiet period: {} — {}",
                title,
                body
            );
            return;
        }
        #[cfg(test)]
        {
            self.toasts_shown += 1;
        }
        self.show_toast(title, body);
    }

    pub fn record_predictive_observation(
        &mut self,
        enabled: bool,
        provider: ProviderId,
        identity: &str,
        window: PredictiveWarningWindow,
        rate_window: &RateWindow,
        pace: &UsagePace,
    ) -> bool {
        if !enabled {
            self.predictive_warning_keys
                .retain(|key| key.provider != provider);
            return false;
        }
        if !matches!(provider, ProviderId::Claude | ProviderId::Codex) || identity.is_empty() {
            return false;
        }
        let Some(resets_at) = rate_window.resets_at else {
            return false;
        };
        let key = PredictiveWarningKey {
            provider,
            identity: identity.to_string(),
            window,
            reset: PredictiveResetWindow {
                window_minutes: rate_window.window_minutes,
                resets_at,
            },
        };

        let warned_this_cycle = self.predictive_warning_keys.iter().any(|existing| {
            existing.provider == key.provider
                && existing.identity == key.identity
                && existing.window == key.window
                && existing.reset.belongs_to_same_cycle(&key.reset)
        });
        self.predictive_warning_keys.retain(|existing| {
            existing.provider != key.provider
                || existing.identity != key.identity
                || existing.window != key.window
        });

        if pace.will_last_to_reset {
            return false;
        }
        if !pace
            .eta_seconds
            .is_some_and(|eta| eta.is_finite() && eta > 0.0)
        {
            return false;
        }

        self.predictive_warning_keys.insert(key);
        !warned_this_cycle
    }

    pub fn set_predictive_warnings_enabled(&mut self, provider: ProviderId, enabled: bool) {
        if !enabled {
            self.predictive_warning_keys
                .retain(|key| key.provider != provider);
        }
    }

    pub fn check_predictive_pace(
        &mut self,
        provider: ProviderId,
        identity: &str,
        window: PredictiveWarningWindow,
        rate_window: &RateWindow,
        pace: &UsagePace,
        settings: &Settings,
    ) {
        if !self.record_predictive_observation(
            settings.show_notifications && settings.predictive_pace_warning_enabled,
            provider,
            identity,
            window,
            rate_window,
            pace,
        ) {
            return;
        }

        let eta = format_duration(pace.eta_seconds.unwrap_or_default());
        let provider_name = provider.display_name();
        let window_label = window.localized_label(settings.ui_language);
        let title = locale::format_locale(
            settings.ui_language,
            LocaleKey::PredictivePaceWarningTitle,
            &[provider_name, &window_label],
        );
        let body = locale::format_locale(
            settings.ui_language,
            LocaleKey::PredictivePaceWarningBody,
            &[&eta],
        );
        self.emit_toast(&title, &body);
        if !self.in_startup_quiet_period() {
            play_alert(AlertSound::Warning, settings);
        }
    }

    /// Check usage and send notifications if thresholds are crossed.
    ///
    /// Alerts are deduped per `(provider, window, severity)`. Dropping below the
    /// high threshold (with hysteresis) re-arms that window only — a quiet weekly
    /// meter must not clear a loud session alert (and vice versa).
    pub fn check_and_notify(
        &mut self,
        provider: ProviderId,
        window: &str,
        used_percent: f64,
        settings: &Settings,
    ) {
        if !settings.show_notifications {
            return;
        }

        let thresholds = settings.usage_thresholds(provider, window);
        let rearm_below = (thresholds.high - REARM_HYSTERESIS).max(0.0);

        // Session depletion/restoration is owned by `check_session_transition` so
        // we do not double-toast Exhausted + SessionDepleted for the same crossing.
        let notification_type = if window == "session" && used_percent >= 100.0 {
            None
        } else if used_percent >= 100.0 {
            Some(NotificationType::Exhausted)
        } else if used_percent >= thresholds.critical {
            Some(NotificationType::CriticalUsage)
        } else if used_percent >= thresholds.high {
            Some(NotificationType::HighUsage)
        } else if used_percent < rearm_below {
            self.clear_window_alerts(provider, window);
            None
        } else {
            None
        };

        if let Some(notif_type) = notification_type {
            let key = ThresholdAlertKey {
                provider,
                window: window.to_string(),
                kind: notif_type,
            };
            if !self.sent_notifications.contains(&key) {
                // Mark before emit so startup quiet still arms state.
                self.sent_notifications.insert(key);
                self.send_notification(provider, window, used_percent, notif_type, settings);
            }
        }
    }

    fn clear_window_alerts(&mut self, provider: ProviderId, window: &str) {
        self.sent_notifications
            .retain(|key| !(key.provider == provider && key.window == window));
    }

    /// Send a notification for a status issue
    pub fn notify_status_issue(
        &mut self,
        provider: ProviderId,
        description: &str,
        settings: &Settings,
    ) {
        if !settings.show_notifications {
            return;
        }
        let key = ThresholdAlertKey {
            provider,
            window: "status".to_string(),
            kind: NotificationType::StatusIssue,
        };
        if !self.sent_notifications.contains(&key) {
            self.sent_notifications.insert(key);
            self.send_status_notification(provider, description, settings);
        }
    }

    /// Clear status issue notification (when resolved)
    pub fn clear_status_issue(&mut self, provider: ProviderId) {
        self.sent_notifications
            .retain(|key| !(key.provider == provider && key.kind == NotificationType::StatusIssue));
    }

    /// Check session quota transitions (depleted/restored)
    /// Call this with each usage update to detect transitions
    pub fn check_session_transition(
        &mut self,
        provider: ProviderId,
        current_percent: f64,
        settings: &Settings,
    ) {
        if !settings.show_notifications {
            return;
        }

        const DEPLETED_THRESHOLD: f64 = 99.99; // Consider depleted at 99.99%+

        let Some(previous_percent) = self.previous_session_percent.get(&provider).copied() else {
            // First observation: baseline only. If already depleted, arm the
            // depleted flag quietly so a later restore can still notify.
            if current_percent >= DEPLETED_THRESHOLD {
                self.sent_notifications.insert(ThresholdAlertKey {
                    provider,
                    window: "session".to_string(),
                    kind: NotificationType::SessionDepleted,
                });
            }
            self.previous_session_percent
                .insert(provider, current_percent);
            return;
        };

        // Check for depleted transition: was not depleted, now is
        if previous_percent < DEPLETED_THRESHOLD && current_percent >= DEPLETED_THRESHOLD {
            let title = NotificationType::SessionDepleted.title();
            let body = format!(
                "{} session depleted. 0% left. Will notify when available again.",
                provider.display_name()
            );
            self.emit_toast(title, &body);
            if !self.in_startup_quiet_period() {
                play_alert(AlertSound::Error, settings);
            }
            self.sent_notifications.insert(ThresholdAlertKey {
                provider,
                window: "session".to_string(),
                kind: NotificationType::SessionDepleted,
            });
        }
        // Check for restored transition: was depleted, now is not
        else if previous_percent >= DEPLETED_THRESHOLD && current_percent < DEPLETED_THRESHOLD {
            let depleted_key = ThresholdAlertKey {
                provider,
                window: "session".to_string(),
                kind: NotificationType::SessionDepleted,
            };
            if self.sent_notifications.contains(&depleted_key) {
                let title = NotificationType::SessionRestored.title();
                let body = format!(
                    "{} session restored. Session quota is available again.",
                    provider.display_name()
                );
                self.emit_toast(title, &body);
                if !self.in_startup_quiet_period() {
                    play_alert(AlertSound::Success, settings);
                }
                self.sent_notifications.remove(&depleted_key);
            }
        }

        self.previous_session_percent
            .insert(provider, current_percent);
    }

    /// Send a Windows toast notification with sound
    fn send_notification(
        &mut self,
        provider: ProviderId,
        window: &str,
        used_percent: f64,
        notif_type: NotificationType,
        settings: &Settings,
    ) {
        let title = notif_type.title();
        let body = Self::notification_body(provider, window, used_percent, notif_type);
        self.emit_toast(title, &body);
        if !self.in_startup_quiet_period() {
            play_alert(Self::alert_sound_for(notif_type), settings);
        }
    }

    fn notification_body(
        provider: ProviderId,
        window: &str,
        used_percent: f64,
        notif_type: NotificationType,
    ) -> String {
        let provider_name = provider.display_name();
        let window_label = match window {
            "session" => "session",
            "weekly" => "weekly",
            other => other,
        };
        match notif_type {
            NotificationType::HighUsage => {
                format!("{provider_name} {window_label} at {used_percent:.0}% — approaching limit")
            }
            NotificationType::CriticalUsage => {
                format!("{provider_name} {window_label} at {used_percent:.0}% — critically high")
            }
            NotificationType::Exhausted => {
                format!("{provider_name} {window_label} exhausted ({used_percent:.0}%)")
            }
            NotificationType::StatusIssue => format!("{provider_name} is experiencing issues"),
            NotificationType::SessionDepleted => {
                format!("{provider_name} session depleted. 0% left.")
            }
            NotificationType::SessionRestored => {
                format!("{provider_name} session restored. Quota available again.")
            }
        }
    }

    fn alert_sound_for(notif_type: NotificationType) -> AlertSound {
        match notif_type {
            NotificationType::HighUsage => AlertSound::Warning,
            NotificationType::CriticalUsage => AlertSound::Critical,
            NotificationType::Exhausted
            | NotificationType::StatusIssue
            | NotificationType::SessionDepleted => AlertSound::Error,
            NotificationType::SessionRestored => AlertSound::Success,
        }
    }

    fn send_status_notification(
        &mut self,
        provider: ProviderId,
        description: &str,
        settings: &Settings,
    ) {
        let title = NotificationType::StatusIssue.title();
        let body = format!("{}: {}", provider.display_name(), description);
        self.emit_toast(title, &body);
        if !self.in_startup_quiet_period() {
            play_alert(AlertSound::Error, settings);
        }
    }

    #[cfg(target_os = "windows")]
    fn show_toast(&self, title: &str, body: &str) {
        use std::os::windows::process::CommandExt;
        use std::process::Command;
        use std::sync::Once;

        // Register our AUMID (App User Model ID) exactly once per process so that
        // CreateToastNotifier("CodexBar") finds a valid registration rather than
        // silently returning a null notifier.
        static AUMID_INIT: Once = Once::new();
        AUMID_INIT.call_once(ensure_aumid_registered);

        // Escape for XML content to prevent injection
        fn xml_escape(s: &str) -> String {
            s.replace('&', "&amp;")
                .replace('<', "&lt;")
                .replace('>', "&gt;")
                .replace('"', "&quot;")
                .replace('\'', "&apos;")
        }

        let safe_title = xml_escape(title);
        let safe_body = xml_escape(body);

        // Uses ToastGeneric (Win 10+) and wraps in try/catch so PowerShell exits
        // with code 1 on failure rather than swallowing the error silently.
        // Single-quoted here-string (@'...'@) prevents variable expansion of the
        // XML content by PowerShell.
        let script = format!(
            r#"try {{
    [Windows.UI.Notifications.ToastNotificationManager, Windows.UI.Notifications, ContentType = WindowsRuntime] | Out-Null
    [Windows.Data.Xml.Dom.XmlDocument, Windows.Data.Xml.Dom.XmlDocument, ContentType = WindowsRuntime] | Out-Null
    $template = @'
<toast><visual><binding template="ToastGeneric"><text>{}</text><text>{}</text></binding></visual></toast>
'@
    $xml = New-Object Windows.Data.Xml.Dom.XmlDocument
    $xml.LoadXml($template)
    $toast = [Windows.UI.Notifications.ToastNotification]::new($xml)
    $notifier = [Windows.UI.Notifications.ToastNotificationManager]::CreateToastNotifier("CodexBar")
    if ($null -eq $notifier) {{ throw "CreateToastNotifier returned null" }}
    $notifier.Show($toast)
}} catch {{
    [System.Console]::Error.WriteLine("CodexBar toast failed: $_")
    exit 1
}}"#,
            safe_title, safe_body
        );

        match Command::new("powershell")
            .args([
                "-NoProfile",
                "-ExecutionPolicy",
                "Bypass",
                "-Command",
                &script,
            ])
            .creation_flags(0x08000000) // CREATE_NO_WINDOW
            .spawn()
        {
            Ok(_) => tracing::debug!("Toast notification dispatched: {}", title),
            Err(e) => tracing::warn!("Failed to dispatch toast notification '{}': {}", title, e),
        }
    }

    #[cfg(not(target_os = "windows"))]
    fn show_toast(&self, title: &str, body: &str) {
        use std::process::Command;

        // Try notify-send first (works on most Linux distros including WSL with WSLg)
        if let Ok(output) = Command::new("notify-send")
            .args([
                "--app-name=CodexBar",
                "--icon=dialog-information",
                title,
                body,
            ])
            .output()
            && output.status.success()
        {
            tracing::debug!("Sent notification via notify-send: {}", title);
            return;
        }

        tracing::info!("Notification: {} - {}", title, body);
    }
}

fn format_duration(seconds: f64) -> String {
    let total_minutes = (seconds / 60.0).ceil().max(1.0) as i64;
    let days = total_minutes / 1440;
    let hours = (total_minutes % 1440) / 60;
    let minutes = total_minutes % 60;
    if days > 0 {
        format!("{days}d {hours}h")
    } else if hours > 0 {
        format!("{hours}h {minutes}m")
    } else {
        format!("{minutes}m")
    }
}

impl Default for NotificationManager {
    fn default() -> Self {
        Self::new()
    }
}

/// Register the CodexBar App User Model ID (AUMID) in the Windows registry so that
/// `CreateToastNotifier("CodexBar")` resolves to a valid notifier instead of returning
/// null.  Must be called at least once before the first toast.  Safe to call multiple
/// times (idempotent registry write).
#[cfg(target_os = "windows")]
fn ensure_aumid_registered() {
    use winreg::RegKey;
    use winreg::enums::*;

    let hkcu = RegKey::predef(HKEY_CURRENT_USER);
    // HKCU\SOFTWARE\Classes\AppUserModelId\<AUMID> is the documented path for
    // registering Win32 desktop app AUMIDs without a COM server or Start Menu shortcut.
    let result = hkcu
        .create_subkey(r"SOFTWARE\Classes\AppUserModelId\CodexBar")
        .and_then(|(key, _)| key.set_value("DisplayName", &"CodexBar"));

    match result {
        Ok(()) => tracing::debug!("CodexBar AUMID registered for Windows toast notifications"),
        Err(e) => tracing::warn!("Failed to register CodexBar AUMID: {}", e),
    }
}

/// Simple notification function for one-off notifications
pub fn show_notification(title: &str, body: &str) {
    let manager = NotificationManager::new();
    manager.show_toast(title, body);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::{PaceStage, RateWindow, UsagePace};
    use chrono::{DateTime, Duration, Utc};

    fn pace(will_last_to_reset: bool, eta_seconds: Option<f64>) -> UsagePace {
        UsagePace {
            stage: PaceStage::Ahead,
            delta_percent: 20.0,
            expected_used_percent: 40.0,
            actual_used_percent: 60.0,
            eta_seconds,
            will_last_to_reset,
        }
    }

    fn window(now: DateTime<Utc>, offset: Duration, minutes: u32) -> RateWindow {
        RateWindow::with_details(60.0, Some(minutes), Some(now + offset), None)
    }

    #[test]
    fn predictive_warning_notifies_once_until_recovery_then_rearms() {
        let now = DateTime::from_timestamp(1_800_000_000, 0).unwrap();
        let window = window(now, Duration::hours(3), 300);
        let risk = pace(false, Some(3600.0));
        let recovery = pace(true, None);
        let mut manager = NotificationManager::new();

        assert!(manager.record_predictive_observation(
            true,
            ProviderId::Claude,
            "oauth:person@example.com",
            PredictiveWarningWindow::Session,
            &window,
            &risk,
        ));
        assert!(!manager.record_predictive_observation(
            true,
            ProviderId::Claude,
            "oauth:person@example.com",
            PredictiveWarningWindow::Session,
            &window,
            &risk,
        ));
        assert!(!manager.record_predictive_observation(
            true,
            ProviderId::Claude,
            "oauth:person@example.com",
            PredictiveWarningWindow::Session,
            &window,
            &recovery,
        ));
        assert!(manager.record_predictive_observation(
            true,
            ProviderId::Claude,
            "oauth:person@example.com",
            PredictiveWarningWindow::Session,
            &window,
            &risk,
        ));
    }

    #[test]
    fn predictive_warning_reset_jitter_does_not_retrigger() {
        let now = DateTime::from_timestamp(1_800_000_000, 0).unwrap();
        let mut manager = NotificationManager::new();
        let risk = pace(false, Some(3600.0));

        assert!(manager.record_predictive_observation(
            true,
            ProviderId::Codex,
            "oauth:account-a",
            PredictiveWarningWindow::Weekly,
            &window(now, Duration::days(3), 10080),
            &risk,
        ));
        assert!(!manager.record_predictive_observation(
            true,
            ProviderId::Codex,
            "oauth:account-a",
            PredictiveWarningWindow::Weekly,
            &window(now, Duration::days(3) + Duration::minutes(5), 10080),
            &risk,
        ));
    }

    #[test]
    fn predictive_warning_isolates_provider_identity_source_and_window() {
        let now = DateTime::from_timestamp(1_800_000_000, 0).unwrap();
        let reset = window(now, Duration::hours(3), 300);
        let risk = pace(false, Some(3600.0));
        let mut manager = NotificationManager::new();

        for (provider, identity, warning_window) in [
            (
                ProviderId::Claude,
                "cli:person@example.com",
                PredictiveWarningWindow::Session,
            ),
            (
                ProviderId::Claude,
                "oauth:person@example.com",
                PredictiveWarningWindow::Session,
            ),
            (
                ProviderId::Claude,
                "token-account:1",
                PredictiveWarningWindow::Session,
            ),
            (
                ProviderId::Claude,
                "oauth:person@example.com",
                PredictiveWarningWindow::Weekly,
            ),
            (
                ProviderId::Codex,
                "oauth:person@example.com",
                PredictiveWarningWindow::Session,
            ),
        ] {
            assert!(manager.record_predictive_observation(
                true,
                provider,
                identity,
                warning_window,
                &reset,
                &risk,
            ));
        }
    }

    #[test]
    fn predictive_warning_requires_enabled_confident_positive_risk() {
        let now = DateTime::from_timestamp(1_800_000_000, 0).unwrap();
        let reset = window(now, Duration::hours(3), 300);
        let mut manager = NotificationManager::new();

        for (enabled, observation) in [
            (false, pace(false, Some(3600.0))),
            (true, pace(true, None)),
            (true, pace(false, Some(0.0))),
        ] {
            assert!(!manager.record_predictive_observation(
                enabled,
                ProviderId::Claude,
                "oauth:person@example.com",
                PredictiveWarningWindow::Session,
                &reset,
                &observation,
            ));
        }

        assert!(manager.record_predictive_observation(
            true,
            ProviderId::Claude,
            "oauth:person@example.com",
            PredictiveWarningWindow::Session,
            &reset,
            &pace(false, Some(3600.0)),
        ));
    }

    #[test]
    fn threshold_alerts_do_not_retrigger_across_refreshes() {
        let mut manager = NotificationManager::new_past_quiet_period();
        let settings = Settings::default();

        manager.check_and_notify(ProviderId::Cursor, "session", 80.0, &settings);
        manager.check_and_notify(ProviderId::Cursor, "weekly", 20.0, &settings);
        assert_eq!(manager.toasts_shown, 1);

        // Simulate many refreshes with the same split (high session, quiet weekly).
        for _ in 0..10 {
            manager.check_and_notify(ProviderId::Cursor, "session", 82.0, &settings);
            manager.check_and_notify(ProviderId::Cursor, "weekly", 25.0, &settings);
        }
        assert_eq!(
            manager.toasts_shown, 1,
            "quiet weekly must not clear session high-usage alert"
        );
    }

    #[test]
    fn threshold_alerts_are_isolated_per_window() {
        let mut manager = NotificationManager::new_past_quiet_period();
        let settings = Settings::default();

        manager.check_and_notify(ProviderId::Claude, "session", 75.0, &settings);
        manager.check_and_notify(ProviderId::Claude, "weekly", 92.0, &settings);
        assert_eq!(manager.toasts_shown, 2);

        manager.check_and_notify(ProviderId::Claude, "session", 76.0, &settings);
        manager.check_and_notify(ProviderId::Claude, "weekly", 93.0, &settings);
        assert_eq!(manager.toasts_shown, 2);
    }

    #[test]
    fn threshold_alerts_rearm_only_after_hysteresis_drop() {
        let mut manager = NotificationManager::new_past_quiet_period();
        let settings = Settings::default();

        manager.check_and_notify(ProviderId::Codex, "session", 80.0, &settings);
        assert_eq!(manager.toasts_shown, 1);

        // Still near the high threshold — do not re-arm.
        manager.check_and_notify(ProviderId::Codex, "session", 68.0, &settings);
        manager.check_and_notify(ProviderId::Codex, "session", 80.0, &settings);
        assert_eq!(manager.toasts_shown, 1);

        // Drop clearly below high-hysteresis, then climb again.
        manager.check_and_notify(ProviderId::Codex, "session", 60.0, &settings);
        manager.check_and_notify(ProviderId::Codex, "session", 80.0, &settings);
        assert_eq!(manager.toasts_shown, 2);
    }

    #[test]
    fn session_transition_ignores_first_observation() {
        let mut manager = NotificationManager::new_past_quiet_period();
        let settings = Settings::default();

        manager.check_session_transition(ProviderId::Claude, 100.0, &settings);
        assert_eq!(manager.toasts_shown, 0);

        manager.check_session_transition(ProviderId::Claude, 100.0, &settings);
        assert_eq!(manager.toasts_shown, 0);

        manager.check_session_transition(ProviderId::Claude, 40.0, &settings);
        assert_eq!(
            manager.toasts_shown, 1,
            "restore should notify after quiet baseline"
        );
    }

    #[test]
    fn startup_quiet_period_suppresses_but_arms_state() {
        let mut manager = NotificationManager::new();
        let settings = Settings::default();

        manager.check_and_notify(ProviderId::Cursor, "session", 95.0, &settings);
        manager.check_and_notify(ProviderId::Cursor, "weekly", 20.0, &settings);
        assert_eq!(manager.toasts_shown, 0);

        // Even after quiet would end, armed state prevents a late flood for the
        // same already-high reading.
        manager.started_at = std::time::Instant::now()
            .checked_sub(STARTUP_QUIET_PERIOD + std::time::Duration::from_secs(1))
            .unwrap();
        manager.check_and_notify(ProviderId::Cursor, "session", 95.0, &settings);
        assert_eq!(manager.toasts_shown, 0);
    }

    #[test]
    fn session_exhaustion_does_not_double_toast_with_transition() {
        let mut manager = NotificationManager::new_past_quiet_period();
        let settings = Settings::default();

        manager.check_session_transition(ProviderId::Claude, 50.0, &settings);
        manager.check_and_notify(ProviderId::Claude, "session", 100.0, &settings);
        manager.check_session_transition(ProviderId::Claude, 100.0, &settings);
        assert_eq!(
            manager.toasts_shown, 1,
            "only session-depleted should fire, not Exhausted + depleted"
        );
    }
}
