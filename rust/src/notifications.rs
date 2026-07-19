//! System notifications for Ceiling
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

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct PredictiveObservationKey {
    provider: ProviderId,
    identity: String,
    window: PredictiveWarningWindow,
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

/// Drop below this offset under the high threshold before re-arming alerts.
/// Avoids flicker when a meter hovers around the boundary across refreshes.
const REARM_HYSTERESIS: f64 = 3.0;

/// Absolute safety limits for Windows toasts. A provider refresh may discover
/// several state changes at once, but Ceiling must never turn that into a toast
/// storm. Suppressed alerts are intentionally not replayed later.
const MIN_TOAST_INTERVAL: std::time::Duration = std::time::Duration::from_secs(5 * 60);
const TOAST_BURST_WINDOW: std::time::Duration = std::time::Duration::from_secs(60 * 60);
const MAX_TOASTS_PER_BURST_WINDOW: usize = 3;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ToastPriority {
    Normal,
    Reset,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct ThresholdAlertKey {
    provider: ProviderId,
    window: String,
    kind: NotificationType,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct ThresholdWindowKey {
    provider: ProviderId,
    window: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum SpendBudgetAlertLevel {
    Warning,
    NearCap,
}

/// Notification manager
pub struct NotificationManager {
    /// Track which threshold notifications have been sent to avoid spam.
    /// Keyed by provider + window so session/weekly cannot clear each other.
    sent_notifications: std::collections::HashSet<ThresholdAlertKey>,
    /// Windows observed during this process. The first trustworthy reading is
    /// a baseline, never a threshold crossing, even when provider startup took
    /// longer than the wall-clock quiet period.
    observed_threshold_windows: std::collections::HashSet<ThresholdWindowKey>,
    /// Candidate threshold crossings must survive a second provider read.
    pending_thresholds: std::collections::HashMap<ThresholdWindowKey, NotificationType>,
    /// Track previous session percent for depleted/restored transitions.
    /// Missing entry means we have not observed this provider yet (baseline).
    previous_session_percent: std::collections::HashMap<ProviderId, f64>,
    /// Target depleted state awaiting a second consistent provider read.
    pending_session_states: std::collections::HashMap<ProviderId, bool>,
    predictive_warning_keys: std::collections::HashSet<PredictiveWarningKey>,
    observed_predictive_windows: std::collections::HashSet<PredictiveObservationKey>,
    /// A budget is global, but its total resets at the boundary represented by
    /// this key (for example, `daily:2026-07-19`).
    spend_budget_cycle: Option<String>,
    spend_budget_observed: bool,
    spend_budget_sent: std::collections::HashSet<SpendBudgetAlertLevel>,
    spend_budget_pending: Option<SpendBudgetAlertLevel>,
    /// Remains false until the initial provider refresh has established a
    /// trustworthy in-process baseline for every enabled provider.
    notifications_armed: bool,
    /// Production provider refreshes opt into a hard per-refresh and rolling
    /// toast budget. Keeping this inactive by default also leaves isolated CLI
    /// callers and focused unit tests deterministic.
    refresh_cycle_active: bool,
    toast_emitted_this_refresh: bool,
    last_toast_at: Option<std::time::Instant>,
    recent_toasts: std::collections::VecDeque<std::time::Instant>,
    #[cfg(test)]
    pub(crate) toasts_shown: usize,
}

impl NotificationManager {
    pub fn new() -> Self {
        Self {
            sent_notifications: std::collections::HashSet::new(),
            observed_threshold_windows: std::collections::HashSet::new(),
            pending_thresholds: std::collections::HashMap::new(),
            previous_session_percent: std::collections::HashMap::new(),
            pending_session_states: std::collections::HashMap::new(),
            predictive_warning_keys: std::collections::HashSet::new(),
            observed_predictive_windows: std::collections::HashSet::new(),
            spend_budget_cycle: None,
            spend_budget_observed: false,
            spend_budget_sent: std::collections::HashSet::new(),
            spend_budget_pending: None,
            notifications_armed: false,
            refresh_cycle_active: false,
            toast_emitted_this_refresh: false,
            last_toast_at: None,
            recent_toasts: std::collections::VecDeque::new(),
            #[cfg(test)]
            toasts_shown: 0,
        }
    }

    #[cfg(test)]
    fn new_armed() -> Self {
        let mut manager = Self::new();
        manager.arm_after_startup_baseline();
        manager
    }

    pub fn arm_after_startup_baseline(&mut self) {
        self.notifications_armed = true;
    }

    pub fn notifications_are_armed(&self) -> bool {
        self.notifications_armed
    }

    /// Start a real provider refresh. Every refresh gets at most one toast, and
    /// the process-wide rolling limits still apply across refreshes.
    pub fn begin_refresh_cycle(&mut self) {
        self.refresh_cycle_active = true;
        self.toast_emitted_this_refresh = false;
    }

    /// Route confirmed reset events through the same startup gate and circuit
    /// breaker used by threshold notifications. Resets are the one event users
    /// should never miss, so they may bypass the rolling cooldown while still
    /// respecting the one-toast-per-refresh ceiling.
    pub fn notify_capacity_event(&mut self, title: &str, body: &str) -> bool {
        self.emit_toast_with_priority(title, body, ToastPriority::Reset)
    }

    fn emit_toast(&mut self, title: &str, body: &str) -> bool {
        self.emit_toast_with_priority(title, body, ToastPriority::Normal)
    }

    fn emit_toast_with_priority(
        &mut self,
        title: &str,
        body: &str,
        priority: ToastPriority,
    ) -> bool {
        if !self.notifications_armed {
            tracing::debug!(
                "Suppressing toast while establishing startup baseline: {} — {}",
                title,
                body
            );
            return false;
        }
        #[cfg(all(debug_assertions, not(test)))]
        if std::env::var("CEILING_ENABLE_DEV_NOTIFICATIONS").as_deref() != Ok("1") {
            tracing::debug!(
                title,
                "suppressing Windows toast from a debug/test-adjacent Ceiling build"
            );
            return false;
        }
        #[cfg(all(target_os = "windows", not(test)))]
        if let Err(reason) = windows_notification_delivery_check() {
            tracing::warn!(title, %reason, "Windows notification delivery is disabled");
            return false;
        }
        if self.refresh_cycle_active {
            let now = std::time::Instant::now();
            while self
                .recent_toasts
                .front()
                .is_some_and(|sent_at| now.duration_since(*sent_at) >= TOAST_BURST_WINDOW)
            {
                self.recent_toasts.pop_front();
            }
            let too_soon = self
                .last_toast_at
                .is_some_and(|sent_at| now.duration_since(sent_at) < MIN_TOAST_INTERVAL);
            let rolling_limit_reached = self.recent_toasts.len() >= MAX_TOASTS_PER_BURST_WINDOW;
            let normal_rate_limited =
                priority == ToastPriority::Normal && (too_soon || rolling_limit_reached);
            if self.toast_emitted_this_refresh || normal_rate_limited {
                tracing::warn!(
                    title,
                    ?priority,
                    per_refresh_limit = self.toast_emitted_this_refresh,
                    minimum_interval_limit = too_soon,
                    rolling_count = self.recent_toasts.len(),
                    "suppressing notification due to toast circuit breaker"
                );
                return false;
            }
            self.toast_emitted_this_refresh = true;
            self.last_toast_at = Some(now);
            self.recent_toasts.push_back(now);
        }
        #[cfg(test)]
        {
            self.toasts_shown += 1;
            // Unit tests exercise notification state with synthetic usage
            // values. Never let those fixtures escape into the host OS.
            true
        }
        #[cfg(not(test))]
        {
            self.show_toast(title, body);
            true
        }
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
            self.observed_predictive_windows
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
        let observation_key = PredictiveObservationKey {
            provider,
            identity: identity.to_string(),
            window,
        };
        let first_observation = self.observed_predictive_windows.insert(observation_key);

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
        !first_observation && !warned_this_cycle
    }

    pub fn set_predictive_warnings_enabled(&mut self, provider: ProviderId, enabled: bool) {
        if !enabled {
            self.predictive_warning_keys
                .retain(|key| key.provider != provider);
            self.observed_predictive_windows
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
        if self.emit_toast(&title, &body) {
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
        if !settings.show_notifications || !matches!(window, "session" | "weekly") {
            return;
        }

        let thresholds = settings.usage_thresholds(provider, window);
        let rearm_below = (thresholds.high - REARM_HYSTERESIS).max(0.0);
        let window_key = ThresholdWindowKey {
            provider,
            window: window.to_string(),
        };

        // A window gets one calm warning per cycle. Crossing critical or
        // exhausted later must not produce a second toast for the same limit.
        let notification_type = if used_percent >= thresholds.high {
            Some(NotificationType::HighUsage)
        } else if used_percent < rearm_below {
            self.clear_window_alerts(provider, window);
            self.pending_thresholds.remove(&window_key);
            None
        } else {
            self.pending_thresholds.remove(&window_key);
            None
        };

        // Treat the first live value for every provider/window as its baseline.
        // A slow launch must not reinterpret already-high usage as a crossing.
        if self.observed_threshold_windows.insert(window_key.clone()) {
            for kind in [(used_percent >= thresholds.high).then_some(NotificationType::HighUsage)]
                .into_iter()
                .flatten()
            {
                self.sent_notifications.insert(ThresholdAlertKey {
                    provider,
                    window: window.to_string(),
                    kind,
                });
            }
            self.pending_thresholds.remove(&window_key);
            return;
        }

        if let Some(notif_type) = notification_type {
            let key = ThresholdAlertKey {
                provider,
                window: window.to_string(),
                kind: notif_type,
            };
            if !self.sent_notifications.contains(&key) {
                if self.pending_thresholds.get(&window_key) != Some(&notif_type) {
                    self.pending_thresholds.insert(window_key, notif_type);
                    return;
                }
                self.pending_thresholds.remove(&window_key);
                // Mark before emit so startup quiet still arms state.
                self.sent_notifications.insert(key);
                self.send_notification(provider, window, used_percent, notif_type, settings);
            }
        }
    }

    /// Check the global, locally-estimated API-value budget. Like quota alerts,
    /// a crossing must survive two scans and a current-period startup reading is
    /// only a baseline. The budget uses its own cycle key so daily and calendar
    /// month-to-date totals re-arm naturally at their reset boundary.
    pub fn check_spend_budget(
        &mut self,
        cycle_id: &str,
        period_label: &str,
        estimated_usd: f64,
        settings: &Settings,
    ) {
        let enabled = settings.show_notifications
            && settings.spend_budget_alerts_enabled
            && settings.spend_budget_limit_usd.is_finite()
            && settings.spend_budget_limit_usd > 0.0;
        if !enabled || !estimated_usd.is_finite() || estimated_usd < 0.0 {
            self.spend_budget_cycle = None;
            self.spend_budget_observed = false;
            self.spend_budget_sent.clear();
            self.spend_budget_pending = None;
            return;
        }

        if self.spend_budget_cycle.as_deref() != Some(cycle_id) {
            self.spend_budget_cycle = Some(cycle_id.to_string());
            self.spend_budget_observed = false;
            self.spend_budget_sent.clear();
            self.spend_budget_pending = None;
        }

        let level = if estimated_usd >= settings.spend_budget_limit_usd {
            Some(SpendBudgetAlertLevel::NearCap)
        } else if settings.spend_budget_warning_usd.is_finite()
            && settings.spend_budget_warning_usd > 0.0
            && estimated_usd >= settings.spend_budget_warning_usd
        {
            Some(SpendBudgetAlertLevel::Warning)
        } else {
            None
        };

        // Never reinterpret an already-high total at startup (or immediately
        // after enabling the feature) as a fresh threshold crossing.
        if !self.spend_budget_observed {
            self.spend_budget_observed = true;
            if let Some(level) = level {
                self.spend_budget_sent.insert(level);
            }
            return;
        }

        let Some(level) = level else {
            self.spend_budget_pending = None;
            return;
        };
        if self.spend_budget_sent.contains(&level) {
            return;
        }
        if self.spend_budget_pending != Some(level) {
            self.spend_budget_pending = Some(level);
            return;
        }

        self.spend_budget_pending = None;
        self.spend_budget_sent.insert(level);
        let title = match level {
            SpendBudgetAlertLevel::Warning => "Estimated API value budget warning",
            SpendBudgetAlertLevel::NearCap => "Estimated API value near cap",
        };
        let threshold = match level {
            SpendBudgetAlertLevel::Warning => settings.spend_budget_warning_usd,
            SpendBudgetAlertLevel::NearCap => settings.spend_budget_limit_usd,
        };
        let body = format!(
            "{period_label} estimated API value is ${estimated_usd:.2}; your {} is ${threshold:.2}. This is an estimate from local Codex and Claude logs, not a bill.",
            match level {
                SpendBudgetAlertLevel::Warning => "warning budget",
                SpendBudgetAlertLevel::NearCap => "cap",
            }
        );
        if self.emit_toast(title, &body) {
            play_alert(
                match level {
                    SpendBudgetAlertLevel::Warning => AlertSound::Warning,
                    SpendBudgetAlertLevel::NearCap => AlertSound::Critical,
                },
                settings,
            );
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
        let current_depleted = current_percent >= DEPLETED_THRESHOLD;

        let Some(previous_percent) = self.previous_session_percent.get(&provider).copied() else {
            // First observation is a baseline only. In particular, do not arm
            // a restoration toast from a single already-depleted startup read.
            self.previous_session_percent
                .insert(provider, current_percent);
            return;
        };
        let previous_depleted = previous_percent >= DEPLETED_THRESHOLD;

        if current_depleted == previous_depleted {
            self.pending_session_states.remove(&provider);
            self.previous_session_percent
                .insert(provider, current_percent);
            return;
        }

        if self.pending_session_states.get(&provider) != Some(&current_depleted) {
            self.pending_session_states
                .insert(provider, current_depleted);
            return;
        }
        self.pending_session_states.remove(&provider);

        // Check for depleted transition: was not depleted, now is
        if current_depleted {
            let title = NotificationType::SessionDepleted.title();
            let body = format!(
                "{} session depleted. 0% left. Will notify when available again.",
                provider.display_name()
            );
            if self.emit_toast(title, &body) {
                play_alert(AlertSound::Error, settings);
            }
            self.sent_notifications.insert(ThresholdAlertKey {
                provider,
                window: "session".to_string(),
                kind: NotificationType::SessionDepleted,
            });
        }
        // Check for restored transition: was depleted, now is not
        else {
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
                if self.emit_toast(title, &body) {
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
        if self.emit_toast(title, &body) {
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
            "monthly" => "monthly",
            "primary" => "usage",
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
        if self.emit_toast(title, &body) {
            play_alert(AlertSound::Error, settings);
        }
    }

    #[cfg(target_os = "windows")]
    fn show_toast(&self, title: &str, body: &str) {
        use std::os::windows::process::CommandExt;
        use std::process::Command;

        ensure_aumid_registered_once();

        // Fire-and-forget on the normal notification path: a provider refresh must
        // never block on PowerShell. Failures are logged, not surfaced.
        match Command::new("powershell")
            .args([
                "-NoProfile",
                "-ExecutionPolicy",
                "Bypass",
                "-Command",
                &toast_powershell_script(title, body),
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
                "--app-name=Ceiling",
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

/// Escape a string for inclusion in the toast XML payload.
#[cfg(target_os = "windows")]
fn xml_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

/// Build the PowerShell snippet that shows a single Ceiling toast. Uses
/// ToastGeneric (Win 10+) and wraps the body in try/catch so PowerShell exits
/// with code 1 on failure instead of swallowing the error. A single-quoted
/// here-string (@'...'@) prevents PowerShell from expanding the XML content.
#[cfg(target_os = "windows")]
fn toast_powershell_script(title: &str, body: &str) -> String {
    format!(
        r#"try {{
    [Windows.UI.Notifications.ToastNotificationManager, Windows.UI.Notifications, ContentType = WindowsRuntime] | Out-Null
    [Windows.Data.Xml.Dom.XmlDocument, Windows.Data.Xml.Dom.XmlDocument, ContentType = WindowsRuntime] | Out-Null
    $template = @'
<toast><visual><binding template="ToastGeneric"><text>{}</text><text>{}</text></binding></visual></toast>
'@
    $xml = New-Object Windows.Data.Xml.Dom.XmlDocument
    $xml.LoadXml($template)
    $toast = [Windows.UI.Notifications.ToastNotification]::new($xml)
    $notifier = [Windows.UI.Notifications.ToastNotificationManager]::CreateToastNotifier("Ceiling")
    if ($null -eq $notifier) {{ throw "CreateToastNotifier returned null" }}
    $notifier.Show($toast)
}} catch {{
    [System.Console]::Error.WriteLine("Ceiling toast failed: $_")
    exit 1
}}"#,
        xml_escape(title),
        xml_escape(body)
    )
}

/// Register the AUMID at most once per process before showing a toast.
#[cfg(target_os = "windows")]
fn ensure_aumid_registered_once() {
    use std::sync::Once;
    static AUMID_INIT: Once = Once::new();
    AUMID_INIT.call_once(ensure_aumid_registered);
}

/// Show a Ceiling toast synchronously and report whether the OS actually
/// accepted it. Powers the Settings "Send test notification" button, so it
/// deliberately bypasses the startup-baseline gate, the per-refresh limit, and
/// the rolling cooldown: the user asked for it right now and needs to see the
/// real Windows toast pipeline (AUMID registration + PowerShell + WinRT)
/// succeed or fail end-to-end. Blocking is fine — this runs off a user click,
/// never the refresh loop.
#[cfg(target_os = "windows")]
pub fn send_test_notification() -> Result<(), String> {
    use std::os::windows::process::CommandExt;
    use std::process::Command;

    ensure_aumid_registered_once();
    windows_notification_delivery_check()?;

    let title = "Ceiling notifications are on";
    let body = "This is a test. Unexpected resets and usage alerts will look like this.";

    let output = Command::new("powershell")
        .args([
            "-NoProfile",
            "-ExecutionPolicy",
            "Bypass",
            "-Command",
            &toast_powershell_script(title, body),
        ])
        .creation_flags(0x08000000) // CREATE_NO_WINDOW
        .output()
        .map_err(|e| format!("Could not launch the Windows notifier: {e}"))?;

    if output.status.success() {
        tracing::debug!("Test toast notification shown");
        return Ok(());
    }

    let detail = String::from_utf8_lossy(&output.stderr);
    let detail = detail.trim();
    tracing::warn!("Test toast notification failed: {detail}");
    if detail.is_empty() {
        Err("Windows rejected the notification. Check notification settings.".to_string())
    } else {
        Err(detail.to_string())
    }
}

#[cfg(target_os = "windows")]
fn windows_notification_delivery_check() -> Result<(), String> {
    use winreg::RegKey;
    use winreg::enums::HKEY_CURRENT_USER;

    let hkcu = RegKey::predef(HKEY_CURRENT_USER);
    if let Ok(key) =
        hkcu.open_subkey(r"SOFTWARE\Microsoft\Windows\CurrentVersion\PushNotifications")
        && key.get_value::<u32, _>("ToastEnabled").ok() == Some(0)
    {
        return Err(
            "Windows notifications are turned off. Turn them on in Settings > System > Notifications."
                .to_string(),
        );
    }

    if let Ok(key) = hkcu
        .open_subkey(r"SOFTWARE\Microsoft\Windows\CurrentVersion\Notifications\Settings\Ceiling")
    {
        let disabled = notification_channel_is_disabled(key.get_value::<u32, _>("Enabled").ok());
        if disabled {
            return Err(
                "Windows notifications are turned off for Ceiling. Enable Ceiling in Settings > System > Notifications."
                    .to_string(),
            );
        }
    }

    Ok(())
}

#[cfg(target_os = "windows")]
fn notification_channel_is_disabled(enabled: Option<u32>) -> bool {
    // `ShowInActionCenter=0` only disables notification-center history. Windows
    // may leave it behind after the user re-enables the app channel, while
    // banners are deliverable again. Only the explicit app toggle blocks us.
    enabled == Some(0)
}

/// Non-Windows fallback for the test-notification button.
#[cfg(not(target_os = "windows"))]
pub fn send_test_notification() -> Result<(), String> {
    use std::process::Command;

    let title = "Ceiling notifications are on";
    let body = "This is a test. Unexpected resets and usage alerts will look like this.";

    match Command::new("notify-send")
        .args([
            "--app-name=Ceiling",
            "--icon=dialog-information",
            title,
            body,
        ])
        .output()
    {
        Ok(output) if output.status.success() => Ok(()),
        Ok(output) => {
            let detail = String::from_utf8_lossy(&output.stderr);
            let detail = detail.trim();
            if detail.is_empty() {
                Err("The system notifier rejected the notification.".to_string())
            } else {
                Err(detail.to_string())
            }
        }
        Err(e) => Err(format!("No system notifier available: {e}")),
    }
}

/// Register the Ceiling App User Model ID (AUMID) in the Windows registry so that
/// `CreateToastNotifier("Ceiling")` resolves to a valid notifier instead of returning
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
        .create_subkey(r"SOFTWARE\Classes\AppUserModelId\Ceiling")
        .and_then(|(key, _)| key.set_value("DisplayName", &"Ceiling"));

    match result {
        Ok(()) => tracing::debug!("Ceiling AUMID registered for Windows toast notifications"),
        Err(e) => tracing::warn!("Failed to register Ceiling AUMID: {}", e),
    }
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

    #[cfg(target_os = "windows")]
    #[test]
    fn notification_channel_only_blocks_explicit_disable_flags() {
        assert!(!notification_channel_is_disabled(None));
        assert!(!notification_channel_is_disabled(Some(1)));
        assert!(notification_channel_is_disabled(Some(0)));
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
        let mut manager = NotificationManager::new_armed();

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
    fn first_predictive_risk_is_a_silent_baseline() {
        let now = DateTime::from_timestamp(1_800_000_000, 0).unwrap();
        let window = window(now, Duration::hours(3), 300);
        let risk = pace(false, Some(3600.0));
        let mut manager = NotificationManager::new_armed();

        assert!(!manager.record_predictive_observation(
            true,
            ProviderId::Codex,
            "oauth:person@example.com",
            PredictiveWarningWindow::Session,
            &window,
            &risk,
        ));
        assert!(!manager.record_predictive_observation(
            true,
            ProviderId::Codex,
            "oauth:person@example.com",
            PredictiveWarningWindow::Session,
            &window,
            &risk,
        ));
    }

    #[test]
    fn predictive_warning_reset_jitter_does_not_retrigger() {
        let now = DateTime::from_timestamp(1_800_000_000, 0).unwrap();
        let mut manager = NotificationManager::new_armed();
        let risk = pace(false, Some(3600.0));

        assert!(!manager.record_predictive_observation(
            true,
            ProviderId::Codex,
            "oauth:account-a",
            PredictiveWarningWindow::Weekly,
            &window(now, Duration::days(3), 10080),
            &pace(true, None),
        ));

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
        let mut manager = NotificationManager::new_armed();

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
            assert!(!manager.record_predictive_observation(
                true,
                provider,
                identity,
                warning_window,
                &reset,
                &pace(true, None),
            ));
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
        let mut manager = NotificationManager::new_armed();
        let settings = Settings::default();

        manager.check_and_notify(ProviderId::Cursor, "session", 40.0, &settings);
        manager.check_and_notify(ProviderId::Cursor, "session", 86.0, &settings);
        manager.check_and_notify(ProviderId::Cursor, "weekly", 20.0, &settings);
        manager.check_and_notify(ProviderId::Cursor, "session", 87.0, &settings);
        assert_eq!(manager.toasts_shown, 1);

        // Simulate many refreshes with the same split (high session, quiet weekly).
        for _ in 0..10 {
            manager.check_and_notify(ProviderId::Cursor, "session", 87.0, &settings);
            manager.check_and_notify(ProviderId::Cursor, "weekly", 25.0, &settings);
        }
        assert_eq!(
            manager.toasts_shown, 1,
            "quiet weekly must not clear session high-usage alert"
        );
    }

    #[test]
    fn threshold_alerts_are_isolated_per_window() {
        let mut manager = NotificationManager::new_armed();
        let settings = Settings::default();

        manager.check_and_notify(ProviderId::Claude, "session", 40.0, &settings);
        manager.check_and_notify(ProviderId::Claude, "weekly", 40.0, &settings);
        manager.check_and_notify(ProviderId::Claude, "session", 86.0, &settings);
        manager.check_and_notify(ProviderId::Claude, "weekly", 92.0, &settings);
        manager.check_and_notify(ProviderId::Claude, "session", 87.0, &settings);
        manager.check_and_notify(ProviderId::Claude, "weekly", 93.0, &settings);
        assert_eq!(manager.toasts_shown, 2);

        manager.check_and_notify(ProviderId::Claude, "session", 87.0, &settings);
        manager.check_and_notify(ProviderId::Claude, "weekly", 93.0, &settings);
        assert_eq!(manager.toasts_shown, 2);
    }

    #[test]
    fn threshold_alerts_rearm_only_after_hysteresis_drop() {
        let mut manager = NotificationManager::new_armed();
        let settings = Settings::default();

        manager.check_and_notify(ProviderId::Codex, "session", 40.0, &settings);
        manager.check_and_notify(ProviderId::Codex, "session", 86.0, &settings);
        manager.check_and_notify(ProviderId::Codex, "session", 87.0, &settings);
        assert_eq!(manager.toasts_shown, 1);

        // Still near the high threshold — do not re-arm.
        manager.check_and_notify(ProviderId::Codex, "session", 83.0, &settings);
        manager.check_and_notify(ProviderId::Codex, "session", 86.0, &settings);
        assert_eq!(manager.toasts_shown, 1);

        // Drop clearly below high-hysteresis, then climb again.
        manager.check_and_notify(ProviderId::Codex, "session", 80.0, &settings);
        manager.check_and_notify(ProviderId::Codex, "session", 86.0, &settings);
        manager.check_and_notify(ProviderId::Codex, "session", 87.0, &settings);
        assert_eq!(manager.toasts_shown, 2);
    }

    #[test]
    fn session_transition_does_not_restore_from_a_depleted_startup_baseline() {
        let mut manager = NotificationManager::new_armed();
        let settings = Settings::default();

        manager.check_session_transition(ProviderId::Claude, 100.0, &settings);
        assert_eq!(manager.toasts_shown, 0);

        manager.check_session_transition(ProviderId::Claude, 100.0, &settings);
        assert_eq!(manager.toasts_shown, 0);

        manager.check_session_transition(ProviderId::Claude, 40.0, &settings);
        manager.check_session_transition(ProviderId::Claude, 42.0, &settings);
        assert_eq!(manager.toasts_shown, 0);
    }

    #[test]
    fn startup_baseline_suppresses_but_arms_state() {
        let mut manager = NotificationManager::new();
        let settings = Settings::default();

        manager.check_and_notify(ProviderId::Cursor, "session", 95.0, &settings);
        manager.check_and_notify(ProviderId::Cursor, "weekly", 20.0, &settings);
        manager.check_and_notify(ProviderId::Cursor, "session", 96.0, &settings);
        assert_eq!(manager.toasts_shown, 0);

        // Completing startup enables future transitions without replaying the
        // same already-high state observed during launch.
        manager.arm_after_startup_baseline();
        manager.check_and_notify(ProviderId::Cursor, "session", 95.0, &settings);
        assert_eq!(manager.toasts_shown, 0);

        manager.check_and_notify(ProviderId::Cursor, "session", 60.0, &settings);
        manager.check_and_notify(ProviderId::Cursor, "session", 86.0, &settings);
        manager.check_and_notify(ProviderId::Cursor, "session", 87.0, &settings);
        assert_eq!(manager.toasts_shown, 1);
    }

    #[test]
    fn first_high_reading_is_a_baseline_even_after_notifications_are_armed() {
        let mut manager = NotificationManager::new_armed();
        let settings = Settings::default();

        manager.check_and_notify(ProviderId::Cursor, "monthly", 82.0, &settings);
        manager.check_and_notify(ProviderId::Cursor, "monthly", 83.0, &settings);

        assert_eq!(
            manager.toasts_shown, 0,
            "usage that was already high at launch is not a new crossing"
        );
    }

    #[test]
    fn baseline_severity_downgrade_does_not_create_a_warning() {
        let mut manager = NotificationManager::new_armed();
        let settings = Settings::default();

        manager.check_and_notify(ProviderId::Cursor, "monthly", 95.0, &settings);
        manager.check_and_notify(ProviderId::Cursor, "monthly", 82.0, &settings);
        manager.check_and_notify(ProviderId::Cursor, "monthly", 81.0, &settings);

        assert_eq!(manager.toasts_shown, 0);
    }

    #[test]
    fn session_exhaustion_does_not_double_toast_with_transition() {
        let mut manager = NotificationManager::new_armed();
        let settings = Settings::default();

        manager.check_session_transition(ProviderId::Claude, 50.0, &settings);
        manager.check_and_notify(ProviderId::Claude, "session", 100.0, &settings);
        manager.check_session_transition(ProviderId::Claude, 100.0, &settings);
        manager.check_session_transition(ProviderId::Claude, 100.0, &settings);
        assert_eq!(
            manager.toasts_shown, 1,
            "only session-depleted should fire, not Exhausted + depleted"
        );
    }

    #[test]
    fn one_read_threshold_and_session_spikes_do_not_notify() {
        let mut manager = NotificationManager::new_armed();
        let settings = Settings::default();

        manager.check_and_notify(ProviderId::Codex, "session", 80.0, &settings);
        manager.check_and_notify(ProviderId::Codex, "session", 5.0, &settings);
        manager.check_session_transition(ProviderId::Claude, 40.0, &settings);
        manager.check_session_transition(ProviderId::Claude, 100.0, &settings);
        manager.check_session_transition(ProviderId::Claude, 42.0, &settings);

        assert_eq!(manager.toasts_shown, 0);
    }

    #[test]
    fn reset_notifications_bypass_cooldown_but_not_per_refresh_limit() {
        let mut manager = NotificationManager::new_armed();

        manager.begin_refresh_cycle();
        assert!(manager.notify_capacity_event("First", "First trusted event"));
        assert!(!manager.notify_capacity_event("Second", "Same refresh"));
        assert_eq!(manager.toasts_shown, 1);

        // A confirmed reset in a later refresh is more important than the
        // general cooldown and must remain dependable.
        manager.begin_refresh_cycle();
        assert!(manager.notify_capacity_event("Third", "Next refresh"));
        assert_eq!(manager.toasts_shown, 2);
    }

    #[test]
    fn monthly_and_unknown_windows_never_raise_usage_toasts() {
        let mut manager = NotificationManager::new_armed();
        let settings = Settings::default();

        for window in ["monthly", "primary"] {
            manager.check_and_notify(ProviderId::Cursor, window, 40.0, &settings);
            manager.check_and_notify(ProviderId::Cursor, window, 100.0, &settings);
            manager.check_and_notify(ProviderId::Cursor, window, 100.0, &settings);
        }

        assert_eq!(manager.toasts_shown, 0);
    }

    #[test]
    fn critical_and_exhausted_usage_do_not_add_more_toasts() {
        let mut manager = NotificationManager::new_armed();
        let settings = Settings::default();

        manager.check_and_notify(ProviderId::Codex, "weekly", 40.0, &settings);
        manager.check_and_notify(ProviderId::Codex, "weekly", 86.0, &settings);
        manager.check_and_notify(ProviderId::Codex, "weekly", 87.0, &settings);
        manager.check_and_notify(ProviderId::Codex, "weekly", 95.0, &settings);
        manager.check_and_notify(ProviderId::Codex, "weekly", 100.0, &settings);

        assert_eq!(manager.toasts_shown, 1);
    }

    #[test]
    fn spend_budget_requires_a_new_confirmed_crossing_per_cycle() {
        let mut manager = NotificationManager::new_armed();
        let settings = Settings {
            spend_budget_alerts_enabled: true,
            spend_budget_warning_usd: 5.0,
            spend_budget_limit_usd: 15.0,
            ..Settings::default()
        };

        manager.check_spend_budget("daily:2026-07-19", "Daily", 4.0, &settings);
        manager.check_spend_budget("daily:2026-07-19", "Daily", 5.2, &settings);
        assert_eq!(manager.toasts_shown, 0);
        manager.check_spend_budget("daily:2026-07-19", "Daily", 5.3, &settings);
        assert_eq!(manager.toasts_shown, 1);

        manager.check_spend_budget("daily:2026-07-19", "Daily", 15.1, &settings);
        manager.check_spend_budget("daily:2026-07-19", "Daily", 15.2, &settings);
        assert_eq!(manager.toasts_shown, 2);

        manager.check_spend_budget("daily:2026-07-20", "Daily", 16.0, &settings);
        manager.check_spend_budget("daily:2026-07-20", "Daily", 16.0, &settings);
        assert_eq!(
            manager.toasts_shown, 2,
            "a high first reading of the next day is a quiet baseline"
        );
    }

    #[test]
    fn disabling_spend_budget_alerts_clears_its_in_memory_state() {
        let mut manager = NotificationManager::new_armed();
        let enabled = Settings {
            spend_budget_alerts_enabled: true,
            spend_budget_warning_usd: 5.0,
            spend_budget_limit_usd: 15.0,
            ..Settings::default()
        };

        manager.check_spend_budget("daily:2026-07-19", "Daily", 4.0, &enabled);
        assert!(manager.spend_budget_observed);
        assert!(manager.spend_budget_cycle.is_some());

        manager.check_spend_budget("", "", 0.0, &Settings::default());

        assert!(!manager.spend_budget_observed);
        assert!(manager.spend_budget_cycle.is_none());
        assert!(manager.spend_budget_sent.is_empty());
        assert!(manager.spend_budget_pending.is_none());
    }
}
