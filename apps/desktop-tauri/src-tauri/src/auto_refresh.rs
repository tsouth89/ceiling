use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use codexbar::settings::Settings;
use tauri::Manager;

use crate::state::AppState;

const AUTO_REFRESH_POLL_INTERVAL: Duration = Duration::from_secs(15);
const PROVIDER_REFRESH_INTERVAL: Duration = Duration::from_secs(5 * 60);

pub fn install(app: tauri::AppHandle) {
    tauri::async_runtime::spawn(async move {
        let mut schedule: Option<(Duration, Instant)> = None;
        loop {
            let interval = PROVIDER_REFRESH_INTERVAL;
            let now = Instant::now();
            let scheduled_at = schedule
                .filter(|(scheduled_interval, _)| *scheduled_interval == interval)
                .map(|(_, scheduled_at)| scheduled_at)
                .unwrap_or(now);
            if now >= scheduled_at {
                if let Err(error) = crate::commands::do_refresh_providers_if_stale(&app).await {
                    tracing::warn!(%error, "Automatic provider refresh failed");
                }
                schedule = Some((
                    interval,
                    next_fixed_tick(scheduled_at, Instant::now(), interval),
                ));
            }
            tokio::time::sleep(AUTO_REFRESH_POLL_INTERVAL).await;
        }
    });
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
