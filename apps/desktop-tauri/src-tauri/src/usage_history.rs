use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};

use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};

use crate::commands::{ProviderUsageSnapshot, RateWindowSnapshot};

const STORE_VERSION: u8 = 3;
const RETENTION_DAYS: i64 = 30;
const MIN_SAMPLE_INTERVAL_MINUTES: i64 = 5;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct UsageHistoryWindow {
    pub id: String,
    pub label: String,
    pub used_percent: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct UsageHistoryPoint {
    pub recorded_at: String,
    pub windows: Vec<UsageHistoryWindow>,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct UsageHistoryStore {
    #[serde(default)]
    version: u8,
    #[serde(default)]
    series: HashMap<String, Vec<UsageHistoryPoint>>,
}

fn store() -> &'static Mutex<UsageHistoryStore> {
    static STORE: OnceLock<Mutex<UsageHistoryStore>> = OnceLock::new();
    STORE.get_or_init(|| Mutex::new(load_store()))
}

pub fn record_snapshot(snapshot: &ProviderUsageSnapshot) {
    if snapshot.error.is_some() {
        return;
    }
    let windows = snapshot_windows(snapshot);
    if windows.is_empty() {
        return;
    }

    let recorded_at = DateTime::parse_from_rfc3339(&snapshot.updated_at)
        .map(|value| value.with_timezone(&Utc))
        .unwrap_or_else(|_| Utc::now());
    let point = UsageHistoryPoint {
        recorded_at: recorded_at.to_rfc3339(),
        windows,
    };
    let key = scope_key(
        &snapshot.provider_id,
        snapshot.account_email.as_deref(),
        snapshot.account_id.as_deref(),
        snapshot.account_organization.as_deref(),
    );

    let Ok(mut guard) = store().lock() else {
        return;
    };
    guard.version = STORE_VERSION;
    let cutoff = recorded_at - Duration::days(RETENTION_DAYS);
    let points = guard.series.entry(key).or_default();
    points.retain(|existing| {
        DateTime::parse_from_rfc3339(&existing.recorded_at)
            .map(|value| value.with_timezone(&Utc) >= cutoff)
            .unwrap_or(false)
    });

    let should_replace = points.last().is_some_and(|last| {
        DateTime::parse_from_rfc3339(&last.recorded_at)
            .map(|value| {
                recorded_at - value.with_timezone(&Utc)
                    < Duration::minutes(MIN_SAMPLE_INTERVAL_MINUTES)
            })
            .unwrap_or(false)
    });
    if should_replace {
        if let Some(last) = points.last_mut() {
            *last = point;
        }
    } else {
        points.push(point);
    }
    persist_store(&guard);
}

pub fn provider_history(
    provider_id: &str,
    account_email: Option<&str>,
    account_id: Option<&str>,
    account_organization: Option<&str>,
) -> Vec<UsageHistoryPoint> {
    let Ok(guard) = store().lock() else {
        return Vec::new();
    };
    select_series(
        &guard.series,
        provider_id,
        account_email,
        account_id,
        account_organization,
    )
    .map(|points| visible_history(provider_id, points))
    .unwrap_or_default()
}

/// Pick the series to chart for a provider/account.
///
/// Providers do not always expose an account identity on every source, so an
/// identified read falls back to this provider's anonymous series, which is the
/// same account seen without its email.
///
/// It deliberately does **not** fall back to whichever series is freshest. That
/// reaches into a *different* account's history, which is exactly the cross-seat
/// leak multi-account switching must not have: switching accounts would chart
/// the previous seat's data until the new one accumulated its own.
fn select_series<'a>(
    series: &'a HashMap<String, Vec<UsageHistoryPoint>>,
    provider_id: &str,
    account_email: Option<&str>,
    account_id: Option<&str>,
    account_organization: Option<&str>,
) -> Option<&'a Vec<UsageHistoryPoint>> {
    let exact = scope_key(provider_id, account_email, account_id, account_organization);
    if let Some(points) = series.get(&exact) {
        return Some(points);
    }
    // A managed account has a stable identity. Falling back to an email or
    // anonymous series here can show another seat's history when accounts
    // share a login address, so wait for this account's first sample instead.
    if account_id.is_some() || account_organization.is_some() {
        return None;
    }
    let anonymous = scope_key(provider_id, None, None, None);
    if anonymous == exact {
        return None;
    }
    series.get(&anonymous)
}

fn visible_history(provider_id: &str, points: &[UsageHistoryPoint]) -> Vec<UsageHistoryPoint> {
    let mut points = points.to_vec();
    if provider_id.eq_ignore_ascii_case("cursor") {
        for point in &mut points {
            // Promotional was an inferred meter and remains hidden. On-demand
            // is a real capped lane, so keep its percentage in the reset
            // schedule; money is shown on the live overview/detail surfaces.
            point
                .windows
                .retain(|window| window.id.as_str() != "promotional");
        }
    }
    points
}

fn snapshot_windows(snapshot: &ProviderUsageSnapshot) -> Vec<UsageHistoryWindow> {
    let mut windows = Vec::new();
    push_window(
        &mut windows,
        "primary",
        snapshot.primary_label.as_deref(),
        &snapshot.primary,
    );
    if let Some(window) = snapshot.secondary.as_ref() {
        push_window(
            &mut windows,
            "secondary",
            snapshot.secondary_label.as_deref(),
            window,
        );
    }
    if let Some(window) = snapshot.model_specific.as_ref() {
        push_window(&mut windows, "model", Some("Model"), window);
    }
    if let Some(window) = snapshot.tertiary.as_ref() {
        // The window names itself when it can (OpenCode Go "Monthly"); "API" is
        // the historical fallback from when Cursor was the only tertiary.
        push_window(
            &mut windows,
            "tertiary",
            snapshot.tertiary_label.as_deref().or(Some("API")),
            window,
        );
    }
    for extra in &snapshot.extra_rate_windows {
        push_window(&mut windows, &extra.id, Some(&extra.title), &extra.window);
    }
    if snapshot.provider_id.eq_ignore_ascii_case("cursor") {
        windows.retain(|window| window.id.as_str() != "promotional");
    }
    windows
}

fn push_window(
    windows: &mut Vec<UsageHistoryWindow>,
    fallback_id: &str,
    label: Option<&str>,
    window: &RateWindowSnapshot,
) {
    let label = label
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or(fallback_id);
    let normalized = normalize_id(label);
    windows.push(UsageHistoryWindow {
        id: if normalized.is_empty() {
            fallback_id.to_string()
        } else {
            normalized
        },
        label: label.to_string(),
        used_percent: window.used_percent.clamp(0.0, 100.0),
    });
}

fn normalize_id(value: &str) -> String {
    value
        .to_ascii_lowercase()
        .split(|character: char| !character.is_ascii_alphanumeric())
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join("-")
}

fn scope_key(
    provider_id: &str,
    account_email: Option<&str>,
    account_id: Option<&str>,
    organization: Option<&str>,
) -> String {
    let identity = account_id
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .or_else(|| {
            account_email
                .map(str::trim)
                .filter(|value| !value.is_empty())
        })
        .or_else(|| {
            organization
                .map(str::trim)
                .filter(|value| !value.is_empty())
        })
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or("anonymous")
        .to_ascii_lowercase();
    format!(
        "{}:{:016x}",
        provider_id.to_ascii_lowercase(),
        fnv1a64(identity.as_bytes())
    )
}

fn fnv1a64(bytes: &[u8]) -> u64 {
    let mut hash = 0xcbf29ce484222325_u64;
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    hash
}

fn persistence_path() -> Option<PathBuf> {
    codexbar::settings::Settings::settings_path().and_then(|path| {
        path.parent()
            .map(|parent| parent.join("usage-history.json"))
    })
}

fn load_store() -> UsageHistoryStore {
    let Some(path) = persistence_path() else {
        return UsageHistoryStore::default();
    };
    let mut store: UsageHistoryStore = fs::read(&path)
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .unwrap_or_default();
    migrate_store(&mut store);
    store
}

/// A window's series id comes from the label it carried when it was recorded,
/// so renaming a label starts a second series and cuts the chart in two. Two
/// OpenCode Go renames land together: the rolling window now states its 5-hour
/// span, and the monthly window keeps its own name instead of the "API"
/// fallback left over from when Cursor was the only provider with a third
/// window. Points recorded under the old ids are relabelled in place so the
/// retained 30 days stay one line each.
fn migrate_store(store: &mut UsageHistoryStore) {
    if store.version >= 3 {
        return;
    }
    for (key, points) in store.series.iter_mut() {
        if !key.starts_with("opencodego:") {
            continue;
        }
        for point in points.iter_mut() {
            for window in &mut point.windows {
                let renamed = match window.id.as_str() {
                    "rolling" => Some(("rolling-5h", "Rolling (5h)")),
                    "api" => Some(("monthly", "Monthly")),
                    _ => None,
                };
                if let Some((id, label)) = renamed {
                    window.id = id.to_string();
                    window.label = label.to_string();
                }
            }
        }
    }
    store.version = STORE_VERSION;
}

fn persist_store(store: &UsageHistoryStore) {
    let Some(path) = persistence_path() else {
        return;
    };
    if let Some(parent) = path.parent()
        && let Err(error) = fs::create_dir_all(parent)
    {
        tracing::warn!("failed to create usage-history directory: {error}");
        return;
    }
    match serde_json::to_vec(store) {
        Ok(bytes) => {
            if let Err(error) = codexbar::secure_file::atomic_write(&path, &bytes) {
                tracing::warn!("failed to persist usage history: {error}");
            }
        }
        Err(error) => tracing::warn!("failed to serialize usage history: {error}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes_window_labels_for_stable_series() {
        assert_eq!(normalize_id("Session (5h)"), "session-5h");
        assert_eq!(normalize_id(" API "), "api");
    }

    fn window_snapshot(used_percent: f64) -> RateWindowSnapshot {
        RateWindowSnapshot {
            used_percent,
            remaining_percent: 100.0 - used_percent,
            window_minutes: None,
            resets_at: None,
            reset_description: None,
            is_exhausted: false,
            reserve_percent: None,
            reserve_description: None,
            reserve_will_last_to_reset: false,
            reserve_eta_seconds: None,
        }
    }

    #[test]
    fn a_named_tertiary_window_charts_under_its_own_name() {
        // OpenCode Go's third window is Monthly, not Cursor's API.
        let mut snapshot = ProviderUsageSnapshot {
            provider_id: "opencodego".into(),
            display_name: "OpenCode Go".into(),
            primary: window_snapshot(34.0),
            primary_label: Some("Rolling (5h)".into()),
            secondary: Some(window_snapshot(32.0)),
            secondary_label: Some("Weekly".into()),
            model_specific: None,
            tertiary: Some(window_snapshot(57.0)),
            tertiary_label: Some("Monthly".into()),
            extra_rate_windows: Vec::new(),
            inactive_rate_windows: Vec::new(),
            promo_signals: Vec::new(),
            reset_credits_available: None,
            cost: None,
            plan_name: None,
            account_email: None,
            source_label: "web".into(),
            updated_at: "2026-08-04T00:00:00Z".into(),
            error: None,
            pace: None,
            account_organization: None,
            tray_status_label: None,
            account_id: None,
            account_label: None,
            account_tint: None,
            fetch_duration_ms: None,
            wayfinder_usage: None,
        };

        let windows = snapshot_windows(&snapshot);
        let named: Vec<_> = windows
            .iter()
            .map(|window| (window.id.as_str(), window.label.as_str()))
            .collect();
        assert_eq!(
            named,
            vec![
                ("rolling-5h", "Rolling (5h)"),
                ("weekly", "Weekly"),
                ("monthly", "Monthly"),
            ]
        );

        // An unnamed tertiary keeps the historical fallback.
        snapshot.tertiary_label = None;
        let windows = snapshot_windows(&snapshot);
        assert_eq!(windows[2].id, "api");
    }

    #[test]
    fn relabelled_opencode_go_windows_keep_one_series() {
        // Recorded under the pre-rename ids; charting them as separate series
        // would break the retained 30 days into two lines each.
        let mut store = UsageHistoryStore {
            version: 2,
            series: HashMap::new(),
        };
        store.series.insert(
            scope_key("opencodego", None, None, None),
            vec![UsageHistoryPoint {
                recorded_at: "2026-08-01T00:00:00Z".into(),
                windows: vec![
                    UsageHistoryWindow {
                        id: "rolling".into(),
                        label: "Rolling".into(),
                        used_percent: 34.0,
                    },
                    UsageHistoryWindow {
                        id: "weekly".into(),
                        label: "Weekly".into(),
                        used_percent: 32.0,
                    },
                    UsageHistoryWindow {
                        id: "api".into(),
                        label: "API".into(),
                        used_percent: 57.0,
                    },
                ],
            }],
        );
        let untouched = vec![UsageHistoryPoint {
            recorded_at: "2026-08-01T00:00:00Z".into(),
            windows: vec![UsageHistoryWindow {
                id: "api".into(),
                label: "API".into(),
                used_percent: 12.0,
            }],
        }];
        store
            .series
            .insert(scope_key("cursor", None, None, None), untouched.clone());

        migrate_store(&mut store);

        let migrated = &store.series[&scope_key("opencodego", None, None, None)][0].windows;
        assert_eq!(
            migrated
                .iter()
                .map(|window| (window.id.as_str(), window.label.as_str()))
                .collect::<Vec<_>>(),
            vec![
                ("rolling-5h", "Rolling (5h)"),
                ("weekly", "Weekly"),
                ("monthly", "Monthly"),
            ]
        );
        // Cursor's API window is a real API allowance and must not be renamed.
        assert_eq!(
            store.series[&scope_key("cursor", None, None, None)],
            untouched
        );
        assert_eq!(store.version, STORE_VERSION);
    }

    fn series_with(
        entries: &[(&str, Option<&str>, &str)],
    ) -> HashMap<String, Vec<UsageHistoryPoint>> {
        let mut series = HashMap::new();
        for (provider, email, recorded_at) in entries {
            series.insert(
                scope_key(provider, *email, None, None),
                vec![UsageHistoryPoint {
                    recorded_at: recorded_at.to_string(),
                    windows: Vec::new(),
                }],
            );
        }
        series
    }

    #[test]
    fn switching_accounts_does_not_chart_the_previous_seat() {
        // The personal seat has history; the work seat is brand new. Charting
        // personal's data under work's label is the leak this guards.
        let series = series_with(&[(
            "codex",
            Some("personal@example.com"),
            "2026-07-21T00:00:00Z",
        )]);

        let selected = select_series(&series, "codex", Some("work@example.com"), None, None);

        assert!(selected.is_none(), "got another account's series");
    }

    #[test]
    fn an_identified_read_still_finds_the_same_account_recorded_anonymously() {
        // A source that reports no email wrote to the anonymous scope; that is
        // the same person, so bridging to it is correct.
        let series = series_with(&[("codex", None, "2026-07-21T00:00:00Z")]);

        let selected = select_series(&series, "codex", Some("person@example.com"), None, None);

        assert!(selected.is_some());
    }

    #[test]
    fn an_exact_account_match_wins_over_the_anonymous_series() {
        let series = series_with(&[
            ("codex", None, "2026-07-21T00:00:00Z"),
            ("codex", Some("person@example.com"), "2026-07-20T00:00:00Z"),
        ]);

        let selected =
            select_series(&series, "codex", Some("person@example.com"), None, None).unwrap();

        // Freshness must not override identity.
        assert_eq!(selected[0].recorded_at, "2026-07-20T00:00:00Z");
    }

    #[test]
    fn another_providers_series_is_never_selected() {
        let series = series_with(&[("claude", Some("person@example.com"), "2026-07-21T00:00:00Z")]);

        assert!(select_series(&series, "codex", Some("person@example.com"), None, None).is_none());
        assert!(select_series(&series, "codex", None, None, None).is_none());
    }

    #[test]
    fn account_scope_does_not_persist_the_email() {
        let key = scope_key("cursor", Some("Person@Example.com"), None, None);
        assert!(key.starts_with("cursor:"));
        assert!(!key.contains("person"));
        assert_eq!(
            key,
            scope_key("cursor", Some("person@example.com"), None, None)
        );
    }

    #[test]
    fn stable_account_id_separates_seats_that_share_an_email() {
        let personal = scope_key(
            "codex",
            Some("shared@example.com"),
            Some("acct-personal"),
            None,
        );
        let work = scope_key("codex", Some("shared@example.com"), Some("acct-work"), None);
        assert_ne!(personal, work);

        let mut series = HashMap::new();
        series.insert(personal, Vec::new());
        assert!(
            select_series(
                &series,
                "codex",
                Some("shared@example.com"),
                Some("acct-work"),
                None,
            )
            .is_none()
        );
    }

    #[test]
    fn organization_only_history_is_read_from_its_recorded_scope() {
        let mut series = HashMap::new();
        let organization_key = scope_key("openai", None, None, Some("org-work"));
        series.insert(
            organization_key,
            vec![UsageHistoryPoint {
                recorded_at: "2026-07-21T00:00:00Z".into(),
                windows: Vec::new(),
            }],
        );

        let selected = select_series(&series, "openai", None, None, Some("org-work"));

        assert!(selected.is_some());
        assert!(select_series(&series, "openai", None, None, Some("org-other")).is_none());
    }

    #[test]
    fn cursor_history_hides_promotional_but_keeps_real_on_demand_usage() {
        let points = vec![UsageHistoryPoint {
            recorded_at: "2026-07-14T10:00:00Z".into(),
            windows: vec![
                UsageHistoryWindow {
                    id: "plan".into(),
                    label: "Plan".into(),
                    used_percent: 50.0,
                },
                UsageHistoryWindow {
                    id: "promotional".into(),
                    label: "Promotional".into(),
                    used_percent: 0.0,
                },
                UsageHistoryWindow {
                    id: "cursor-on-demand".into(),
                    label: "On-demand".into(),
                    used_percent: 56.0,
                },
            ],
        }];

        let visible = visible_history("cursor", &points);

        assert_eq!(visible[0].windows.len(), 2);
        assert_eq!(visible[0].windows[0].id, "plan");
        assert_eq!(visible[0].windows[1].id, "cursor-on-demand");
        assert_eq!(visible[0].windows[1].used_percent, 56.0);
    }
}
