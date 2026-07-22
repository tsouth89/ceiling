use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use chrono::{DateTime, Utc};
use codexbar::settings::Settings;
use tauri::Manager;

use crate::commands::ProviderUsageSnapshot;
use crate::state::AppState;

const AUTO_REFRESH_POLL_INTERVAL: Duration = Duration::from_secs(15);
const PROVIDER_REFRESH_INTERVAL: Duration = Duration::from_secs(5 * 60);

pub fn install(app: tauri::AppHandle) {
    tauri::async_runtime::spawn(async move {
        let mut schedule: Option<(Duration, Instant)> = None;
        let mut next_reset: Option<DateTime<Utc>> = None;
        loop {
            let interval = PROVIDER_REFRESH_INTERVAL;
            let now = Instant::now();
            let scheduled_at = schedule
                .filter(|(scheduled_interval, _)| *scheduled_interval == interval)
                .map(|(_, scheduled_at)| scheduled_at)
                .unwrap_or(now);
            // A window that just reset is the one moment a stale reading is most
            // visible, and reset notifications are only produced inside a refresh.
            // Waiting for the next fixed tick delayed them by minutes, so cross a
            // known boundary and refresh on this wake instead.
            let scheduled_due = now >= scheduled_at;
            let reset_due = next_reset.is_some_and(|reset| Utc::now() >= reset);

            if scheduled_due || reset_due {
                // A crossed boundary must re-fetch even when the cache looks
                // fresh, otherwise the staleness gate swallows the reset read.
                let refreshed = if reset_due {
                    crate::commands::do_refresh_providers(&app).await
                } else {
                    crate::commands::do_refresh_providers_if_stale(&app).await
                };
                if let Err(error) = refreshed {
                    tracing::warn!(%error, "Automatic provider refresh failed");
                }
                if scheduled_due {
                    schedule = Some((
                        interval,
                        next_fixed_tick(scheduled_at, Instant::now(), interval),
                    ));
                }
            }

            // Only ever track a boundary that is still ahead of us. A provider
            // that keeps reporting an already-passed reset therefore falls back
            // to the fixed cadence instead of refreshing on every wake.
            if reset_due || next_reset.is_none() {
                next_reset = soonest_reset_after(&app, Utc::now());
            }
            tokio::time::sleep(AUTO_REFRESH_POLL_INTERVAL).await;
        }
    });
}

/// Earliest reset strictly after `after` across every enforced window we hold.
fn soonest_reset_after(app: &tauri::AppHandle, after: DateTime<Utc>) -> Option<DateTime<Utc>> {
    let state = app.state::<Mutex<AppState>>();
    let snapshots = match state.lock() {
        Ok(guard) => guard.provider_cache.clone(),
        Err(error) => {
            tracing::warn!("failed to lock app state to read reset boundaries: {error}");
            return None;
        }
    };
    snapshots
        .iter()
        .flat_map(snapshot_reset_times)
        .filter(|reset| *reset > after)
        .min()
}

/// Reset instants for a provider's enforced windows. Inactive windows are
/// skipped: they are not being enforced, so their boundaries should not wake
/// an extra fetch.
fn snapshot_reset_times(snapshot: &ProviderUsageSnapshot) -> Vec<DateTime<Utc>> {
    [
        Some(&snapshot.primary),
        snapshot.secondary.as_ref(),
        snapshot.model_specific.as_ref(),
        snapshot.tertiary.as_ref(),
    ]
    .into_iter()
    .flatten()
    .chain(
        snapshot
            .extra_rate_windows
            .iter()
            .map(|extra| &extra.window),
    )
    .filter_map(|window| window.resets_at.as_deref())
    .filter_map(parse_reset_time)
    .collect()
}

fn parse_reset_time(value: &str) -> Option<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(value)
        .ok()
        .map(|parsed| parsed.with_timezone(&Utc))
}

fn next_fixed_tick(
    previous_scheduled_at: Instant,
    completed_at: Instant,
    interval: Duration,
) -> Instant {
    let mut scheduled_at = previous_scheduled_at + interval;
    while scheduled_at <= completed_at {
        scheduled_at += interval;
    }
    scheduled_at
}

fn local_usage_provider_ids(settings: &Settings) -> Vec<String> {
    let budget_alerts_enabled = settings.show_notifications && settings.spend_budget_alerts_enabled;
    if !settings.powertoys_status_pipe_enabled && !budget_alerts_enabled {
        return Vec::new();
    }

    settings
        .get_enabled_provider_ids()
        .into_iter()
        .map(|provider| provider.cli_name().to_string())
        .filter(|provider_id| matches!(provider_id.as_str(), "codex" | "claude"))
        .collect()
}

fn clear_spend_budget_alert_state(app: &tauri::AppHandle, settings: &Settings) {
    if settings.show_notifications && settings.spend_budget_alerts_enabled {
        return;
    }

    let state = app.state::<Mutex<AppState>>();
    match state.lock() {
        Ok(mut guard) => guard
            .notification_manager
            .check_spend_budget("", "", 0.0, settings),
        Err(error) => {
            tracing::warn!("failed to lock app state to clear spend budget notification: {error}")
        }
    }
}

pub(crate) fn schedule_refresh_enrichment(app: &tauri::AppHandle, settings: &Settings) {
    // Clear the manager before a disabled budget path can return early. This
    // also handles the no-provider case, so re-enabling starts from a fresh
    // baseline rather than stale threshold state.
    clear_spend_budget_alert_state(app, settings);

    let provider_ids = local_usage_provider_ids(settings);
    if provider_ids.is_empty() {
        return;
    }
    let app = app.clone();
    let settings = settings.clone();
    static ENRICHMENT: OnceLock<Arc<tokio::sync::Mutex<()>>> = OnceLock::new();
    let Ok(guard) = Arc::clone(ENRICHMENT.get_or_init(|| Arc::new(tokio::sync::Mutex::new(()))))
        .try_lock_owned()
    else {
        return;
    };
    tauri::async_runtime::spawn(async move {
        let _guard = guard;
        crate::commands::refresh_provider_local_usage_cache(provider_ids.clone()).await;
        if settings.show_notifications && settings.spend_budget_alerts_enabled {
            let Some(total) = crate::commands::load_spend_budget_total(
                provider_ids,
                settings.spend_budget_period.clone(),
            )
            .await
            else {
                tracing::warn!("Unable to calculate local estimated API-value budget");
                return;
            };
            let state = app.state::<Mutex<AppState>>();
            match state.lock() {
                Ok(mut guard) => guard.notification_manager.check_spend_budget(
                    &total.cycle_id,
                    total.period_label,
                    total.estimated_usd,
                    &settings,
                ),
                Err(error) => {
                    tracing::warn!(
                        "failed to lock app state for spend budget notification: {error}"
                    )
                }
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fixed_cadence_advances_from_the_scheduled_tick() {
        let start = Instant::now();
        let interval = Duration::from_secs(100);
        let first_tick = start + interval;

        assert_eq!(
            next_fixed_tick(first_tick, first_tick + Duration::from_secs(60), interval),
            start + Duration::from_secs(200)
        );
        assert_eq!(
            next_fixed_tick(first_tick, first_tick + Duration::from_secs(260), interval),
            start + Duration::from_secs(400)
        );
    }

    fn sample_snapshot() -> crate::commands::ProviderUsageSnapshot {
        crate::commands::ProviderUsageSnapshot {
            provider_id: "claude".into(),
            display_name: "Claude".into(),
            primary: window_resetting_at(None),
            primary_label: Some("Session (5h)".into()),
            secondary: None,
            secondary_label: Some("Weekly".into()),
            model_specific: None,
            tertiary: None,
            extra_rate_windows: Vec::new(),
            inactive_rate_windows: Vec::new(),
            promo_signals: Vec::new(),
            reset_credits_available: None,
            cost: None,
            plan_name: None,
            account_email: None,
            source_label: "oauth".into(),
            updated_at: "2026-07-21T00:00:00Z".into(),
            error: None,
            pace: None,
            account_organization: None,
            tray_status_label: None,
            account_id: None,
            account_label: None,
            account_tint: None,
            fetch_duration_ms: None,
            wayfinder_usage: None,
        }
    }

    fn window_resetting_at(resets_at: Option<&str>) -> crate::commands::RateWindowSnapshot {
        crate::commands::RateWindowSnapshot {
            used_percent: 10.0,
            remaining_percent: 90.0,
            window_minutes: Some(300),
            resets_at: resets_at.map(str::to_string),
            reset_description: None,
            is_exhausted: false,
            reserve_percent: None,
            reserve_description: None,
            reserve_will_last_to_reset: false,
            reserve_eta_seconds: None,
        }
    }

    #[test]
    fn reset_times_cover_every_enforced_window_and_ignore_unparsable_ones() {
        let mut snapshot = crate::commands::ProviderUsageSnapshot {
            primary: window_resetting_at(Some("2026-07-21T05:00:00Z")),
            secondary: Some(window_resetting_at(Some("2026-07-28T02:00:00Z"))),
            tertiary: Some(window_resetting_at(None)),
            model_specific: Some(window_resetting_at(Some("not-a-timestamp"))),
            ..sample_snapshot()
        };
        snapshot
            .extra_rate_windows
            .push(crate::commands::NamedRateWindowSnapshot {
                id: "claude-routines".to_string(),
                title: "Daily Routines".to_string(),
                window: window_resetting_at(Some("2026-07-21T03:00:00Z")),
            });

        let mut times = snapshot_reset_times(&snapshot);
        times.sort();

        assert_eq!(
            times,
            vec![
                parse_reset_time("2026-07-21T03:00:00Z").unwrap(),
                parse_reset_time("2026-07-21T05:00:00Z").unwrap(),
                parse_reset_time("2026-07-28T02:00:00Z").unwrap(),
            ],
            "extra windows count, and missing or malformed timestamps are dropped"
        );
    }

    /// The loop only ever arms a boundary still ahead of it, so a provider stuck
    /// reporting a passed reset cannot force a refresh on every 15s wake.
    #[test]
    fn only_future_boundaries_are_armed() {
        let snapshot = crate::commands::ProviderUsageSnapshot {
            primary: window_resetting_at(Some("2026-07-21T05:00:00Z")),
            secondary: Some(window_resetting_at(Some("2026-07-21T03:00:00Z"))),
            ..sample_snapshot()
        };
        let times = snapshot_reset_times(&snapshot);

        let before_both = parse_reset_time("2026-07-21T02:00:00Z").unwrap();
        assert_eq!(
            times.iter().copied().filter(|t| *t > before_both).min(),
            parse_reset_time("2026-07-21T03:00:00Z"),
            "the nearest upcoming boundary wins"
        );

        let after_both = parse_reset_time("2026-07-21T06:00:00Z").unwrap();
        assert_eq!(
            times.iter().copied().filter(|t| *t > after_both).min(),
            None,
            "nothing is armed once every known boundary has passed"
        );
    }

    #[test]
    fn local_usage_refresh_only_includes_supported_enabled_providers() {
        let mut settings = Settings::default();
        assert!(local_usage_provider_ids(&settings).is_empty());

        settings.powertoys_status_pipe_enabled = true;
        settings.enabled_providers = ["codex".to_string(), "cursor".to_string()]
            .into_iter()
            .collect();

        assert_eq!(
            local_usage_provider_ids(&settings),
            vec!["codex".to_string()]
        );
    }

    #[test]
    fn spend_budget_also_enriches_supported_enabled_providers() {
        let settings = Settings {
            spend_budget_alerts_enabled: true,
            enabled_providers: ["claude".to_string(), "cursor".to_string()]
                .into_iter()
                .collect(),
            ..Settings::default()
        };

        assert_eq!(
            local_usage_provider_ids(&settings),
            vec!["claude".to_string()]
        );
    }
}
