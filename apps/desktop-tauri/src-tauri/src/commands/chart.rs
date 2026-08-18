//! Provider chart data commands and DTOs.
//!
//! Cost history comes from the shared JSONL cost scanner and is available for
//! every provider. Credits history + usage breakdowns currently only apply to
//! the Codex / OpenAI dashboard cache and require an `account_email` to scope
//! reads to the right cached bundle.

use chrono::{DateTime, Datelike, Local, LocalResult, NaiveDate, TimeZone, Timelike, Utc};
use codexbar::core::OpenAIDashboardCacheStore;
use codexbar::cost_scanner::{
    CostScanner, CostSummary, CostUsageReport, CurrentUsageWindow, get_cost_usage_report,
    get_cost_usage_report_hourly, get_cost_usage_report_scoped, get_cost_usage_report_with_windows,
};
use codexbar::locale::{self, LocaleKey};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{
    Arc, Mutex, OnceLock,
    atomic::{AtomicBool, Ordering},
};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

const LOCAL_USAGE_TTL: Duration = Duration::from_secs(30);
const CHART_CACHE_TTL: Duration = Duration::from_secs(5 * 60);
// Version 9: account scope is part of every chart cache key. Older entries can
// conflate an unresolved account with the machine-wide scan.
const CHART_CACHE_VERSION: u8 = 9;
// Cache keys embed the live reset window, so every provider reset mints a new
// key and strands the old one. `CHART_CACHE_TTL` only schedules a rebuild of
// the *same* key, so without a bound the file grows for the life of the install
// (SBS-887). A rolled window is never read again; keep entries only long enough
// to survive a reset the user did not open Charts for.
//
// Two days covers roughly ten Claude/Codex session rolls, so a window a user
// did not open Charts for still survives, while a genuinely dead one goes
// quickly. Age is what retires entries here. A week let enough still-live keys
// pile up that the count backstop below did the eviction instead, and that
// evicts by refresh time rather than by whether the window is still current.
const CHART_CACHE_MAX_ENTRY_AGE: Duration = Duration::from_secs(2 * 24 * 60 * 60);
// Pure backstop against pathological churn, not the primary mechanism. Derived
// from the key shapes that actually churn, rather than picked:
//
// * Only two surfaces send usage windows. Charts sends account scope plus a
//   real source label; Compare sends machine-wide identity with source
//   "unknown". They mint different keys for the same provider. MenuCard and
//   provider detail send no windows at all, so their keys are stable and do not
//   churn with resets.
// * Only Claude and Codex roll on the 5-hour cadence (~5 rolls/day); both of
//   their windows live in one key, so it is 5 new keys per provider per day per
//   distinct scope. Grok only carries a weekly window, and the remaining chart
//   providers carry none.
// * Compare is machine-wide, so it is 2 providers x 5 rolls = 10 keys/day
//   regardless of account count. Charts is per account: 2 x 5 x accounts.
//
// So a machine with N accounts mints ~10 + 10N windowed keys/day, and the
// two-day age bound holds ~20 + 20N of them, plus a couple dozen stable
// empty-window keys. That is ~60 live entries at 2 accounts and ~100 at 3, so
// 256 leaves room for roughly eight accounts before the count can bind at all.
// If it ever does, evicting the least recently refreshed is the least-bad
// choice available.
const CHART_CACHE_MAX_ENTRIES: usize = 256;

/// A single (date, value) point for cost or credits history charts.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DailyCostPoint {
    pub date: String,
    pub value: f64,
}

/// A single service's usage within a day for the stacked usage breakdown chart.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ServiceUsagePoint {
    pub service: String,
    pub credits_used: f64,
}

/// One day's stacked usage breakdown.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DailyUsageBreakdown {
    pub day: String,
    pub services: Vec<ServiceUsagePoint>,
    pub total_credits_used: f64,
}

/// Real local usage summary from Codex / Claude log files.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderLocalUsageSummary {
    pub today_cost: Option<f64>,
    #[serde(default)]
    pub last_session_cost: Option<f64>,
    #[serde(default)]
    pub last_session_tokens: Option<u64>,
    #[serde(default)]
    pub last_session_token_breakdown: Option<LocalTokenBreakdown>,
    #[serde(default)]
    pub seven_day_cost: Option<f64>,
    #[serde(default)]
    pub seven_day_tokens: Option<u64>,
    #[serde(default)]
    pub seven_day_token_breakdown: Option<LocalTokenBreakdown>,
    /// Model tokens with a canonical price over the last 7 calendar days.
    #[serde(default)]
    pub seven_day_priced_tokens: u64,
    /// All model tokens (priced + unpriced) over the last 7 calendar days.
    #[serde(default)]
    pub seven_day_total_model_tokens: u64,
    pub thirty_day_cost: Option<f64>,
    pub thirty_day_tokens: Option<u64>,
    #[serde(default)]
    pub thirty_day_token_breakdown: Option<LocalTokenBreakdown>,
    /// Model tokens with a canonical price over the last 30 calendar days.
    #[serde(default)]
    pub thirty_day_priced_tokens: u64,
    /// All model tokens (priced + unpriced) over the last 30 calendar days.
    #[serde(default)]
    pub thirty_day_total_model_tokens: u64,
    #[serde(default)]
    pub current_windows: Vec<LocalUsageWindowSummary>,
    #[serde(default)]
    pub comparison_periods: Vec<LocalUsageComparisonPeriod>,
    /// Legacy alias retained for older UI surfaces. This now means the latest
    /// transcript/session, rather than today's aggregate.
    pub latest_tokens: Option<u64>,
    pub top_model: Option<String>,
    /// Per-model spend over the 30-day period, sorted by cost then tokens.
    /// Priced and unpriced models are both included.
    #[serde(default)]
    pub model_breakdown: Vec<LocalModelCost>,
    /// Per-reasoning-effort spend over the 30-day period (Codex only; empty
    /// for providers without an effort tier). Sorted by cost then tokens.
    #[serde(default)]
    pub effort_breakdown: Vec<LocalEffortCost>,
    /// Plans observed in local logs for the 30-day period, largest first.
    /// More than one entry means the total is not account-scoped; it spans
    /// several plans, which may or may not be several accounts.
    #[serde(default)]
    pub plan_breakdown: Vec<LocalPlanUsage>,
    /// Per-project/repo spend over the 30-day period, sorted by cost then
    /// tokens. Priced and unpriced projects are both included.
    #[serde(default)]
    pub project_breakdown: Vec<LocalProjectCost>,
    pub estimate_note: String,
    pub token_cost_updated_at_ms: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LocalUsageWindowRequest {
    pub id: String,
    pub label: String,
    pub starts_at: String,
    pub ends_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LocalUsageWindowSummary {
    pub id: String,
    pub label: String,
    pub starts_at: String,
    pub ends_at: String,
    pub tokens: u64,
    pub token_breakdown: LocalTokenBreakdown,
    /// Estimated API-value dollars from priced models in this reset window.
    /// `None` when there is no priced activity (unpriced models stay excluded).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cost: Option<f64>,
    /// Model tokens with a canonical price (pricing-coverage numerator).
    #[serde(default)]
    pub priced_tokens: u64,
    /// All model tokens (priced + unpriced) — pricing-coverage denominator.
    #[serde(default)]
    pub total_model_tokens: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LocalUsageComparisonPeriod {
    pub id: String,
    pub label: String,
    pub current_tokens: u64,
    pub current_breakdown: LocalTokenBreakdown,
    pub previous_tokens: u64,
    pub previous_breakdown: LocalTokenBreakdown,
}

#[derive(Debug, Clone)]
struct ComparisonPeriodSpec {
    id: &'static str,
    label: &'static str,
    current_window_id: String,
    previous_window_id: String,
}

/// Shared rolling windows for Compare, plus the preceding window of the same
/// length so each period can report a change against its own recent past.
///
/// These deliberately ignore provider reset boundaries: Codex and Claude reset
/// on different clocks, so an identical window ending now is the only span that
/// compares the two fairly. The end is snapped to the minute because a raw
/// `now` would move every call and defeat the chart cache.
fn comparison_period_specs(
    now: DateTime<Utc>,
) -> (Vec<ComparisonPeriodSpec>, Vec<CurrentUsageWindow>) {
    let now = now
        .with_second(0)
        .and_then(|value| value.with_nanosecond(0))
        .unwrap_or(now);
    let periods = [
        ("five-hours", "Last 5 hours", chrono::Duration::hours(5)),
        ("seven-days", "Last 7 days", chrono::Duration::days(7)),
    ];
    let mut specs = Vec::with_capacity(periods.len());
    let mut windows = Vec::with_capacity(periods.len() * 2);
    for (id, label, duration) in periods {
        let current_window_id = format!("compare-{id}-current");
        let previous_window_id = format!("compare-{id}-previous");
        let current_start = now - duration;
        let previous_start = current_start - duration;
        windows.push(CurrentUsageWindow {
            id: current_window_id.clone(),
            starts_at: current_start,
            ends_at: now,
        });
        windows.push(CurrentUsageWindow {
            id: previous_window_id.clone(),
            starts_at: previous_start,
            ends_at: current_start,
        });
        specs.push(ComparisonPeriodSpec {
            id,
            label,
            current_window_id,
            previous_window_id,
        });
    }
    (specs, windows)
}

/// Provider-normalized token categories. Codex reports cached input as a
/// subset of input, while Claude reports cache reads and writes separately;
/// this shape makes the frontend comparison consistent.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct LocalTokenBreakdown {
    pub processed_tokens: u64,
    pub fresh_input_tokens: u64,
    pub output_tokens: u64,
    pub cache_read_tokens: u64,
    pub cache_write_tokens: u64,
    /// Reasoning tokens when the provider reports them (Grok). Not added into
    /// `processed_tokens` because they are often already counted in output.
    #[serde(default)]
    pub reasoning_tokens: u64,
}

/// Per-model local spend for a period. `cost` is `None` for models with no
/// canonical price (their tokens still count, but no dollars are fabricated).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct LocalModelCost {
    pub model: String,
    pub cost: Option<f64>,
    pub tokens: u64,
    /// Cache-read share of processed tokens (0–100), when any tokens exist.
    pub cache_read_percent: Option<f64>,
    /// Estimated USD per usage record, when cost and calls are both present.
    pub cost_per_call: Option<f64>,
    /// Output tokens per usage record, when calls > 0.
    pub output_tokens_per_call: Option<f64>,
    pub calls: u64,
}

/// Per-reasoning-effort local spend for a period (Codex only: "high"/"xhigh"/
/// "medium"/"unknown"). `cost` is `None` when the tier's models are unpriced.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct LocalEffortCost {
    pub effort: String,
    pub cost: Option<f64>,
    pub tokens: u64,
}

/// Locally observed activity attributed to one subscription plan.
///
/// Local logs carry no account identity, so a machine's activity can span
/// several accounts with no way to tell them apart. The plan is the only proxy
/// Codex emits, and it is imperfect: two accounts on the same plan look
/// identical, and one account changing plans looks like two. It exists so the
/// UI can disclose that a total is not account-scoped, never to silently
/// filter one out and never to assert how many accounts produced it.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct LocalPlanUsage {
    pub plan: String,
    pub tokens: u64,
}

/// Per-project/repo local spend for a period (basename of the session cwd).
/// `cost` is `None` when the project's models are unpriced.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct LocalProjectCost {
    pub project: String,
    pub cost: Option<f64>,
    pub tokens: u64,
}

/// One provider's local usage for a single period, for the aggregate
/// API-value card. Dollars are token-derived "estimated API value", never a
/// bill; unpriced models contribute tokens but no dollars.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct LocalApiValuePeriod {
    /// Estimated API value in USD (priced models only).
    pub api_value_usd: f64,
    /// Processed tokens (fresh input + output + cache read/write).
    pub tokens: u64,
    /// Model tokens (input + output) that have a canonical price.
    pub priced_tokens: u64,
    /// All model tokens (priced + unpriced) — the pricing-coverage denominator.
    pub total_tokens: u64,
    /// Whether the provider had any source data in this period. A provider with
    /// no data is omitted from the card rather than counted as zero.
    pub has_data: bool,
}

/// One provider's local usage across the card's periods.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct LocalApiValueProvider {
    pub provider_id: String,
    pub today: LocalApiValuePeriod,
    pub yesterday: LocalApiValuePeriod,
    pub thirty_days: LocalApiValuePeriod,
    /// Calendar days [today-60, today-30) for dollar period-over-period on 30d.
    pub prior_thirty_days: LocalApiValuePeriod,
    /// Optional inclusive local-calendar range from `get_local_api_value_totals`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub custom: Option<LocalApiValuePeriod>,
    /// Last seven local calendar days, oldest first, today last.
    #[serde(default)]
    pub last_seven_days: Vec<LocalApiValueDay>,
    /// Scanned local calendar days (oldest first). Used for custom ranges and
    /// trends so the card never depends only on a single window key.
    #[serde(default)]
    pub daily_series: Vec<LocalApiValueDay>,
}

/// One local calendar day of estimated API value, for the card's trend.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct LocalApiValueDay {
    /// Local calendar date as `YYYY-MM-DD`.
    pub date: String,
    pub api_value_usd: f64,
    pub tokens: u64,
}

/// Full chart data bundle for one provider.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderChartData {
    pub provider_id: String,
    pub cost_history: Vec<DailyCostPoint>,
    pub credits_history: Vec<DailyCostPoint>,
    pub usage_breakdown: Vec<DailyUsageBreakdown>,
    pub local_usage: Option<ProviderLocalUsageSummary>,
    #[serde(default)]
    pub local_usage_scope: LocalUsageScope,
    #[serde(default)]
    pub quota_history: Vec<crate::usage_history::UsageHistoryPoint>,
}

/// Scope used for local transcript-derived usage in this response.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum LocalUsageScope {
    #[default]
    MachineWide,
    Account,
    UnresolvedAccount,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum ChartAccountScope {
    MachineWide,
    Account {
        account_id: String,
        config_dir: PathBuf,
    },
    UnresolvedAccount {
        account_id: String,
    },
}

impl ChartAccountScope {
    fn kind(&self) -> LocalUsageScope {
        match self {
            Self::MachineWide => LocalUsageScope::MachineWide,
            Self::Account { .. } => LocalUsageScope::Account,
            Self::UnresolvedAccount { .. } => LocalUsageScope::UnresolvedAccount,
        }
    }

    fn cache_identity(
        &self,
        _account_email: Option<&str>,
        _account_organization: Option<&str>,
    ) -> String {
        match self {
            Self::Account { account_id, .. } => {
                format!("account:{}", account_id.trim().to_ascii_lowercase())
            }
            Self::UnresolvedAccount { account_id } => {
                format!("unresolved:{}", account_id.trim().to_ascii_lowercase())
            }
            // The underlying scan covers the whole machine, so requests from a
            // provider tab and the identity-free Compare view must share it.
            Self::MachineWide => "machine".to_string(),
        }
    }
}

fn resolve_chart_account_scope(
    provider_id: &str,
    account_id: Option<&str>,
    accounts: &codexbar::core::ConfiguredAccounts,
) -> ChartAccountScope {
    let Some(account_id) = account_id.map(str::trim).filter(|value| !value.is_empty()) else {
        return ChartAccountScope::MachineWide;
    };
    let Some(provider) = codexbar::core::ProviderId::from_cli_name(provider_id) else {
        return ChartAccountScope::UnresolvedAccount {
            account_id: account_id.to_string(),
        };
    };
    match accounts.config_dir_for_account(provider, account_id) {
        Some(config_dir) => ChartAccountScope::Account {
            account_id: account_id.to_string(),
            config_dir,
        },
        None => ChartAccountScope::UnresolvedAccount {
            account_id: account_id.to_string(),
        },
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct CachedProviderChartData {
    refreshed_at_ms: i64,
    data: ProviderChartData,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct PersistedChartCache {
    #[serde(default)]
    version: u8,
    #[serde(default)]
    entries: HashMap<String, CachedProviderChartData>,
}

/// Completed quota-run snapshots for a provider/account (SOU-298), oldest first.
#[tauri::command]
pub fn get_quota_run_history(
    provider_id: String,
    account_email: Option<String>,
    account_id: Option<String>,
) -> Vec<crate::quota_run_history::QuotaRunSnapshot> {
    crate::quota_run_history::list_runs(
        &provider_id,
        account_email.as_deref(),
        account_id.as_deref(),
    )
}

/// Latest quota-run efficiency cards per window (SOU-299).
#[tauri::command]
pub fn get_quota_run_efficiency(
    provider_id: String,
    account_email: Option<String>,
    account_id: Option<String>,
) -> Vec<crate::quota_run_history::QuotaRunEfficiency> {
    crate::quota_run_history::efficiency_for_provider(
        &provider_id,
        account_email.as_deref(),
        account_id.as_deref(),
    )
}

#[tauri::command]
pub async fn get_provider_chart_data(
    provider_id: String,
    account_email: Option<String>,
    account_id: Option<String>,
    source_label: Option<String>,
    usage_windows: Option<Vec<LocalUsageWindowRequest>>,
    account_organization: Option<String>,
) -> ProviderChartData {
    let usage_windows = usage_windows.unwrap_or_default();
    // Missing account identity is the intentional machine-wide case. A supplied
    // id that no longer resolves must stay distinct: silently treating it as
    // machine-wide can display another account's activity under the stale id.
    let account_scope = resolve_chart_account_scope(
        &provider_id,
        account_id.as_deref(),
        &codexbar::core::ConfiguredAccounts::load(),
    );
    let cache_key = chart_cache_key(
        &provider_id,
        account_email.as_deref(),
        account_organization.as_deref(),
        source_label.as_deref(),
        &usage_windows,
        &account_scope,
    );
    if let Some(mut cached) = cached_chart_data(&cache_key) {
        cached.data.quota_history = crate::usage_history::provider_history(
            &provider_id,
            account_email.as_deref(),
            account_id.as_deref(),
            account_organization.as_deref(),
        );
        if current_unix_ms().saturating_sub(cached.refreshed_at_ms)
            > CHART_CACHE_TTL.as_millis() as i64
        {
            schedule_chart_cache_refresh(
                cache_key,
                provider_id,
                account_email,
                account_id,
                account_organization,
                account_scope.clone(),
                usage_windows,
            );
        }
        return cached.data;
    }

    let quota_history = crate::usage_history::provider_history(
        &provider_id,
        account_email.as_deref(),
        account_id.as_deref(),
        account_organization.as_deref(),
    );
    if !quota_history.is_empty() {
        let mut immediate =
            ProviderChartData::empty_with_scope(provider_id.clone(), account_scope.kind());
        immediate.quota_history = quota_history;
        schedule_chart_cache_refresh(
            cache_key,
            provider_id,
            account_email,
            account_id,
            account_organization,
            account_scope.clone(),
            usage_windows,
        );
        return immediate;
    }

    let fallback_provider_id = provider_id.clone();
    let fallback_scope = account_scope.kind();
    let cancel = register_chart_scan(&provider_id);
    let result = tauri::async_runtime::spawn_blocking(move || {
        build_provider_chart_data_with_cancel(
            provider_id,
            account_email,
            account_id,
            account_organization,
            account_scope,
            usage_windows,
            Some(cancel),
        )
    })
    .await
    .unwrap_or_else(|err| {
        tracing::warn!("Provider chart data worker failed: {}", err);
        ProviderChartData::empty_with_scope(fallback_provider_id, fallback_scope)
    });
    store_chart_data(cache_key, result.clone());
    result
}

fn chart_cache() -> &'static Mutex<PersistedChartCache> {
    static CACHE: OnceLock<Mutex<PersistedChartCache>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(load_persisted_chart_cache()))
}

fn active_cache_refreshes() -> &'static Mutex<HashSet<String>> {
    static ACTIVE: OnceLock<Mutex<HashSet<String>>> = OnceLock::new();
    ACTIVE.get_or_init(|| Mutex::new(HashSet::new()))
}

fn cached_chart_data(key: &str) -> Option<CachedProviderChartData> {
    chart_cache().lock().ok()?.entries.get(key).cloned()
}

fn store_chart_data(key: String, data: ProviderChartData) {
    let Ok(mut guard) = chart_cache().lock() else {
        return;
    };
    guard.version = CHART_CACHE_VERSION;
    guard.entries.insert(
        key.clone(),
        CachedProviderChartData {
            refreshed_at_ms: current_unix_ms(),
            data,
        },
    );
    prune_chart_cache(&mut guard, &key);
    persist_chart_cache(&guard);
}

/// Drops the entries a rolled reset window left behind: first anything past
/// [`CHART_CACHE_MAX_ENTRY_AGE`], then the least recently refreshed until the
/// map fits [`CHART_CACHE_MAX_ENTRIES`].
///
/// `keep` is never evicted. It is the key the caller just stored, which would
/// otherwise be a candidate on a machine whose clock moved backwards. Pass `""`
/// when there is no such key; no real key is empty.
fn prune_chart_cache(cache: &mut PersistedChartCache, keep: &str) {
    let now_ms = current_unix_ms();
    let max_age_ms = CHART_CACHE_MAX_ENTRY_AGE.as_millis() as i64;
    cache.entries.retain(|key, entry| {
        key == keep || now_ms.saturating_sub(entry.refreshed_at_ms) <= max_age_ms
    });

    if cache.entries.len() <= CHART_CACHE_MAX_ENTRIES {
        return;
    }

    let mut by_age: Vec<(&String, i64)> = cache
        .entries
        .iter()
        .filter(|(key, _)| key.as_str() != keep)
        .map(|(key, entry)| (key, entry.refreshed_at_ms))
        .collect();
    // Oldest first. The key breaks ties so eviction does not depend on
    // HashMap iteration order.
    by_age.sort_by(|a, b| a.1.cmp(&b.1).then_with(|| a.0.cmp(b.0)));

    let excess = cache.entries.len() - CHART_CACHE_MAX_ENTRIES;
    let evict: Vec<String> = by_age
        .into_iter()
        .take(excess)
        .map(|(key, _)| key.clone())
        .collect();
    for key in evict {
        cache.entries.remove(&key);
    }
}

fn schedule_chart_cache_refresh(
    key: String,
    provider_id: String,
    account_email: Option<String>,
    account_id: Option<String>,
    account_organization: Option<String>,
    account_scope: ChartAccountScope,
    usage_windows: Vec<LocalUsageWindowRequest>,
) {
    let Ok(mut active) = active_cache_refreshes().lock() else {
        return;
    };
    if !active.insert(key.clone()) {
        return;
    }
    drop(active);

    tauri::async_runtime::spawn(async move {
        let refresh_key = key.clone();
        let refreshed = tauri::async_runtime::spawn_blocking(move || {
            build_provider_chart_data_with_cancel(
                provider_id,
                account_email,
                account_id,
                account_organization,
                account_scope,
                usage_windows,
                None,
            )
        })
        .await;
        match refreshed {
            Ok(data) => store_chart_data(key, data),
            Err(error) => tracing::warn!("Provider chart cache refresh failed: {error}"),
        }
        if let Ok(mut active) = active_cache_refreshes().lock() {
            active.remove(&refresh_key);
        }
    });
}

fn chart_cache_key(
    provider_id: &str,
    account_email: Option<&str>,
    account_organization: Option<&str>,
    source_label: Option<&str>,
    usage_windows: &[LocalUsageWindowRequest],
    account_scope: &ChartAccountScope,
) -> String {
    let identity = account_scope.cache_identity(account_email, account_organization);
    let windows = usage_windows
        .iter()
        .map(|window| format!("{}:{}:{}", window.id, window.starts_at, window.ends_at))
        .collect::<Vec<_>>()
        .join("|");
    let source = source_label
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or("unknown")
        .to_ascii_lowercase();
    format!(
        "{}:{:016x}:{:016x}:{:016x}",
        provider_id.to_ascii_lowercase(),
        fnv1a64(identity.as_bytes()),
        fnv1a64(source.as_bytes()),
        fnv1a64(windows.as_bytes()),
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

fn chart_cache_path() -> Option<PathBuf> {
    codexbar::settings::Settings::settings_path().and_then(|path| {
        path.parent()
            .map(|parent| parent.join("chart-data-cache.json"))
    })
}

fn load_persisted_chart_cache() -> PersistedChartCache {
    let Some(path) = chart_cache_path() else {
        return PersistedChartCache::default();
    };
    load_chart_cache_from(&path)
}

/// Reads the cache file, prunes it, and heals the file on disk when what it
/// held is no longer usable.
///
/// Takes the path so tests can exercise the whole disk round trip; the caller
/// resolves the real user config directory.
///
/// [`store_chart_data`] is the only other write site and it does not run until
/// someone opens a chart. So anything left here survives to the next launch and
/// is re-read in full every time. That covers three cases: a file that shrank
/// under the bounds, a file written by a superseded [`CHART_CACHE_VERSION`]
/// (whose entries are dropped in memory but would otherwise stay on disk), and
/// a file that no longer parses.
fn load_chart_cache_from(path: &Path) -> PersistedChartCache {
    let bytes = fs::read(path).ok();
    let parsed: Option<PersistedChartCache> = bytes
        .as_ref()
        .and_then(|bytes| serde_json::from_slice(bytes).ok());
    let unusable = match parsed.as_ref() {
        Some(cache) => cache.version != CHART_CACHE_VERSION,
        // Bytes we could read but not parse are dead weight. A file we could
        // not read at all is not ours to overwrite.
        None => bytes.is_some(),
    };
    let cache = parsed
        .filter(|cache: &PersistedChartCache| cache.version == CHART_CACHE_VERSION)
        .unwrap_or_default();

    let (cache, shrank) = prune_loaded_chart_cache(cache);
    if shrank || unusable {
        write_chart_cache(path, &cache);
    }
    cache
}

/// Prunes a cache just read from disk, reporting whether it shrank so the
/// caller knows to write it back.
fn prune_loaded_chart_cache(mut cache: PersistedChartCache) -> (PersistedChartCache, bool) {
    let before = cache.entries.len();
    prune_chart_cache(&mut cache, "");
    let shrank = cache.entries.len() != before;
    // Always stamp the version: the caller may write this back because the file
    // it came from was a superseded version, not because anything was pruned.
    cache.version = CHART_CACHE_VERSION;
    (cache, shrank)
}

fn persist_chart_cache(cache: &PersistedChartCache) {
    let Some(path) = chart_cache_path() else {
        return;
    };
    write_chart_cache(&path, cache);
}

fn write_chart_cache(path: &Path, cache: &PersistedChartCache) {
    if let Some(parent) = path.parent()
        && let Err(error) = fs::create_dir_all(parent)
    {
        tracing::warn!("failed to create chart cache directory: {error}");
        return;
    }
    match serde_json::to_vec(cache) {
        Ok(bytes) => {
            if let Err(error) = codexbar::secure_file::atomic_write(path, &bytes) {
                tracing::warn!("failed to persist chart cache: {error}");
            }
        }
        Err(error) => tracing::warn!("failed to serialize chart cache: {error}"),
    }
}

/// Providers that expose token-derived local usage for the aggregate card.
/// Inclusion is by capability, not by merely having some other dollar balance.
// Grok dollars come from session costUsdTicks (API-equivalent), same as provider Charts.
const API_VALUE_PROVIDERS: [&str; 3] = ["codex", "claude", "grok"];

/// Priced vs total model-token counts for pricing-coverage disclosure.
///
/// Unpriced models are those the scanner flagged as unknown; their tokens still
/// count toward the denominator so a partial window cannot look exact.
fn pricing_coverage_tokens(summary: &CostSummary) -> (u64, u64) {
    let total_tokens: u64 = summary
        .by_model_tokens
        .values()
        .map(|counts| counts.total())
        .sum();
    let unpriced_tokens: u64 = summary
        .unknown_models
        .iter()
        .filter_map(|model| summary.by_model_tokens.get(model))
        .map(|counts| counts.total())
        .sum();
    let priced_tokens = total_tokens.saturating_sub(unpriced_tokens);
    (priced_tokens, total_tokens)
}

/// Aggregate one period's usage for one provider into the card shape.
fn api_value_period(provider_id: &str, summary: &CostSummary) -> LocalApiValuePeriod {
    let processed = token_breakdown(provider_id, summary).processed_tokens;
    let (priced_tokens, total_tokens) = pricing_coverage_tokens(summary);
    let has_data = summary.sessions_count > 0
        || total_tokens > 0
        || processed > 0
        || summary.total_cost_usd > 0.0;
    LocalApiValuePeriod {
        api_value_usd: summary.total_cost_usd,
        tokens: processed,
        priced_tokens,
        total_tokens,
        has_data,
    }
}

/// Local-calendar midnight for `date`, as a UTC instant.
fn local_midnight_utc(date: NaiveDate) -> DateTime<Utc> {
    local_midnight_in_tz(&Local, date)
}

/// Resolve local midnight of `date` in `tz` to a UTC instant. DST-safe: a
/// fall-back (ambiguous) midnight picks the earliest instant; a spring-forward
/// (skipped) midnight advances minute by minute to the first local time that
/// actually exists, so it never mis-interprets the naive time as UTC.
fn local_midnight_in_tz<Tz: TimeZone>(tz: &Tz, date: NaiveDate) -> DateTime<Utc> {
    let mut naive = date.and_hms_opt(0, 0, 0).expect("valid midnight");
    // A DST gap is at most a couple of hours; bound the walk so a pathological
    // zone can't loop forever.
    for _ in 0..=180 {
        match tz.from_local_datetime(&naive) {
            LocalResult::Single(dt) => return dt.with_timezone(&Utc),
            LocalResult::Ambiguous(earliest, _) => return earliest.with_timezone(&Utc),
            LocalResult::None => naive += chrono::Duration::minutes(1),
        }
    }
    // Unreachable for real zones (gaps never exceed a few hours).
    Utc.from_utc_datetime(&date.and_hms_opt(0, 0, 0).expect("valid midnight"))
}

/// The user's local "yesterday" as a `[start, end)` UTC window: yesterday's
/// local midnight up to today's local midnight.
fn local_yesterday_window_utc(now: DateTime<Local>) -> (DateTime<Utc>, DateTime<Utc>) {
    let today = now.date_naive();
    let yesterday = today - chrono::Duration::days(1);
    (local_midnight_utc(yesterday), local_midnight_utc(today))
}

#[derive(Debug, Clone)]
pub(crate) struct SpendBudgetTotal {
    pub cycle_id: String,
    pub period_label: &'static str,
    pub estimated_usd: f64,
}

fn spend_budget_period_details(
    date: NaiveDate,
    period: &str,
) -> Option<(String, &'static str, NaiveDate, u32)> {
    if period == "monthly" {
        let month_start = NaiveDate::from_ymd_opt(date.year(), date.month(), 1)?;
        Some((
            format!("monthly:{:04}-{:02}", date.year(), date.month()),
            "Month to date",
            month_start,
            date.day(),
        ))
    } else {
        Some((format!("daily:{}", date.format("%F")), "Daily", date, 1))
    }
}

/// Scan the selected local-log period once per supported provider. This uses a
/// real local-calendar start rather than presenting a rolling 30-day number as
/// "monthly".
pub(crate) async fn load_spend_budget_total(
    provider_ids: Vec<String>,
    period: String,
) -> Option<SpendBudgetTotal> {
    tauri::async_runtime::spawn_blocking(move || {
        let now = Local::now();
        let date = now.date_naive();
        let (cycle_id, period_label, start_date, days) =
            spend_budget_period_details(date, &period)?;
        let start = local_midnight_utc(start_date);
        let end = now.with_timezone(&Utc);
        let window = CurrentUsageWindow {
            id: "spend-budget".to_string(),
            starts_at: start,
            ends_at: end,
        };
        let estimated_usd = provider_ids
            .iter()
            .filter_map(|provider_id| {
                get_cost_usage_report_with_windows(provider_id, days, std::slice::from_ref(&window))
            })
            .filter_map(|report| report.current_windows.get("spend-budget").cloned())
            .map(|summary| summary.total_cost_usd)
            .sum();
        Some(SpendBudgetTotal {
            cycle_id,
            period_label,
            estimated_usd,
        })
    })
    .await
    .ok()
    .flatten()
}

/// Preceding local days the spike detector compares today against. A week is
/// long enough to cover a normal work rhythm without reaching so far back that
/// a changed workload still counts as "normal".
const SPEND_ANOMALY_BASELINE_DAYS: u32 = 7;

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct SpendAnomalyReading {
    /// Local calendar day the reading is for, `YYYY-MM-DD`.
    pub day_id: String,
    pub today_usd: f64,
    /// Median of the preceding days, excluding today.
    pub baseline_usd: f64,
}

/// Today's estimated API value against the preceding week's median (SBS-279).
///
/// Scanned separately from the budget total because the two need different
/// windows: a daily budget only needs today, while the spike detector needs
/// today plus a week of history to have anything to compare against.
pub(crate) async fn load_spend_anomaly_reading(
    provider_ids: Vec<String>,
) -> Option<SpendAnomalyReading> {
    tauri::async_runtime::spawn_blocking(move || {
        let today = Local::now().date_naive();
        let daily = daily_spend_by_local_day(&provider_ids, SPEND_ANOMALY_BASELINE_DAYS + 1);
        spend_anomaly_reading(today, &daily)
    })
    .await
    .ok()
    .flatten()
}

/// Estimated API value per local calendar day, summed across providers.
fn daily_spend_by_local_day(provider_ids: &[String], days: u32) -> HashMap<String, f64> {
    let mut totals: HashMap<String, f64> = HashMap::new();
    for provider_id in provider_ids {
        let Some(report) = get_cost_usage_report(provider_id, days) else {
            continue;
        };
        for (day, cost) in &report.daily_costs {
            *totals.entry(day.clone()).or_default() += *cost;
        }
    }
    totals
}

/// Split a daily series into today and the preceding days' median.
///
/// Today is excluded from the baseline: including it would let a spike raise
/// the very bar it is measured against, which is how a naive "today vs the
/// last N days" check silently stops firing as the spike grows.
fn spend_anomaly_reading(
    today: NaiveDate,
    daily: &HashMap<String, f64>,
) -> Option<SpendAnomalyReading> {
    let day_id = today.format("%Y-%m-%d").to_string();
    let today_usd = daily.get(&day_id).copied().unwrap_or(0.0);
    let baseline_days: Vec<f64> = (1..=SPEND_ANOMALY_BASELINE_DAYS as i64)
        .map(|offset| {
            let date = (today - chrono::Duration::days(offset))
                .format("%Y-%m-%d")
                .to_string();
            daily.get(&date).copied().unwrap_or(0.0)
        })
        .collect();
    Some(SpendAnomalyReading {
        day_id,
        today_usd,
        baseline_usd: codexbar::notifications::spend_baseline_usd(baseline_days),
    })
}

/// Inclusive local-calendar custom range for Estimated API value (YYYY-MM-DD).
const API_VALUE_CUSTOM_MAX_DAYS: i64 = 366;
/// Default scan horizon.
///
/// The furthest-back period the card shows without a custom range is
/// "prior thirty days", `[today-59, today-29)`, so 60 days is everything the
/// default view can read. It used to scan 90, which on a real machine is an
/// extra gigabyte of transcripts parsed for rows nothing displays.
const API_VALUE_DEFAULT_SCAN_DAYS: u32 = 60;
/// How long a cached API-value scan is served before a refresh is scheduled.
const API_VALUE_TTL: Duration = Duration::from_secs(5 * 60);

static API_VALUE_CACHE: crate::commands::scan_cache::ScanCache<Vec<LocalApiValueProvider>> =
    crate::commands::scan_cache::ScanCache::new("api-value-cache.json", 1);

/// Delay before the launch prewarm starts, so it never competes with the first
/// provider refresh or the window's first paint.
const LOCAL_SCAN_PREWARM_DELAY: Duration = Duration::from_secs(15);

/// Rebuild the local-scan caches in the background at launch.
///
/// The first scan of a day costs tens of seconds on a machine holding gigabytes
/// of transcripts, and it used to land while the user sat watching the card.
/// Doing it at launch moves that wait off the interaction entirely.
///
/// A machine whose caches have never been written is no evidence that anyone
/// opens these cards, so it stays idle rather than scanning speculatively; the
/// first open still pays for itself once, and every one after that is warm. The
/// two scans run one after the other because each already saturates the
/// machine's cores on its own.
pub fn prewarm_local_scan_caches(app: tauri::AppHandle) {
    // Each card is judged on its own cache. Someone who only ever opens
    // Estimated API value should not have the heatmap scan saturating their
    // cores for a card they have never looked at.
    let api_value = API_VALUE_CACHE.has_entries();
    let heatmap = ACTIVITY_HEATMAP_CACHE.has_entries();
    if !api_value && !heatmap {
        return;
    }
    tauri::async_runtime::spawn(async move {
        tokio::time::sleep(LOCAL_SCAN_PREWARM_DELAY).await;
        if api_value && let Err(error) = get_local_api_value_totals(app.clone(), None, None).await {
            tracing::debug!("API-value prewarm skipped: {error}");
        }
        if heatmap && let Err(error) = get_local_activity_heatmap(app).await {
            tracing::debug!("Activity heatmap prewarm skipped: {error}");
        }
    });
}

/// Cache key for one API-value request.
///
/// The local date is part of the key because every period on this card is
/// anchored to "today". A cached bundle from yesterday is not stale, it is
/// wrong, so a new day mints a new key rather than being served while a refresh
/// runs behind it.
fn api_value_cache_key(today: NaiveDate, custom: Option<(DateTime<Utc>, DateTime<Utc>)>) -> String {
    match custom {
        Some((start, end)) => format!(
            "{}:{}:{}",
            today.format("%F"),
            start.timestamp_millis(),
            end.timestamp_millis()
        ),
        None => format!("{}:default", today.format("%F")),
    }
}

/// Run `scan` for each provider concurrently, keeping the input order.
///
/// Each provider reads a different transcript tree, so scanning them one after
/// another just added their times together. Providers with no data return
/// `None` and drop out, exactly as the sequential `filter_map` did.
///
/// A worker that panics is re-raised on this thread rather than swallowed. Both
/// commands that call this promise an error when a scan fails, and a provider
/// silently missing from a spend total looks exactly like a provider that was
/// idle.
fn scan_providers_parallel<T, F>(provider_ids: &[&'static str], scan: F) -> Vec<T>
where
    T: Send,
    F: Fn(&'static str) -> Option<T> + Sync,
{
    std::thread::scope(|scope| {
        provider_ids
            .iter()
            .map(|provider_id| scope.spawn(|| scan(provider_id)))
            .collect::<Vec<_>>()
            .into_iter()
            .filter_map(|handle| {
                handle
                    .join()
                    .unwrap_or_else(|panic| std::panic::resume_unwind(panic))
            })
            .collect()
    })
}

#[tauri::command]
pub async fn get_local_api_value_totals(
    app: tauri::AppHandle,
    since: Option<String>,
    until: Option<String>,
) -> Result<Vec<LocalApiValueProvider>, String> {
    let since = since
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string);
    let until = until
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string);
    let custom = match (since.as_deref(), until.as_deref()) {
        (None, None) => None,
        (Some(since), Some(until)) => Some(parse_api_value_custom_range(
            since,
            until,
            Local::now().date_naive(),
        )?),
        _ => {
            return Err("Custom range needs both a start and end date (YYYY-MM-DD).".to_string());
        }
    };
    // A worker panic/cancel must surface as an error, not an empty result —
    // "unavailable" and "genuinely no data" are distinct on this card.
    API_VALUE_CACHE
        .load(
            api_value_cache_key(Local::now().date_naive(), custom),
            API_VALUE_TTL,
            move || load_local_api_value_totals(Local::now(), custom),
            move || crate::events::emit_local_scan_refreshed(&app, "api-value"),
            "Unable to load local API-value totals.",
        )
        .await
}

/// Parse and validate an inclusive local-calendar custom range.
/// Returns `(start, end_exclusive)` as UTC midnights for the cost scanner.
fn parse_api_value_custom_range(
    since: &str,
    until: &str,
    today: NaiveDate,
) -> Result<(DateTime<Utc>, DateTime<Utc>), String> {
    let start = NaiveDate::parse_from_str(since.trim(), "%Y-%m-%d")
        .map_err(|_| format!("Start date must be YYYY-MM-DD (got {since:?})."))?;
    let end_inclusive = NaiveDate::parse_from_str(until.trim(), "%Y-%m-%d")
        .map_err(|_| format!("End date must be YYYY-MM-DD (got {until:?})."))?;
    if start > end_inclusive {
        return Err("Start date must be on or before the end date.".to_string());
    }
    if end_inclusive > today {
        return Err("End date cannot be in the future.".to_string());
    }
    let span_days = (end_inclusive - start).num_days() + 1;
    if span_days > API_VALUE_CUSTOM_MAX_DAYS {
        return Err(format!(
            "Custom range can span at most {API_VALUE_CUSTOM_MAX_DAYS} days."
        ));
    }
    // Inclusive end day → [start midnight, day-after-end midnight).
    Ok((
        local_midnight_utc(start),
        local_midnight_utc(end_inclusive + chrono::Duration::days(1)),
    ))
}

fn load_local_api_value_totals(
    now: DateTime<Local>,
    custom_range: Option<(DateTime<Utc>, DateTime<Utc>)>,
) -> Vec<LocalApiValueProvider> {
    let today = now.date_naive();
    let (yesterday_start, yesterday_end) = local_yesterday_window_utc(now);
    // Exact [start, end) windows so thirty-day and prior-thirty stay adjacent
    // and each spans exactly 30 calendar days (including today for "thirty").
    // [today-29, tomorrow) = today-29 … today; [today-59, today-29) = prior 30.
    let thirty_start = local_midnight_utc(today - chrono::Duration::days(29));
    let thirty_end = local_midnight_utc(today + chrono::Duration::days(1));
    let prior_start = local_midnight_utc(today - chrono::Duration::days(59));
    let scan_days = custom_range
        .map(|(start, _)| {
            let start_date = start.with_timezone(&Local).date_naive();
            let days = (today - start_date).num_days().max(0) as u32 + 1;
            days.max(API_VALUE_DEFAULT_SCAN_DAYS)
                .min(API_VALUE_CUSTOM_MAX_DAYS as u32)
        })
        .unwrap_or(API_VALUE_DEFAULT_SCAN_DAYS);
    scan_providers_parallel(&API_VALUE_PROVIDERS, |provider_id| {
        let mut windows = vec![
            CurrentUsageWindow {
                id: "yesterday".to_string(),
                starts_at: yesterday_start,
                ends_at: yesterday_end,
            },
            CurrentUsageWindow {
                id: "thirty".to_string(),
                starts_at: thirty_start,
                ends_at: thirty_end,
            },
            CurrentUsageWindow {
                id: "prior_thirty".to_string(),
                starts_at: prior_start,
                ends_at: thirty_start,
            },
        ];
        if let Some((starts_at, ends_at)) = custom_range {
            windows.push(CurrentUsageWindow {
                id: "custom".to_string(),
                starts_at,
                ends_at,
            });
        }
        // One window per local calendar day for the seven-day trend.
        for offset in 0..7i64 {
            let date = today - chrono::Duration::days(offset);
            windows.push(CurrentUsageWindow {
                id: format!("day-{offset}"),
                starts_at: local_midnight_utc(date),
                ends_at: local_midnight_utc(date + chrono::Duration::days(1)),
            });
        }
        let report = get_cost_usage_report_with_windows(provider_id, scan_days, &windows)?;
        let yesterday = report
            .current_windows
            .get("yesterday")
            .cloned()
            .unwrap_or_default();
        let thirty_days = report
            .current_windows
            .get("thirty")
            .cloned()
            .unwrap_or_default();
        let prior_thirty_days = report
            .current_windows
            .get("prior_thirty")
            .cloned()
            .unwrap_or_default();
        let daily_series = daily_series_from_report(&report.daily_costs);
        let custom = custom_range.map(|(starts_at, ends_at)| {
            let start_day = starts_at.with_timezone(&Local).date_naive();
            // ends_at is exclusive midnight; convert to inclusive local day.
            let end_day = ends_at.with_timezone(&Local).date_naive() - chrono::Duration::days(1);
            let from_window = report
                .current_windows
                .get("custom")
                .cloned()
                .map(|summary| api_value_period(provider_id, &summary))
                .unwrap_or_else(empty_api_value_period);
            let from_daily = period_from_daily_series(&daily_series, start_day, end_day);
            // Prefer the richer window (tokens + dollars). If the window is
            // empty but daily dollars exist, use daily so Custom never lies.
            if from_window.has_data {
                from_window
            } else {
                from_daily
            }
        });
        // Oldest first so the trend reads left to right, ending today.
        let last_seven_days = (0..7i64)
            .rev()
            .map(|offset| {
                let date = today - chrono::Duration::days(offset);
                let summary = report
                    .current_windows
                    .get(&format!("day-{offset}"))
                    .cloned()
                    .unwrap_or_default();
                let period = api_value_period(provider_id, &summary);
                LocalApiValueDay {
                    date: date.format("%Y-%m-%d").to_string(),
                    api_value_usd: period.api_value_usd,
                    tokens: period.tokens,
                }
            })
            .collect();
        let provider = LocalApiValueProvider {
            provider_id: provider_id.to_string(),
            today: api_value_period(provider_id, &report.today),
            yesterday: api_value_period(provider_id, &yesterday),
            thirty_days: api_value_period(provider_id, &thirty_days),
            prior_thirty_days: api_value_period(provider_id, &prior_thirty_days),
            custom,
            last_seven_days,
            daily_series,
        };
        // Omit providers with no source data in any period.
        (provider.today.has_data
            || provider.yesterday.has_data
            || provider.thirty_days.has_data
            || provider.prior_thirty_days.has_data
            || provider.custom.as_ref().is_some_and(|p| p.has_data)
            || provider
                .daily_series
                .iter()
                .any(|day| day.api_value_usd > 0.0))
        .then_some(provider)
    })
}

fn empty_api_value_period() -> LocalApiValuePeriod {
    LocalApiValuePeriod {
        api_value_usd: 0.0,
        tokens: 0,
        priced_tokens: 0,
        total_tokens: 0,
        has_data: false,
    }
}

fn daily_series_from_report(daily_costs: &[(String, f64)]) -> Vec<LocalApiValueDay> {
    let mut days: Vec<LocalApiValueDay> = daily_costs
        .iter()
        .map(|(date, api_value_usd)| LocalApiValueDay {
            date: date.clone(),
            api_value_usd: *api_value_usd,
            tokens: 0,
        })
        .collect();
    days.sort_by(|left, right| left.date.cmp(&right.date));
    days
}

/// Inclusive local-calendar sum from the daily dollar series.
fn period_from_daily_series(
    daily_series: &[LocalApiValueDay],
    start: NaiveDate,
    end_inclusive: NaiveDate,
) -> LocalApiValuePeriod {
    if start > end_inclusive {
        return empty_api_value_period();
    }
    let mut api_value_usd = 0.0;
    let mut tokens: u64 = 0;
    let mut has_data = false;
    for day in daily_series {
        let Ok(date) = NaiveDate::parse_from_str(&day.date, "%Y-%m-%d") else {
            continue;
        };
        if date < start || date > end_inclusive {
            continue;
        }
        if day.api_value_usd > 0.0 || day.tokens > 0 {
            has_data = true;
        }
        api_value_usd += day.api_value_usd;
        tokens = tokens.saturating_add(day.tokens);
    }
    LocalApiValuePeriod {
        api_value_usd,
        tokens,
        priced_tokens: tokens,
        total_tokens: tokens,
        has_data: has_data || api_value_usd > 0.0,
    }
}

/// One provider's activity in a single local clock-hour.
///
/// Hours with no activity are omitted rather than sent as zeros: a 30-day grid
/// is 720 cells per provider, and the UI fills the gaps itself.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ActivityHourPoint {
    pub provider_id: String,
    /// Local calendar date, `YYYY-MM-DD`.
    pub date: String,
    /// Local hour of day, 0-23.
    pub hour: u32,
    /// Estimated API value in USD from priced models in this hour. Unpriced
    /// models contribute tokens but no dollars, exactly as elsewhere.
    pub api_value_usd: f64,
    /// Provider-normalized processed tokens.
    pub tokens: u64,
    /// Usage records in this hour. Dollars can be dominated by one big call,
    /// so this is the honest answer to "when am I actually working".
    pub calls: u64,
}

/// Local activity by calendar day and clock hour, for the heatmap card.
///
/// Everything here is derived from transcript timestamps already on disk. No
/// new data is collected and nothing leaves the machine.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ActivityHeatmap {
    /// Every local calendar day in range, oldest first, including empty days,
    /// so the calendar renders a continuous strip.
    pub days: Vec<String>,
    /// Providers that contributed at least one hour, for the filter chips.
    pub provider_ids: Vec<String>,
    /// Non-empty hour buckets, oldest first.
    pub hours: Vec<ActivityHourPoint>,
    /// UTC offset of the clock these buckets use, for example `UTC-07:00`.
    /// Shown so a heatmap read on a different machine is not misread.
    pub timezone_label: String,
}

/// Days of history the heatmap covers. Matches the cost scanner's own 30-day
/// retention, so the card never claims a range the underlying scan cannot fill.
const ACTIVITY_HEATMAP_DAYS: u32 = 30;
const ACTIVITY_HEATMAP_TTL: Duration = Duration::from_secs(5 * 60);

static ACTIVITY_HEATMAP_CACHE: crate::commands::scan_cache::ScanCache<ActivityHeatmap> =
    crate::commands::scan_cache::ScanCache::new("activity-heatmap-cache.json", 1);

/// Local activity by day and hour across every provider with local logs.
///
/// This is a second pass over the same transcripts the API-value card reads, so
/// results are cached for the same five minutes the chart caches use, and the
/// cache is kept on disk: an app restart used to mean waiting out a full
/// multi-gigabyte scan before the card drew anything. A cached grid older than
/// the TTL still paints, with the refresh running behind it.
///
/// The key carries the local date because the grid's last column is today.
#[tauri::command]
pub async fn get_local_activity_heatmap(app: tauri::AppHandle) -> Result<ActivityHeatmap, String> {
    // A worker panic must surface as an error: "unavailable" and "no activity"
    // look identical on a heatmap otherwise.
    ACTIVITY_HEATMAP_CACHE
        .load(
            Local::now().date_naive().format("%F").to_string(),
            ACTIVITY_HEATMAP_TTL,
            || load_activity_heatmap(Local::now()),
            move || crate::events::emit_local_scan_refreshed(&app, "activity-heatmap"),
            "Unable to read local activity.",
        )
        .await
}

/// The day axis: `ACTIVITY_HEATMAP_DAYS` local calendar days ending on `today`,
/// oldest first. Built from the calendar rather than from the scan, so an idle
/// machine still renders a full grid instead of collapsing to nothing.
fn activity_heatmap_days(today: NaiveDate) -> Vec<String> {
    (0..ACTIVITY_HEATMAP_DAYS as i64)
        .rev()
        .map(|offset| {
            (today - chrono::Duration::days(offset))
                .format("%Y-%m-%d")
                .to_string()
        })
        .collect()
}

/// Keep only the hours that fall on the grid's own day axis.
///
/// The report a scan produces is not required to stop where the grid does, and
/// today it does not have to: an hour from a day the calendar strip never draws
/// would still land in the weekday-by-hour view, which reads as activity on a
/// day the user cannot see. Filtering here keeps the two views of the same data
/// answering the same question, whatever window the report was built for.
fn heatmap_hours_for_days(
    provider_id: &str,
    report: &CostUsageReport,
    days: &[String],
) -> Vec<ActivityHourPoint> {
    let axis: HashSet<&str> = days.iter().map(String::as_str).collect();
    activity_hours_for_provider(provider_id, report)
        .into_iter()
        .filter(|point| axis.contains(point.date.as_str()))
        .collect()
}

/// Hour rows for one provider's report.
fn activity_hours_for_provider(
    provider_id: &str,
    report: &CostUsageReport,
) -> Vec<ActivityHourPoint> {
    report
        .hourly_activity
        .iter()
        .filter_map(|point| {
            let tokens = point.summary.normalized_tokens(provider_id).processed();
            let calls: u64 = point
                .summary
                .by_model_tokens
                .values()
                .map(|counts| counts.calls)
                .sum();
            // An hour that produced no tokens, dollars, or calls is not
            // activity; keeping it would darken the grid for nothing.
            (tokens > 0 || calls > 0 || point.summary.total_cost_usd > 0.0).then(|| {
                ActivityHourPoint {
                    provider_id: provider_id.to_string(),
                    date: point.date.format("%Y-%m-%d").to_string(),
                    hour: point.hour,
                    api_value_usd: point.summary.total_cost_usd,
                    tokens,
                    calls,
                }
            })
        })
        .collect()
}

/// Merge each provider's rows into one timeline.
///
/// Providers are scanned one after another, so the concatenation is only
/// sorted within a provider; the card reads it as a single series. A provider
/// that contributed no hours is left out of `provider_ids` so the filter chips
/// never offer a control that can only ever blank the grid.
fn assemble_activity_heatmap(
    days: Vec<String>,
    per_provider: Vec<Vec<ActivityHourPoint>>,
    timezone_label: String,
) -> ActivityHeatmap {
    let mut provider_ids = Vec::new();
    let mut hours = Vec::new();
    for rows in per_provider {
        if let Some(first) = rows.first() {
            provider_ids.push(first.provider_id.clone());
        }
        hours.extend(rows);
    }
    hours.sort_by(|left, right| {
        (&left.date, left.hour, &left.provider_id).cmp(&(
            &right.date,
            right.hour,
            &right.provider_id,
        ))
    });

    ActivityHeatmap {
        days,
        provider_ids,
        hours,
        timezone_label,
    }
}

fn load_activity_heatmap<Tz: TimeZone>(now: DateTime<Tz>) -> ActivityHeatmap
where
    Tz::Offset: std::fmt::Display,
{
    let days = activity_heatmap_days(now.date_naive());
    let per_provider = scan_providers_parallel(&API_VALUE_PROVIDERS, |provider_id| {
        let report = get_cost_usage_report_hourly(provider_id, ACTIVITY_HEATMAP_DAYS)?;
        Some(heatmap_hours_for_days(provider_id, &report, &days))
    });
    assemble_activity_heatmap(days.clone(), per_provider, format!("UTC{}", now.offset()))
}

/// One model's local Cursor activity. This is code-contribution activity from
/// Cursor's on-disk tracking, NOT tokens or dollars (Cursor logs neither).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct CursorModelActivityRow {
    pub model: String,
    pub contributions: u64,
    pub requests: u64,
}

/// Local Composer activity plus an honest missing-data signal.
///
/// `status` is `available`, `empty`, `unavailable`, or `unreadable`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct CursorActivitySnapshotBridge {
    pub status: String,
    pub rows: Vec<CursorModelActivityRow>,
}

/// Providers currently reporting an incident on their public status page
/// (SBS-280). Empty while the feature is off, and empty for every provider
/// that is operational or has no readable status page.
#[tauri::command]
pub async fn get_provider_incidents()
-> std::collections::HashMap<String, crate::provider_incidents::ProviderIncident> {
    // Settings::load reads from disk, so it goes to a blocking thread rather
    // than stalling the runtime every other command shares.
    let Ok(settings) =
        tauri::async_runtime::spawn_blocking(codexbar::settings::Settings::load).await
    else {
        return std::collections::HashMap::new();
    };
    crate::provider_incidents::current_incidents(&settings).await
}

#[tauri::command]
pub async fn get_cursor_model_activity() -> CursorActivitySnapshotBridge {
    tauri::async_runtime::spawn_blocking(|| {
        let snapshot = codexbar::cursor_activity::cursor_model_activity(current_unix_ms(), 30);
        CursorActivitySnapshotBridge {
            status: match snapshot.status {
                codexbar::cursor_activity::CursorActivityStatus::Available => {
                    "available".to_string()
                }
                codexbar::cursor_activity::CursorActivityStatus::Empty => "empty".to_string(),
                codexbar::cursor_activity::CursorActivityStatus::Unavailable => {
                    "unavailable".to_string()
                }
                codexbar::cursor_activity::CursorActivityStatus::Unreadable => {
                    "unreadable".to_string()
                }
            },
            rows: snapshot
                .rows
                .into_iter()
                .map(|activity| CursorModelActivityRow {
                    model: activity.model,
                    contributions: activity.contributions,
                    requests: activity.requests,
                })
                .collect(),
        }
    })
    .await
    .unwrap_or_else(|err| {
        tracing::warn!("Cursor model-activity worker failed: {}", err);
        CursorActivitySnapshotBridge {
            status: "unreadable".to_string(),
            rows: Vec::new(),
        }
    })
}

/// Quote a CSV field only when it contains a delimiter, quote, or newline.
fn csv_field(value: &str) -> String {
    if value.contains([',', '"', '\n', '\r']) {
        format!("\"{}\"", value.replace('"', "\"\""))
    } else {
        value.to_string()
    }
}

/// Flat, spreadsheet-friendly CSV of the 30-day spend: period totals plus the
/// per-model, per-effort, and per-project breakdowns already shown in the UI.
/// Unpriced rows leave `cost_usd` blank rather than reporting a fabricated $0.
fn format_cost_csv(summary: &ProviderLocalUsageSummary) -> String {
    let mut out = String::from("section,name,cost_usd,tokens\n");
    let mut row = |section: &str, name: &str, cost: Option<f64>, tokens: Option<u64>| {
        out.push_str(&format!(
            "{},{},{},{}\n",
            csv_field(section),
            csv_field(name),
            cost.map(|c| format!("{c:.4}")).unwrap_or_default(),
            tokens.map(|t| t.to_string()).unwrap_or_default(),
        ));
    };
    row("period", "today", summary.today_cost, None);
    row(
        "period",
        "30 days",
        summary.thirty_day_cost,
        summary.thirty_day_tokens,
    );
    for model in &summary.model_breakdown {
        row("model", &model.model, model.cost, Some(model.tokens));
    }
    for effort in &summary.effort_breakdown {
        row("effort", &effort.effort, effort.cost, Some(effort.tokens));
    }
    for project in &summary.project_breakdown {
        row(
            "project",
            &project.project,
            project.cost,
            Some(project.tokens),
        );
    }
    out
}

/// Write the provider's 30-day spend breakdown to a CSV in the user's Downloads
/// folder and return the saved path. Local-only; nothing leaves the machine.
#[tauri::command]
pub async fn export_cost_csv(app: tauri::AppHandle, provider_id: String) -> Result<String, String> {
    use tauri::Manager;
    let download_dir = app
        .path()
        .download_dir()
        .map_err(|_| "Could not locate your Downloads folder.".to_string())?;
    let today = Local::now().format("%Y-%m-%d").to_string();

    tauri::async_runtime::spawn_blocking(move || {
        let summary = load_provider_local_usage_summary(&provider_id)
            .ok_or_else(|| "No local usage to export yet.".to_string())?;
        let csv = format_cost_csv(&summary);
        let path = download_dir.join(format!("ceiling-{provider_id}-spend-{today}.csv"));
        fs::write(&path, csv).map_err(|error| format!("Could not write the CSV: {error}"))?;
        Ok(path.to_string_lossy().into_owned())
    })
    .await
    .map_err(|error| {
        tracing::warn!("CSV export worker failed: {}", error);
        "The export did not finish.".to_string()
    })?
}

#[tauri::command]
pub async fn get_provider_local_usage_summary(
    provider_id: String,
) -> Option<ProviderLocalUsageSummary> {
    let failure_provider_id = provider_id.clone();
    tauri::async_runtime::spawn_blocking(move || load_provider_local_usage_summary(&provider_id))
        .await
        .unwrap_or_else(|err| {
            tracing::warn!("Provider local usage worker failed: {}", err);
            record_local_usage_fetch_failure(&failure_provider_id, CostFetchFailure::Failed);
            None
        })
}

#[cfg(test)]
pub(crate) fn build_provider_chart_data(
    provider_id: String,
    account_email: Option<String>,
) -> ProviderChartData {
    build_provider_chart_data_with_cancel(
        provider_id,
        account_email,
        None,
        None,
        ChartAccountScope::MachineWide,
        Vec::new(),
        None,
    )
}

fn build_provider_chart_data_with_cancel(
    provider_id: String,
    account_email: Option<String>,
    account_id: Option<String>,
    account_organization: Option<String>,
    account_scope: ChartAccountScope,
    usage_window_requests: Vec<LocalUsageWindowRequest>,
    cancel: Option<Arc<AtomicBool>>,
) -> ProviderChartData {
    let mut usage_windows = usage_window_requests
        .iter()
        .filter_map(|window| {
            let starts_at = DateTime::parse_from_rfc3339(&window.starts_at)
                .ok()?
                .with_timezone(&Utc);
            let ends_at = DateTime::parse_from_rfc3339(&window.ends_at)
                .ok()?
                .with_timezone(&Utc);
            (starts_at < ends_at).then(|| CurrentUsageWindow {
                id: window.id.clone(),
                starts_at,
                ends_at,
            })
        })
        .collect::<Vec<_>>();
    // Compare reads shared rolling windows, which are independent of each
    // provider's own reset boundaries. They are scanned alongside the
    // reset-aligned windows so one pass over the logs serves both.
    let (comparison_specs, comparison_windows) = comparison_period_specs(Utc::now());
    usage_windows.extend(comparison_windows);
    let report = match &account_scope {
        ChartAccountScope::MachineWide => {
            get_cost_usage_report_scoped(&provider_id, 30, &usage_windows, None)
        }
        ChartAccountScope::Account { config_dir, .. } => {
            get_cost_usage_report_scoped(&provider_id, 30, &usage_windows, Some(config_dir.clone()))
        }
        ChartAccountScope::UnresolvedAccount { .. } => None,
    };
    let cost_history: Vec<DailyCostPoint> = report
        .as_ref()
        .map(|report| {
            report
                .daily_costs
                .iter()
                .map(|(date, value)| DailyCostPoint {
                    date: date.clone(),
                    value: *value,
                })
                .collect()
        })
        .unwrap_or_default();

    let (credits_history, usage_breakdown) =
        load_openai_dashboard_chart_data(&provider_id, account_email.as_deref());
    let local_usage = if cancel
        .as_deref()
        .is_some_and(|flag| flag.load(Ordering::Relaxed))
    {
        None
    } else {
        let scoped_summary = report.as_ref().and_then(|report| {
            local_usage_summary_from_report(
                &provider_id,
                report,
                &usage_window_requests,
                &comparison_specs,
            )
        });
        if scoped_summary.is_some() || account_scope != ChartAccountScope::MachineWide {
            scoped_summary
        } else {
            load_local_usage_summary_cached(&provider_id, cancel.as_deref())
        }
    };

    ProviderChartData {
        quota_history: crate::usage_history::provider_history(
            &provider_id,
            account_email.as_deref(),
            account_id.as_deref(),
            account_organization.as_deref(),
        ),
        provider_id,
        cost_history,
        credits_history,
        usage_breakdown,
        local_usage,
        local_usage_scope: account_scope.kind(),
    }
}

fn local_usage_summary_from_report(
    provider_id: &str,
    report: &CostUsageReport,
    usage_window_requests: &[LocalUsageWindowRequest],
    comparison_specs: &[ComparisonPeriodSpec],
) -> Option<ProviderLocalUsageSummary> {
    let thirty_day_breakdown = token_breakdown(provider_id, &report.thirty_days);
    // Calendar summaries stay calendar summaries. Provider reset windows are
    // supplied explicitly above, never inferred from rolling durations.
    let seven_day_summary = &report.seven_days;
    let seven_day_breakdown = token_breakdown(provider_id, seven_day_summary);
    let last_session_breakdown = report
        .latest_session
        .as_ref()
        .map(|summary| token_breakdown(provider_id, summary));
    let thirty_day_tokens = thirty_day_breakdown.processed_tokens;
    let seven_day_tokens = seven_day_breakdown.processed_tokens;
    let last_session_tokens = last_session_breakdown
        .as_ref()
        .map(|breakdown| breakdown.processed_tokens)
        .unwrap_or(0);
    let has_usage = report.thirty_days.sessions_count > 0
        || report.thirty_days.total_cost_usd > 0.0
        || thirty_day_tokens > 0;
    if !has_usage {
        return None;
    }

    let lang = locale::current_language();
    let (seven_day_priced_tokens, seven_day_total_model_tokens) =
        pricing_coverage_tokens(seven_day_summary);
    let (thirty_day_priced_tokens, thirty_day_total_model_tokens) =
        pricing_coverage_tokens(&report.thirty_days);
    let current_windows = usage_window_requests
        .iter()
        .filter_map(|window| {
            let summary = report.current_windows.get(&window.id)?;
            let token_breakdown = token_breakdown(provider_id, summary);
            let (priced_tokens, total_model_tokens) = pricing_coverage_tokens(summary);
            Some(LocalUsageWindowSummary {
                id: window.id.clone(),
                label: window.label.clone(),
                starts_at: window.starts_at.clone(),
                ends_at: window.ends_at.clone(),
                tokens: token_breakdown.processed_tokens,
                token_breakdown,
                cost: non_zero_f64(summary.total_cost_usd),
                priced_tokens,
                total_model_tokens,
            })
        })
        .collect();
    let comparison_periods = comparison_specs
        .iter()
        .filter_map(|period| {
            let current = report.current_windows.get(&period.current_window_id)?;
            let previous = report.current_windows.get(&period.previous_window_id)?;
            let current_breakdown = token_breakdown(provider_id, current);
            let previous_breakdown = token_breakdown(provider_id, previous);
            Some(LocalUsageComparisonPeriod {
                id: period.id.to_string(),
                label: period.label.to_string(),
                current_tokens: current_breakdown.processed_tokens,
                current_breakdown,
                previous_tokens: previous_breakdown.processed_tokens,
                previous_breakdown,
            })
        })
        .collect();
    Some(ProviderLocalUsageSummary {
        today_cost: non_zero_f64(report.today.total_cost_usd),
        last_session_cost: report
            .latest_session
            .as_ref()
            .and_then(|summary| non_zero_f64(summary.total_cost_usd)),
        last_session_tokens: non_zero_u64(last_session_tokens),
        last_session_token_breakdown: last_session_breakdown,
        seven_day_cost: non_zero_f64(seven_day_summary.total_cost_usd),
        seven_day_tokens: non_zero_u64(seven_day_tokens),
        seven_day_token_breakdown: Some(seven_day_breakdown),
        seven_day_priced_tokens,
        seven_day_total_model_tokens,
        thirty_day_cost: non_zero_f64(report.thirty_days.total_cost_usd),
        thirty_day_tokens: non_zero_u64(thirty_day_tokens),
        thirty_day_token_breakdown: Some(thirty_day_breakdown),
        thirty_day_priced_tokens,
        thirty_day_total_model_tokens,
        current_windows,
        comparison_periods,
        latest_tokens: non_zero_u64(last_session_tokens),
        top_model: top_model(&report.thirty_days),
        model_breakdown: model_breakdown(provider_id, &report.thirty_days),
        effort_breakdown: effort_breakdown(&report.thirty_days),
        plan_breakdown: plan_breakdown(provider_id, &report.thirty_days),
        project_breakdown: project_breakdown(&report.thirty_days),
        estimate_note: localized_estimate_note(provider_id, lang),
        token_cost_updated_at_ms: current_unix_ms(),
    })
}

impl ProviderChartData {
    fn empty_with_scope(provider_id: String, local_usage_scope: LocalUsageScope) -> Self {
        Self {
            provider_id,
            cost_history: Vec::new(),
            credits_history: Vec::new(),
            usage_breakdown: Vec::new(),
            local_usage: None,
            local_usage_scope,
            quota_history: Vec::new(),
        }
    }
}

fn active_chart_scans() -> &'static Mutex<HashMap<String, Arc<AtomicBool>>> {
    static ACTIVE: OnceLock<Mutex<HashMap<String, Arc<AtomicBool>>>> = OnceLock::new();
    ACTIVE.get_or_init(|| Mutex::new(HashMap::new()))
}

fn register_chart_scan(provider_id: &str) -> Arc<AtomicBool> {
    let next = Arc::new(AtomicBool::new(false));
    if let Ok(mut active) = active_chart_scans().lock()
        && let Some(previous) = active.insert(provider_id.to_string(), next.clone())
    {
        previous.store(true, Ordering::Relaxed);
    }
    next
}

fn load_local_usage_summary(
    provider_id: &str,
    cancel: Option<&AtomicBool>,
) -> Option<ProviderLocalUsageSummary> {
    load_local_usage_summary_with_unknown_models(provider_id, cancel).0
}

fn load_local_usage_summary_with_unknown_models(
    provider_id: &str,
    cancel: Option<&AtomicBool>,
) -> (Option<ProviderLocalUsageSummary>, HashSet<String>) {
    let Some(thirty_day) = scan_local_cost(provider_id, 30, cancel) else {
        return (None, HashSet::new());
    };
    if cancel.is_some_and(|flag| flag.load(Ordering::Relaxed)) {
        return (None, HashSet::new());
    }
    let today = scan_local_cost(provider_id, 1, cancel).unwrap_or_default();
    let unknown_models = thirty_day
        .unknown_models
        .union(&today.unknown_models)
        .cloned()
        .collect();

    let thirty_day_breakdown = token_breakdown(provider_id, &thirty_day);
    let latest_breakdown = token_breakdown(provider_id, &today);
    let thirty_day_tokens = thirty_day_breakdown.processed_tokens;
    let latest_tokens = latest_breakdown.processed_tokens;
    let has_usage =
        thirty_day.sessions_count > 0 || thirty_day.total_cost_usd > 0.0 || thirty_day_tokens > 0;
    if !has_usage {
        return (None, unknown_models);
    }

    let lang = locale::current_language();
    let (thirty_day_priced_tokens, thirty_day_total_model_tokens) =
        pricing_coverage_tokens(&thirty_day);
    (
        Some(ProviderLocalUsageSummary {
            today_cost: non_zero_f64(today.total_cost_usd),
            last_session_cost: None,
            last_session_tokens: non_zero_u64(latest_tokens),
            last_session_token_breakdown: Some(latest_breakdown),
            seven_day_cost: None,
            seven_day_tokens: None,
            seven_day_token_breakdown: None,
            seven_day_priced_tokens: 0,
            seven_day_total_model_tokens: 0,
            thirty_day_cost: non_zero_f64(thirty_day.total_cost_usd),
            thirty_day_tokens: non_zero_u64(thirty_day_tokens),
            thirty_day_token_breakdown: Some(thirty_day_breakdown),
            thirty_day_priced_tokens,
            thirty_day_total_model_tokens,
            current_windows: Vec::new(),
            comparison_periods: Vec::new(),
            latest_tokens: non_zero_u64(latest_tokens),
            top_model: top_model(&thirty_day),
            model_breakdown: model_breakdown(provider_id, &thirty_day),
            effort_breakdown: effort_breakdown(&thirty_day),
            plan_breakdown: plan_breakdown(provider_id, &thirty_day),
            project_breakdown: project_breakdown(&thirty_day),
            estimate_note: localized_estimate_note(provider_id, lang),
            token_cost_updated_at_ms: current_unix_ms(),
        }),
        unknown_models,
    )
}

pub(crate) fn load_provider_local_usage_summary(
    provider_id: &str,
) -> Option<ProviderLocalUsageSummary> {
    load_local_usage_summary_cached(provider_id, None)
}

struct CachedLocalUsage {
    loaded_at: Instant,
    summary: Option<ProviderLocalUsageSummary>,
}

fn local_usage_cache() -> &'static Mutex<HashMap<String, CachedLocalUsage>> {
    static CACHE: OnceLock<Mutex<HashMap<String, CachedLocalUsage>>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

pub(crate) fn clear_provider_local_usage_cache() {
    if let Ok(mut guard) = local_usage_cache().lock() {
        guard.clear();
    }
}

pub(crate) fn cached_provider_local_usage_summary(
    provider_id: &str,
) -> Option<ProviderLocalUsageSummary> {
    let Ok(guard) = local_usage_cache().lock() else {
        return None;
    };
    guard
        .get(provider_id)
        .and_then(|entry| entry.summary.clone())
}

pub(crate) async fn refresh_provider_local_usage_cache(provider_ids: Vec<String>) {
    if provider_ids.is_empty() {
        return;
    }

    let failure_provider_ids = provider_ids.clone();
    let scans = match tauri::async_runtime::spawn_blocking(move || {
        provider_ids
            .into_iter()
            .map(|provider_id| {
                let (summary, unknown_models) =
                    load_local_usage_summary_with_unknown_models(&provider_id, None);
                (provider_id, summary, unknown_models)
            })
            .collect::<Vec<_>>()
    })
    .await
    {
        Ok(scans) => scans,
        Err(err) => {
            tracing::warn!("Provider local usage refresh worker failed: {err}");
            for provider_id in failure_provider_ids {
                record_local_usage_fetch_failure(&provider_id, CostFetchFailure::Failed);
            }
            return;
        }
    };

    for (provider_id, mut summary, unknown_models) in scans {
        let pricing_provider = match provider_id.as_str() {
            "codex" => Some("openai"),
            "claude" => Some("anthropic"),
            _ => None,
        };
        if let Some(pricing_provider) = pricing_provider
            && codexbar::core::refresh_unknown_models_if_needed(pricing_provider, &unknown_models)
                .await
        {
            let rescan_provider = provider_id.clone();
            summary = tauri::async_runtime::spawn_blocking(move || {
                load_local_usage_summary(&rescan_provider, None)
            })
            .await
            .unwrap_or(summary);
        }
        store_local_usage_summary(&provider_id, summary);
    }
}

#[cfg(test)]
pub(crate) fn cache_provider_local_usage_summary_for_test(
    provider_id: &str,
    summary: Option<ProviderLocalUsageSummary>,
) {
    store_local_usage_summary(provider_id, summary);
}

fn load_local_usage_summary_cached(
    provider_id: &str,
    cancel: Option<&AtomicBool>,
) -> Option<ProviderLocalUsageSummary> {
    let cache = local_usage_cache();
    if let Ok(guard) = cache.lock()
        && let Some(entry) = guard.get(provider_id)
        && token_cost_cache_is_fresh(Some(entry.loaded_at), Instant::now(), LOCAL_USAGE_TTL)
    {
        return entry.summary.clone();
    }

    if cancel.is_some_and(|flag| flag.load(Ordering::Relaxed)) {
        return None;
    }

    let summary = load_local_usage_summary(provider_id, cancel);
    if cancel.is_some_and(|flag| flag.load(Ordering::Relaxed)) {
        return None;
    }

    store_local_usage_summary(provider_id, summary.clone());
    summary
}

fn store_local_usage_summary(provider_id: &str, summary: Option<ProviderLocalUsageSummary>) {
    if let Ok(mut guard) = local_usage_cache().lock() {
        guard.insert(
            provider_id.to_string(),
            CachedLocalUsage {
                loaded_at: Instant::now(),
                summary,
            },
        );
    }
}

fn record_local_usage_fetch_failure(provider_id: &str, failure: CostFetchFailure) {
    let loaded_at = if cost_fetch_failure_allows_early_retry(failure) {
        Instant::now() - LOCAL_USAGE_TTL - Duration::from_secs(1)
    } else {
        Instant::now()
    };
    if let Ok(mut guard) = local_usage_cache().lock() {
        guard.insert(
            provider_id.to_string(),
            CachedLocalUsage {
                loaded_at,
                summary: None,
            },
        );
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(dead_code)]
pub(crate) enum CostFetchFailure {
    Failed,
    TimedOut,
}

pub(crate) fn token_cost_cache_is_fresh(
    loaded_at: Option<Instant>,
    now: Instant,
    ttl: Duration,
) -> bool {
    loaded_at
        .and_then(|loaded| now.checked_duration_since(loaded))
        .map(|age| age <= ttl)
        .unwrap_or(false)
}

pub(crate) fn cost_fetch_failure_allows_early_retry(failure: CostFetchFailure) -> bool {
    !matches!(failure, CostFetchFailure::TimedOut)
}

fn current_unix_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis().min(i64::MAX as u128) as i64)
        .unwrap_or(0)
}

fn localized_estimate_note(provider_id: &str, lang: codexbar::settings::Language) -> String {
    match provider_id {
        "claude" => locale::get_text(lang, LocaleKey::PanelEstimatedFromLocalLogsClaude),
        "grok" => locale::get_text(lang, LocaleKey::PanelEstimatedFromLocalLogsGrok),
        _ => locale::get_text(lang, LocaleKey::PanelEstimatedFromLocalLogs),
    }
}

fn scan_local_cost(
    provider_id: &str,
    days: u32,
    cancel: Option<&AtomicBool>,
) -> Option<CostSummary> {
    let scanner = CostScanner::new(days);
    match provider_id {
        "codex" => Some(scanner.scan_codex_with_cancel(cancel)),
        "claude" => Some(scanner.scan_claude_with_cancel(cancel)),
        // Grok has no cancel-aware summary path yet; use the full report scan.
        "grok" => {
            let _ = (scanner, cancel);
            codexbar::cost_scanner::get_cost_usage_report("grok", days).map(|r| r.thirty_days)
        }
        _ => None,
    }
}

fn token_breakdown(provider_id: &str, summary: &CostSummary) -> LocalTokenBreakdown {
    let normalized = summary.normalized_tokens(provider_id);
    LocalTokenBreakdown {
        processed_tokens: normalized.processed(),
        fresh_input_tokens: normalized.fresh_input_tokens,
        output_tokens: normalized.output_tokens,
        cache_read_tokens: normalized.cache_read_tokens,
        cache_write_tokens: normalized.cache_write_tokens,
        reasoning_tokens: summary.reasoning_tokens,
    }
}

fn non_zero_f64(value: f64) -> Option<f64> {
    (value > 0.0).then_some(value)
}

fn non_zero_u64(value: u64) -> Option<u64> {
    (value > 0).then_some(value)
}

/// Per-model spend for a period: every model that recorded tokens, with its
/// dollar cost when the model is priced (`None` otherwise). Sorted by cost
/// descending, then tokens descending, so the priciest models lead and
/// unpriced models fall to the end.
fn model_breakdown(provider_id: &str, summary: &CostSummary) -> Vec<LocalModelCost> {
    let mut rows: Vec<LocalModelCost> = summary
        .by_model_tokens
        .iter()
        .map(|(model, counts)| {
            let cost = summary.by_model.get(model).copied();
            // Provider-normalized, so Codex's cached input is not counted in
            // both the input and cache buckets of the same ratio.
            let cache_read_percent = counts.normalized(provider_id).cache_read_percent();
            let cost_per_call = match (cost, counts.calls) {
                (Some(usd), calls) if calls > 0 => Some(usd / calls as f64),
                _ => None,
            };
            let output_tokens_per_call =
                (counts.calls > 0).then_some(counts.output_tokens as f64 / counts.calls as f64);
            LocalModelCost {
                model: model.clone(),
                cost,
                tokens: counts.total(),
                cache_read_percent,
                cost_per_call,
                output_tokens_per_call,
                calls: counts.calls,
            }
        })
        .collect();
    rows.sort_by(|a, b| {
        // Priced models always lead unpriced ones, even a priced $0.00 model,
        // so an unpriced row can't jump ahead on token count alone.
        b.cost
            .is_some()
            .cmp(&a.cost.is_some())
            .then_with(|| b.cost.unwrap_or(0.0).total_cmp(&a.cost.unwrap_or(0.0)))
            .then(b.tokens.cmp(&a.tokens))
            .then(a.model.cmp(&b.model))
    });
    rows
}

/// Per-reasoning-effort spend for a period, mirroring `model_breakdown`.
/// Codex populates `by_effort` / `by_effort_tokens`; other providers leave
/// them empty, so this returns an empty vec for them.
/// Plans seen in local logs, largest first. Empty when the provider emits no
/// plan signal at all (Claude), so the UI shows nothing rather than a
/// misleading single-plan claim.
fn plan_breakdown(provider_id: &str, summary: &CostSummary) -> Vec<LocalPlanUsage> {
    let mut rows: Vec<LocalPlanUsage> = summary
        .by_plan_tokens
        .iter()
        .map(|(plan, counts)| LocalPlanUsage {
            plan: plan.clone(),
            tokens: counts.normalized(provider_id).processed(),
        })
        .filter(|row| row.tokens > 0)
        .collect();
    rows.sort_by(|a, b| b.tokens.cmp(&a.tokens).then(a.plan.cmp(&b.plan)));
    rows
}

fn effort_breakdown(summary: &CostSummary) -> Vec<LocalEffortCost> {
    let mut rows: Vec<LocalEffortCost> = summary
        .by_effort_tokens
        .iter()
        .map(|(effort, counts)| LocalEffortCost {
            effort: effort.clone(),
            cost: summary.by_effort.get(effort).copied(),
            tokens: counts.total(),
        })
        .collect();
    rows.sort_by(|a, b| {
        b.cost
            .is_some()
            .cmp(&a.cost.is_some())
            .then_with(|| b.cost.unwrap_or(0.0).total_cmp(&a.cost.unwrap_or(0.0)))
            .then(b.tokens.cmp(&a.tokens))
            .then(a.effort.cmp(&b.effort))
    });
    rows
}

/// Per-project spend for a period, mirroring `model_breakdown`: every project
/// that recorded tokens, priced-first, unpriced kept and sorted last.
fn project_breakdown(summary: &CostSummary) -> Vec<LocalProjectCost> {
    let mut rows: Vec<LocalProjectCost> = summary
        .by_project_tokens
        .iter()
        .map(|(project, counts)| LocalProjectCost {
            project: project.clone(),
            cost: summary.by_project.get(project).copied(),
            tokens: counts.total(),
        })
        .collect();
    rows.sort_by(|a, b| {
        b.cost
            .is_some()
            .cmp(&a.cost.is_some())
            .then_with(|| b.cost.unwrap_or(0.0).total_cmp(&a.cost.unwrap_or(0.0)))
            .then(b.tokens.cmp(&a.tokens))
            .then(a.project.cmp(&b.project))
    });
    rows
}

fn top_model(summary: &CostSummary) -> Option<String> {
    summary
        .by_model_tokens
        .iter()
        .max_by_key(|(_, counts)| counts.total())
        .map(|(model, _)| model.clone())
        .or_else(|| {
            summary
                .by_model
                .iter()
                .max_by(|a, b| a.1.total_cmp(b.1))
                .map(|(model, _)| model.clone())
        })
}

fn load_openai_dashboard_chart_data(
    provider_id: &str,
    account_email: Option<&str>,
) -> (Vec<DailyCostPoint>, Vec<DailyUsageBreakdown>) {
    if provider_id != "codex" && provider_id != "openai" {
        return (Vec::new(), Vec::new());
    }

    let Some(account_email) = account_email else {
        return (Vec::new(), Vec::new());
    };

    let Some(cache) = OpenAIDashboardCacheStore::load() else {
        return (Vec::new(), Vec::new());
    };

    if !cache.account_email.eq_ignore_ascii_case(account_email) {
        return (Vec::new(), Vec::new());
    }

    let snapshot = &cache.snapshot;

    let breakdown_source = if !snapshot.daily_breakdown.is_empty() {
        &snapshot.daily_breakdown
    } else if !snapshot.usage_breakdown.is_empty() {
        &snapshot.usage_breakdown
    } else {
        return (Vec::new(), Vec::new());
    };

    let credits_history: Vec<DailyCostPoint> = breakdown_source
        .iter()
        .map(|d| DailyCostPoint {
            date: d.day.clone(),
            value: d.total_credits_used,
        })
        .collect();

    let usage_breakdown: Vec<DailyUsageBreakdown> = snapshot
        .usage_breakdown
        .iter()
        .map(|d| DailyUsageBreakdown {
            day: d.day.clone(),
            services: d
                .services
                .iter()
                .map(|s| ServiceUsagePoint {
                    service: s.service.clone(),
                    credits_used: s.credits_used,
                })
                .collect(),
            total_credits_used: d.total_credits_used,
        })
        .collect();

    (credits_history, usage_breakdown)
}

#[cfg(test)]
mod tests {
    use super::{
        ACTIVITY_HEATMAP_DAYS, ActivityHourPoint, CHART_CACHE_MAX_ENTRIES,
        CHART_CACHE_MAX_ENTRY_AGE, CHART_CACHE_VERSION, CachedProviderChartData, ChartAccountScope,
        CostFetchFailure, LocalEffortCost, LocalModelCost, LocalPlanUsage, LocalProjectCost,
        LocalTokenBreakdown, LocalUsageWindowRequest, PersistedChartCache, ProviderChartData,
        ProviderLocalUsageSummary, activity_heatmap_days, activity_hours_for_provider,
        api_value_period, assemble_activity_heatmap, chart_cache_key, comparison_period_specs,
        cost_fetch_failure_allows_early_retry, current_unix_ms, daily_series_from_report,
        effort_breakdown, format_cost_csv, heatmap_hours_for_days, load_chart_cache_from,
        local_midnight_in_tz, local_usage_summary_from_report, local_yesterday_window_utc,
        localized_estimate_note, model_breakdown, parse_api_value_custom_range,
        period_from_daily_series, pricing_coverage_tokens, project_breakdown, prune_chart_cache,
        prune_loaded_chart_cache, resolve_chart_account_scope, spend_anomaly_reading,
        spend_budget_period_details, token_breakdown, token_cost_cache_is_fresh,
    };
    use crate::commands::is_provider_cache_fresh;
    use chrono::{Local, LocalResult, NaiveDate, NaiveTime, TimeZone, Timelike, Utc};
    use codexbar::core::{CodexIdentity, ConfiguredAccounts, DirectoryAccount};
    use codexbar::cost_scanner::{
        CostSummary, CostUsageReport, HourlyActivityPoint, ModelTokenCounts,
    };
    use codexbar::settings::Language;
    use std::collections::HashMap;
    use std::path::PathBuf;
    use std::time::{Duration, Instant};

    fn cache_entry(refreshed_at_ms: i64) -> CachedProviderChartData {
        CachedProviderChartData {
            refreshed_at_ms,
            data: ProviderChartData {
                provider_id: "claude".to_string(),
                cost_history: Vec::new(),
                credits_history: Vec::new(),
                usage_breakdown: Vec::new(),
                local_usage: None,
                local_usage_scope: Default::default(),
                quota_history: Vec::new(),
            },
        }
    }

    fn cache_with(entries: impl IntoIterator<Item = (String, i64)>) -> PersistedChartCache {
        let mut cache = PersistedChartCache::default();
        for (key, refreshed_at_ms) in entries {
            cache.entries.insert(key, cache_entry(refreshed_at_ms));
        }
        cache
    }

    /// SBS-887: a rolled reset window mints a new key and strands the old one.
    #[test]
    fn prune_drops_entries_past_the_max_age() {
        let now = current_unix_ms();
        let day_ms = 24 * 60 * 60 * 1_000;
        let mut cache = cache_with([
            ("fresh".to_string(), now - day_ms),
            ("stale".to_string(), now - 8 * day_ms),
        ]);

        prune_chart_cache(&mut cache, "");

        assert!(cache.entries.contains_key("fresh"));
        assert!(
            !cache.entries.contains_key("stale"),
            "an entry past CHART_CACHE_MAX_ENTRY_AGE is a dead reset window"
        );
    }

    #[test]
    fn prune_caps_the_entry_count_and_evicts_the_oldest_first() {
        let now = current_unix_ms();
        // Every entry is inside the age window, so only the count bound applies.
        let entries = (0..CHART_CACHE_MAX_ENTRIES + 10)
            .map(|index| (format!("key-{index:03}"), now - index as i64 * 1_000));
        let mut cache = cache_with(entries);

        prune_chart_cache(&mut cache, "");

        assert_eq!(cache.entries.len(), CHART_CACHE_MAX_ENTRIES);
        // index 0 is the most recently refreshed, so the low indexes survive.
        assert!(cache.entries.contains_key("key-000"));
        assert!(
            !cache
                .entries
                .contains_key(&format!("key-{:03}", CHART_CACHE_MAX_ENTRIES + 9)),
            "the least recently refreshed entry must be evicted first"
        );
    }

    #[test]
    fn prune_never_evicts_the_key_just_stored() {
        let now = current_unix_ms();
        let day_ms = 24 * 60 * 60 * 1_000;
        // Older than the age bound, and the oldest of an over-cap map.
        let mut entries: Vec<(String, i64)> = (0..CHART_CACHE_MAX_ENTRIES + 10)
            .map(|index| (format!("key-{index:03}"), now - index as i64 * 1_000))
            .collect();
        entries.push(("just-stored".to_string(), now - 30 * day_ms));
        let mut cache = cache_with(entries);

        prune_chart_cache(&mut cache, "just-stored");

        assert!(
            cache.entries.contains_key("just-stored"),
            "the entry the caller just wrote must survive its own prune"
        );
    }

    #[test]
    fn prune_leaves_a_small_cache_alone() {
        let now = current_unix_ms();
        let mut cache = cache_with([("a".to_string(), now), ("b".to_string(), now - 1_000)]);

        prune_chart_cache(&mut cache, "");

        assert_eq!(cache.entries.len(), 2);
    }

    /// A cache that predates the bound has to be written back, or the file stays
    /// oversized until some later store happens to run and every launch pays the
    /// full read.
    #[test]
    fn a_shrunk_cache_reports_that_it_needs_writing_back() {
        let now = current_unix_ms();
        let day_ms = 24 * 60 * 60 * 1_000;
        let oversized = cache_with([
            ("fresh".to_string(), now),
            ("dead-window".to_string(), now - 30 * day_ms),
        ]);

        let (pruned, shrank) = prune_loaded_chart_cache(oversized);

        assert!(shrank, "dropping an entry must ask for a write-back");
        assert_eq!(pruned.entries.len(), 1);
        assert_eq!(
            pruned.version, CHART_CACHE_VERSION,
            "the written-back file must carry the current version"
        );
    }

    #[test]
    fn an_already_bounded_cache_is_not_rewritten() {
        let now = current_unix_ms();
        let cache = cache_with([("a".to_string(), now), ("b".to_string(), now - 1_000)]);

        let (pruned, shrank) = prune_loaded_chart_cache(cache);

        assert!(!shrank, "no change means no pointless disk write");
        assert_eq!(pruned.entries.len(), 2);
    }

    /// Providers that send usage windows and so mint a new key on every roll.
    /// Grok is in here even though it only carries a weekly window: the fixture
    /// deliberately rolls all three at the 5-hour cadence, which is the worst
    /// case the bound has to hold up against.
    const WINDOWED_PROVIDERS: [&str; 3] = ["claude", "codex", "grok"];
    /// The Claude/Codex session window length.
    const ROLL_HOURS: i64 = 5;

    fn session_window(now_ms: i64, rolls_ago: i64) -> Vec<LocalUsageWindowRequest> {
        let hour_ms = 60 * 60 * 1_000;
        let starts_at = now_ms - rolls_ago * ROLL_HOURS * hour_ms;
        vec![LocalUsageWindowRequest {
            id: "session".to_string(),
            label: "Session".to_string(),
            starts_at: starts_at.to_string(),
            ends_at: (starts_at + ROLL_HOURS * hour_ms).to_string(),
        }]
    }

    fn account_scope(account_id: &str) -> ChartAccountScope {
        ChartAccountScope::Account {
            account_id: account_id.to_string(),
            config_dir: PathBuf::from("/tmp/ceiling-test"),
        }
    }

    /// The age bound has to be what retires rolled windows. If the count cap
    /// binds first it evicts by refresh time, which can drop a window that is
    /// still current.
    ///
    /// This builds the real key mix rather than asserting arithmetic: two
    /// accounts on each windowed provider, seen on both the account-scoped
    /// Charts tab and the machine-wide Compare tab (which mint different keys
    /// for the same data), across twice the age bound of 5-hour rolls, plus the
    /// stable empty-window keys MenuCard and provider detail hold.
    #[test]
    fn prune_keeps_every_current_key_across_a_realistic_surface_mix() {
        let now = current_unix_ms();
        let hour_ms = 60 * 60 * 1_000;
        let max_age_ms = CHART_CACHE_MAX_ENTRY_AGE.as_millis() as i64;
        let rolls = (CHART_CACHE_MAX_ENTRY_AGE.as_secs() / (ROLL_HOURS as u64 * 60 * 60)) as i64;

        let mut cache = PersistedChartCache::default();
        let mut in_age: Vec<String> = Vec::new();
        let mut past_age: Vec<String> = Vec::new();

        // Twice the age bound of rolls, so the second half must be retired.
        for rolls_ago in 0..rolls * 2 {
            let refreshed_at_ms = now - rolls_ago * ROLL_HOURS * hour_ms;
            let windows = session_window(now, rolls_ago);
            for provider in WINDOWED_PROVIDERS {
                let mut keys = vec![
                    // Compare: machine-wide identity, no source label.
                    chart_cache_key(
                        provider,
                        None,
                        None,
                        None,
                        &windows,
                        &ChartAccountScope::MachineWide,
                    ),
                ];
                for account in ["acct-primary", "acct-secondary"] {
                    // Charts: account scope plus a source label.
                    keys.push(chart_cache_key(
                        provider,
                        Some("user@example.com"),
                        None,
                        Some("oauth"),
                        &windows,
                        &account_scope(account),
                    ));
                }
                for key in keys {
                    assert!(
                        cache
                            .entries
                            .insert(key.clone(), cache_entry(refreshed_at_ms))
                            .is_none(),
                        "the fixture must not collide keys: {key}"
                    );
                    if now - refreshed_at_ms <= max_age_ms {
                        in_age.push(key);
                    } else {
                        past_age.push(key);
                    }
                }
            }
        }

        // MenuCard and provider detail: stable empty-window keys, last refreshed
        // a while ago but still inside the age bound.
        let stable_refreshed_at_ms = now - max_age_ms / 2;
        let mut stable: Vec<String> = Vec::new();
        for provider in WINDOWED_PROVIDERS {
            for account in ["acct-primary", "acct-secondary"] {
                stable.push(chart_cache_key(
                    provider,
                    Some("user@example.com"),
                    None,
                    Some("menu"),
                    &[],
                    &account_scope(account),
                ));
            }
        }
        for key in &stable {
            cache
                .entries
                .insert(key.clone(), cache_entry(stable_refreshed_at_ms));
        }

        let live = in_age.len() + stable.len();
        assert!(
            live <= CHART_CACHE_MAX_ENTRIES,
            "the count backstop must not bind in normal use: {live} live keys vs a \
             {CHART_CACHE_MAX_ENTRIES} cap"
        );

        prune_chart_cache(&mut cache, "");

        for key in &in_age {
            assert!(
                cache.entries.contains_key(key),
                "a key inside the age bound must survive: {key}"
            );
        }
        for key in &stable {
            assert!(
                cache.entries.contains_key(key),
                "a stable MenuCard key must survive the window churn around it: {key}"
            );
        }
        for key in &past_age {
            assert!(
                !cache.entries.contains_key(key),
                "a key past the age bound is a dead window: {key}"
            );
        }
        assert_eq!(cache.entries.len(), live);
    }

    fn write_cache_file(path: &std::path::Path, version: u8, entries: usize, refreshed_at_ms: i64) {
        let mut cache = PersistedChartCache {
            version,
            entries: HashMap::new(),
        };
        for index in 0..entries {
            cache
                .entries
                .insert(format!("key-{index:04}"), cache_entry(refreshed_at_ms));
        }
        std::fs::write(path, serde_json::to_vec(&cache).expect("serialize fixture"))
            .expect("write fixture");
    }

    fn read_cache_file(path: &std::path::Path) -> PersistedChartCache {
        serde_json::from_slice(&std::fs::read(path).expect("read cache file")).expect("parse cache")
    }

    /// SBS-887: an oversized file written before the bound existed has to shrink
    /// on load. Nothing else rewrites it until someone opens a chart, so without
    /// this every launch pays the full read.
    #[test]
    fn an_oversized_current_version_file_shrinks_on_disk_at_load() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("chart-data-cache.json");
        write_cache_file(
            &path,
            CHART_CACHE_VERSION,
            CHART_CACHE_MAX_ENTRIES + 40,
            current_unix_ms(),
        );
        let before = std::fs::metadata(&path).expect("metadata").len();

        let loaded = load_chart_cache_from(&path);

        assert_eq!(loaded.entries.len(), CHART_CACHE_MAX_ENTRIES);
        let on_disk = read_cache_file(&path);
        assert_eq!(on_disk.version, CHART_CACHE_VERSION);
        assert_eq!(
            on_disk.entries.len(),
            CHART_CACHE_MAX_ENTRIES,
            "the file itself has to shrink, not just the in-memory map"
        );
        assert!(
            std::fs::metadata(&path).expect("metadata").len() < before,
            "the rewritten file must be smaller than the one we read"
        );
    }

    /// A superseded-version file is discarded in memory, which used to leave the
    /// old bytes on disk to be re-read on every launch forever.
    #[test]
    fn an_oversized_superseded_version_file_shrinks_on_disk_at_load() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("chart-data-cache.json");
        write_cache_file(
            &path,
            CHART_CACHE_VERSION - 1,
            CHART_CACHE_MAX_ENTRIES + 40,
            current_unix_ms(),
        );
        let before = std::fs::metadata(&path).expect("metadata").len();

        let loaded = load_chart_cache_from(&path);

        assert!(
            loaded.entries.is_empty(),
            "a superseded cache is not usable"
        );
        let on_disk = read_cache_file(&path);
        assert_eq!(
            on_disk.version, CHART_CACHE_VERSION,
            "the healed file must carry the current version"
        );
        assert!(
            on_disk.entries.is_empty(),
            "the superseded entries must be gone from disk, not just from memory"
        );
        assert!(
            std::fs::metadata(&path).expect("metadata").len() < before,
            "the rewritten file must be smaller than the one we read"
        );
    }

    #[test]
    fn an_unparsable_cache_file_is_rewritten_at_load() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("chart-data-cache.json");
        std::fs::write(&path, vec![b'x'; 64 * 1024]).expect("write fixture");

        let loaded = load_chart_cache_from(&path);

        assert!(loaded.entries.is_empty());
        let on_disk = read_cache_file(&path);
        assert_eq!(on_disk.version, CHART_CACHE_VERSION);
        assert!(on_disk.entries.is_empty());
    }

    #[test]
    fn an_already_bounded_file_is_left_untouched_at_load() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("chart-data-cache.json");
        let cache = cache_with([("a".to_string(), current_unix_ms())]);
        // Pretty-printed on purpose: a needless rewrite would compact it.
        let original = serde_json::to_vec_pretty(&PersistedChartCache {
            version: CHART_CACHE_VERSION,
            entries: cache.entries,
        })
        .expect("serialize fixture");
        std::fs::write(&path, &original).expect("write fixture");

        let loaded = load_chart_cache_from(&path);

        assert_eq!(loaded.entries.len(), 1);
        assert_eq!(
            std::fs::read(&path).expect("read cache file"),
            original,
            "a healthy cache must not be rewritten on every launch"
        );
    }

    #[test]
    fn a_missing_cache_file_is_not_created_at_load() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("chart-data-cache.json");

        let loaded = load_chart_cache_from(&path);

        assert!(loaded.entries.is_empty());
        assert!(
            !path.exists(),
            "loading must not create an empty cache file"
        );
    }

    #[test]
    fn chart_cache_separates_managed_accounts_that_share_an_email() {
        let personal = chart_cache_key(
            "codex",
            Some("shared@example.com"),
            None,
            Some("oauth"),
            &[],
            &ChartAccountScope::Account {
                account_id: "acct-personal".into(),
                config_dir: PathBuf::from("personal"),
            },
        );
        let work = chart_cache_key(
            "codex",
            Some("shared@example.com"),
            None,
            Some("oauth"),
            &[],
            &ChartAccountScope::Account {
                account_id: "acct-work".into(),
                config_dir: PathBuf::from("work"),
            },
        );

        assert_ne!(personal, work);
    }

    #[test]
    fn machine_wide_cache_identity_matches_charts_panel_and_compare() {
        let scope = ChartAccountScope::MachineWide;
        let charts_panel_with_email = scope.cache_identity(Some("person@example.com"), None);
        let charts_panel_with_organization =
            scope.cache_identity(None, Some("Example Organization"));
        let compare = scope.cache_identity(None, None);

        assert_eq!(compare, "machine");
        assert_eq!(charts_panel_with_email, compare);
        assert_eq!(charts_panel_with_organization, compare);
    }

    #[test]
    fn chart_scope_distinguishes_machine_wide_resolved_and_stale_accounts() {
        let mut accounts = ConfiguredAccounts::default();
        let account_id = accounts
            .codex
            .add_account(DirectoryAccount::<CodexIdentity>::new(
                Some("work".into()),
                PathBuf::from("/homes/work"),
            ))
            .to_string();

        assert_eq!(
            resolve_chart_account_scope("codex", None, &accounts),
            ChartAccountScope::MachineWide
        );
        assert_eq!(
            resolve_chart_account_scope("codex", Some(&account_id), &accounts),
            ChartAccountScope::Account {
                account_id: account_id.clone(),
                config_dir: PathBuf::from("/homes/work"),
            }
        );
        assert_eq!(
            resolve_chart_account_scope("codex", Some("removed-account"), &accounts),
            ChartAccountScope::UnresolvedAccount {
                account_id: "removed-account".into(),
            }
        );
    }

    #[test]
    fn unresolved_account_never_shares_the_machine_wide_chart_cache() {
        let machine = chart_cache_key(
            "claude",
            Some("same@example.com"),
            None,
            Some("oauth"),
            &[],
            &ChartAccountScope::MachineWide,
        );
        let unresolved = chart_cache_key(
            "claude",
            Some("same@example.com"),
            None,
            Some("oauth"),
            &[],
            &ChartAccountScope::UnresolvedAccount {
                account_id: "stale-id".into(),
            },
        );

        assert_ne!(machine, unresolved);
    }

    #[test]
    fn token_cost_age_does_not_use_provider_quota_age() {
        let now = Instant::now();
        let token_loaded = now - Duration::from_secs(31);
        let provider_updated = now;
        assert!(!token_cost_cache_is_fresh(
            Some(token_loaded),
            now,
            Duration::from_secs(30)
        ));
        assert!(is_provider_cache_fresh(
            Some(provider_updated),
            Duration::from_secs(30)
        ));
    }

    #[test]
    fn fast_cost_failures_allow_the_next_pass_to_retry() {
        assert!(cost_fetch_failure_allows_early_retry(
            CostFetchFailure::Failed
        ));
        assert!(!cost_fetch_failure_allows_early_retry(
            CostFetchFailure::TimedOut
        ));
    }

    #[test]
    fn local_usage_summary_serializes_token_cost_timestamp() {
        let summary = ProviderLocalUsageSummary {
            today_cost: Some(1.0),
            last_session_cost: Some(0.5),
            last_session_tokens: Some(40),
            last_session_token_breakdown: None,
            seven_day_cost: Some(1.5),
            seven_day_tokens: Some(200),
            seven_day_token_breakdown: None,
            seven_day_priced_tokens: 200,
            seven_day_total_model_tokens: 200,
            thirty_day_cost: Some(2.0),
            thirty_day_tokens: Some(300),
            thirty_day_token_breakdown: None,
            thirty_day_priced_tokens: 300,
            thirty_day_total_model_tokens: 300,
            current_windows: Vec::new(),
            comparison_periods: Vec::new(),
            latest_tokens: Some(40),
            top_model: Some("gpt-5".to_string()),
            model_breakdown: vec![LocalModelCost {
                model: "gpt-5".to_string(),
                cost: Some(2.0),
                tokens: 300,
                cache_read_percent: None,
                cost_per_call: None,
                output_tokens_per_call: None,
                calls: 0,
            }],
            effort_breakdown: vec![LocalEffortCost {
                effort: "high".to_string(),
                cost: Some(2.0),
                tokens: 300,
            }],
            plan_breakdown: vec![LocalPlanUsage {
                plan: "prolite".to_string(),
                tokens: 300,
            }],
            project_breakdown: vec![LocalProjectCost {
                project: "ceiling".to_string(),
                cost: Some(2.0),
                tokens: 300,
            }],
            estimate_note: "estimated".to_string(),
            token_cost_updated_at_ms: 1234,
        };

        // CSV export covers the period totals and each breakdown; unpriced rows
        // (none here) would leave cost_usd blank.
        let csv = format_cost_csv(&summary);
        assert!(csv.starts_with("section,name,cost_usd,tokens\n"));
        assert!(csv.contains("period,today,1.0000,\n"), "csv: {csv}");
        assert!(csv.contains("period,30 days,2.0000,300\n"), "csv: {csv}");
        assert!(csv.contains("model,gpt-5,2.0000,300\n"), "csv: {csv}");
        assert!(csv.contains("effort,high,2.0000,300\n"), "csv: {csv}");
        assert!(csv.contains("project,ceiling,2.0000,300\n"), "csv: {csv}");

        let json = serde_json::to_value(summary).expect("serialize summary");
        assert_eq!(
            json.get("tokenCostUpdatedAtMs").and_then(|v| v.as_i64()),
            Some(1234)
        );
        assert_eq!(
            json.get("modelBreakdown")
                .and_then(|v| v.as_array())
                .map(|rows| rows.len()),
            Some(1)
        );
    }

    /// SOU-295: Codex reports cached input inside `input_tokens`. Adding the
    /// cache bucket on top counted those tokens twice, so a 97%-cached model
    /// rendered as 49% (`97 / (100 + 97)`).
    #[test]
    fn codex_cache_percent_does_not_double_count_cached_input() {
        let mut summary = CostSummary::default();
        summary.by_model_tokens.insert(
            "gpt-5.6-sol".to_string(),
            ModelTokenCounts {
                // 338.2M input, of which 329.0M was served from cache.
                input_tokens: 338_200_000,
                output_tokens: 1_000_000,
                cached_tokens: 329_000_000,
                cache_read_tokens: 329_000_000,
                ..Default::default()
            },
        );

        let percent = model_breakdown("codex", &summary)[0]
            .cache_read_percent
            .expect("cache percent");

        assert!(
            (96.0..=98.0).contains(&percent),
            "expected ~97% cache read, got {percent:.1}% (49% means cached input was counted twice)"
        );
    }

    /// Claude reports cache reads outside its input count, so the same figures
    /// must not have anything subtracted from them.
    #[test]
    fn claude_cache_percent_leaves_input_untouched() {
        let mut summary = CostSummary::default();
        summary.by_model_tokens.insert(
            "claude-sonnet".to_string(),
            ModelTokenCounts {
                input_tokens: 10_000,
                output_tokens: 0,
                cache_read_tokens: 90_000,
                ..Default::default()
            },
        );

        let percent = model_breakdown("claude", &summary)[0]
            .cache_read_percent
            .expect("cache percent");

        // 90k of 100k processed, with no Codex-style correction applied.
        assert!((percent - 90.0).abs() < 0.001, "got {percent}");
    }

    #[test]
    fn model_breakdown_orders_priced_first_and_keeps_unpriced() {
        let mut summary = CostSummary::default();
        // Two priced models and one unpriced (tokens only, no dollars).
        summary.by_model.insert("cheap".to_string(), 1.0);
        summary.by_model.insert("pricey".to_string(), 9.0);
        // A priced $0.00 model must still lead any unpriced model, even though
        // the unpriced one below has far more tokens.
        summary.by_model.insert("free".to_string(), 0.0);
        summary.by_model_tokens.insert(
            "free".to_string(),
            ModelTokenCounts {
                input_tokens: 1,
                output_tokens: 1,
                ..Default::default()
            },
        );
        summary.by_model_tokens.insert(
            "cheap".to_string(),
            ModelTokenCounts {
                input_tokens: 100,
                output_tokens: 100,
                ..Default::default()
            },
        );
        summary.by_model_tokens.insert(
            "pricey".to_string(),
            ModelTokenCounts {
                input_tokens: 10,
                output_tokens: 10,
                ..Default::default()
            },
        );
        summary.by_model_tokens.insert(
            "unpriced".to_string(),
            ModelTokenCounts {
                input_tokens: 500,
                output_tokens: 500,
                ..Default::default()
            },
        );

        let rows = model_breakdown("codex", &summary);

        assert_eq!(
            rows,
            vec![
                LocalModelCost {
                    model: "pricey".to_string(),
                    cost: Some(9.0),
                    tokens: 20,
                    cache_read_percent: Some(0.0),
                    cost_per_call: None,
                    output_tokens_per_call: None,
                    calls: 0,
                },
                LocalModelCost {
                    model: "cheap".to_string(),
                    cost: Some(1.0),
                    tokens: 200,
                    cache_read_percent: Some(0.0),
                    cost_per_call: None,
                    output_tokens_per_call: None,
                    calls: 0,
                },
                // Priced $0.00 still leads the unpriced model despite fewer tokens.
                LocalModelCost {
                    model: "free".to_string(),
                    cost: Some(0.0),
                    tokens: 2,
                    cache_read_percent: Some(0.0),
                    cost_per_call: None,
                    output_tokens_per_call: None,
                    calls: 0,
                },
                // Unpriced model keeps its tokens but sorts last with no dollars.
                LocalModelCost {
                    model: "unpriced".to_string(),
                    cost: None,
                    tokens: 1000,
                    cache_read_percent: Some(0.0),
                    cost_per_call: None,
                    output_tokens_per_call: None,
                    calls: 0,
                },
            ]
        );
    }

    #[test]
    fn effort_breakdown_orders_by_cost_and_is_empty_without_effort_data() {
        // No effort data (e.g. Claude) yields an empty breakdown.
        assert!(effort_breakdown(&CostSummary::default()).is_empty());

        let mut summary = CostSummary::default();
        summary.by_effort.insert("high".to_string(), 8.0);
        summary.by_effort.insert("medium".to_string(), 2.0);
        summary.by_effort_tokens.insert(
            "high".to_string(),
            ModelTokenCounts {
                input_tokens: 50,
                output_tokens: 50,
                ..Default::default()
            },
        );
        summary.by_effort_tokens.insert(
            "medium".to_string(),
            ModelTokenCounts {
                input_tokens: 200,
                output_tokens: 200,
                ..Default::default()
            },
        );
        // Unknown-effort usage with no price sorts last.
        summary.by_effort_tokens.insert(
            "unknown".to_string(),
            ModelTokenCounts {
                input_tokens: 900,
                output_tokens: 900,
                ..Default::default()
            },
        );

        let rows = effort_breakdown(&summary);

        assert_eq!(
            rows,
            vec![
                LocalEffortCost {
                    effort: "high".to_string(),
                    cost: Some(8.0),
                    tokens: 100,
                },
                LocalEffortCost {
                    effort: "medium".to_string(),
                    cost: Some(2.0),
                    tokens: 400,
                },
                LocalEffortCost {
                    effort: "unknown".to_string(),
                    cost: None,
                    tokens: 1800,
                },
            ]
        );
    }

    #[test]
    fn project_breakdown_orders_priced_first_and_keeps_unpriced() {
        assert!(project_breakdown(&CostSummary::default()).is_empty());

        let mut summary = CostSummary::default();
        summary.by_project.insert("ceiling".to_string(), 9.0);
        summary.by_project.insert("burnwatch".to_string(), 1.0);
        for (name, input) in [("ceiling", 100), ("burnwatch", 100), ("unknown", 900)] {
            summary.by_project_tokens.insert(
                name.to_string(),
                ModelTokenCounts {
                    input_tokens: input,
                    output_tokens: input,
                    ..Default::default()
                },
            );
        }

        let rows = project_breakdown(&summary);

        assert_eq!(
            rows,
            vec![
                LocalProjectCost {
                    project: "ceiling".to_string(),
                    cost: Some(9.0),
                    tokens: 200,
                },
                LocalProjectCost {
                    project: "burnwatch".to_string(),
                    cost: Some(1.0),
                    tokens: 200,
                },
                // Unpriced project keeps tokens, sorts last.
                LocalProjectCost {
                    project: "unknown".to_string(),
                    cost: None,
                    tokens: 1800,
                },
            ]
        );
    }

    #[test]
    fn api_value_period_reports_partial_pricing_coverage() {
        let mut summary = CostSummary {
            total_cost_usd: 5.0,
            sessions_count: 2,
            input_tokens: 400,
            output_tokens: 100,
            ..Default::default()
        };
        // One priced model (400 tokens) and one unpriced (100 tokens).
        summary.by_model.insert("gpt-5.6-sol".to_string(), 5.0);
        summary.by_model_tokens.insert(
            "gpt-5.6-sol".to_string(),
            ModelTokenCounts {
                input_tokens: 300,
                output_tokens: 100,
                ..Default::default()
            },
        );
        summary.by_model_tokens.insert(
            "gpt-mystery".to_string(),
            ModelTokenCounts {
                input_tokens: 100,
                output_tokens: 0,
                ..Default::default()
            },
        );
        summary.unknown_models.insert("gpt-mystery".to_string());

        let period = api_value_period("codex", &summary);

        assert_eq!(period.api_value_usd, 5.0);
        assert_eq!(period.tokens, 500); // processed = fresh input + output
        assert_eq!(period.total_tokens, 500); // model tokens: 400 priced + 100 unpriced
        assert_eq!(period.priced_tokens, 400);
        assert!(period.has_data);

        // SOU-302: the same coverage feeds reset-window / calendar period cards.
        assert_eq!(pricing_coverage_tokens(&summary), (400, 500));
    }

    #[test]
    fn pricing_coverage_tokens_is_full_when_every_model_is_priced() {
        let mut summary = CostSummary {
            total_cost_usd: 2.0,
            ..Default::default()
        };
        summary.by_model_tokens.insert(
            "gpt-5.6-sol".to_string(),
            ModelTokenCounts {
                input_tokens: 200,
                output_tokens: 50,
                ..Default::default()
            },
        );
        assert_eq!(pricing_coverage_tokens(&summary), (250, 250));
    }

    #[test]
    fn local_usage_window_summary_carries_pricing_coverage() {
        let mut window_summary = CostSummary {
            total_cost_usd: 8.0,
            sessions_count: 1,
            ..Default::default()
        };
        window_summary
            .by_model
            .insert("gpt-5.6-sol".to_string(), 8.0);
        window_summary.by_model_tokens.insert(
            "gpt-5.6-sol".to_string(),
            ModelTokenCounts {
                input_tokens: 600,
                output_tokens: 200,
                ..Default::default()
            },
        );
        window_summary.by_model_tokens.insert(
            "gpt-mystery".to_string(),
            ModelTokenCounts {
                input_tokens: 200,
                output_tokens: 0,
                ..Default::default()
            },
        );
        window_summary
            .unknown_models
            .insert("gpt-mystery".to_string());

        let mut report = CostUsageReport {
            daily_costs: Vec::new(),
            thirty_days: window_summary.clone(),
            seven_days: window_summary.clone(),
            today: CostSummary::default(),
            latest_session: None,
            current_windows: Default::default(),
            hourly_activity: Vec::new(),
        };
        report
            .current_windows
            .insert("weekly".to_string(), window_summary);

        let summary = local_usage_summary_from_report(
            "codex",
            &report,
            &[LocalUsageWindowRequest {
                id: "weekly".to_string(),
                label: "Weekly".to_string(),
                starts_at: "2026-07-01T00:00:00Z".to_string(),
                ends_at: "2026-07-08T00:00:00Z".to_string(),
            }],
            &[],
        )
        .expect("summary");

        let window = summary
            .current_windows
            .iter()
            .find(|row| row.id == "weekly")
            .expect("weekly window");
        assert_eq!(window.cost, Some(8.0));
        assert_eq!(window.priced_tokens, 800);
        assert_eq!(window.total_model_tokens, 1000);
        assert_eq!(summary.seven_day_priced_tokens, 800);
        assert_eq!(summary.seven_day_total_model_tokens, 1000);
        assert_eq!(summary.thirty_day_priced_tokens, 800);
        assert_eq!(summary.thirty_day_total_model_tokens, 1000);
    }

    #[test]
    fn api_value_period_empty_summary_has_no_data() {
        let period = api_value_period("codex", &CostSummary::default());
        assert_eq!(period.api_value_usd, 0.0);
        assert_eq!(period.tokens, 0);
        assert_eq!(period.total_tokens, 0);
        assert_eq!(period.priced_tokens, 0);
        assert!(!period.has_data);
    }

    #[test]
    fn api_value_period_fully_priced_has_full_coverage() {
        let mut summary = CostSummary {
            total_cost_usd: 3.0,
            input_tokens: 200,
            output_tokens: 50,
            ..Default::default()
        };
        summary.by_model.insert("gpt-5.6-sol".to_string(), 3.0);
        summary.by_model_tokens.insert(
            "gpt-5.6-sol".to_string(),
            ModelTokenCounts {
                input_tokens: 200,
                output_tokens: 50,
                ..Default::default()
            },
        );

        let period = api_value_period("codex", &summary);

        // No unknown models: every token is priced.
        assert_eq!(period.priced_tokens, period.total_tokens);
        assert_eq!(period.total_tokens, 250);
        assert!(period.has_data);
    }

    #[test]
    fn local_yesterday_window_spans_one_local_day() {
        let (start, end) = local_yesterday_window_utc(Local::now());
        assert!(start < end);
        // A local calendar day is 24h, or 23h/25h across a DST transition.
        let hours = (end - start).num_hours();
        assert!((23..=25).contains(&hours), "unexpected span: {hours}h");
    }

    #[test]
    fn monthly_spend_budget_uses_calendar_month_not_rolling_thirty_days() {
        let date = NaiveDate::from_ymd_opt(2026, 7, 31).unwrap();
        let (cycle, label, start, days) = spend_budget_period_details(date, "monthly").unwrap();

        assert_eq!(cycle, "monthly:2026-07");
        assert_eq!(label, "Month to date");
        assert_eq!(start, NaiveDate::from_ymd_opt(2026, 7, 1).unwrap());
        assert_eq!(days, 31);
    }

    #[test]
    fn local_midnight_resolves_dst_gap_and_overlap() {
        // Several zones move their clocks at/around local midnight, so both a
        // skipped ("None") and an ambiguous ("Ambiguous") midnight exist. Find
        // real ones near the present rather than hard-coding transition dates.
        use chrono_tz::Tz;
        let zones: [Tz; 4] = [
            chrono_tz::America::Santiago,
            chrono_tz::America::Asuncion,
            chrono_tz::America::Havana,
            chrono_tz::Asia::Beirut,
        ];
        let find = |want_gap: bool| -> Option<(Tz, NaiveDate)> {
            for tz in zones {
                let mut date = NaiveDate::from_ymd_opt(2020, 1, 1).unwrap();
                let last = NaiveDate::from_ymd_opt(2027, 12, 31).unwrap();
                while date <= last {
                    let naive = date.and_hms_opt(0, 0, 0).unwrap();
                    match tz.from_local_datetime(&naive) {
                        LocalResult::None if want_gap => return Some((tz, date)),
                        LocalResult::Ambiguous(..) if !want_gap => return Some((tz, date)),
                        _ => {}
                    }
                    date += chrono::Duration::days(1);
                }
            }
            None
        };

        let (gap_tz, gap) = find(true).expect("a skipped midnight exists in some zone");
        // Skipped midnight resolves to the first local instant that exists,
        // never the naive-as-UTC fallback.
        let resolved = local_midnight_in_tz(&gap_tz, gap).with_timezone(&gap_tz);
        assert_eq!(resolved.date_naive(), gap);
        assert!(resolved.time() > NaiveTime::from_hms_opt(0, 0, 0).unwrap());
        assert!(matches!(
            gap_tz.from_local_datetime(&resolved.naive_local()),
            LocalResult::Single(_)
        ));

        let (ov_tz, overlap) = find(false).expect("an ambiguous midnight exists in some zone");
        // Ambiguous midnight picks the earliest of the two valid instants.
        let naive = overlap.and_hms_opt(0, 0, 0).unwrap();
        if let LocalResult::Ambiguous(earliest, _) = ov_tz.from_local_datetime(&naive) {
            assert_eq!(
                local_midnight_in_tz(&ov_tz, overlap),
                earliest.with_timezone(&Utc)
            );
        }
    }

    /// The seven-day headline stays a calendar total even though Compare reads
    /// a rolling window; the two are deliberately different periods.
    #[test]
    fn seven_day_summary_stays_a_calendar_period() {
        let report = CostUsageReport {
            seven_days: CostSummary {
                cache_read_tokens: 4_500_000_000,
                ..CostSummary::default()
            },
            thirty_days: CostSummary {
                sessions_count: 1,
                ..CostSummary::default()
            },
            ..CostUsageReport::default()
        };

        let summary = local_usage_summary_from_report("claude", &report, &[], &[])
            .expect("local usage summary");

        assert_eq!(summary.seven_day_tokens, Some(4_500_000_000));
    }

    /// Compare cannot render without these periods, so an empty list is a
    /// broken tab rather than a harmless default.
    #[test]
    fn rolling_comparison_periods_are_produced_for_compare() {
        let (specs, windows) = comparison_period_specs(Utc::now());
        assert_eq!(specs.len(), 2, "Compare renders a 5-hour and a 7-day card");
        assert_eq!(
            windows.len(),
            4,
            "each period needs a current and a prior window"
        );

        let mut current_windows = std::collections::HashMap::new();
        for (index, window) in windows.iter().enumerate() {
            current_windows.insert(
                window.id.clone(),
                CostSummary {
                    output_tokens: 1_000 * (index as u64 + 1),
                    sessions_count: 1,
                    ..CostSummary::default()
                },
            );
        }
        let report = CostUsageReport {
            thirty_days: CostSummary {
                sessions_count: 1,
                ..CostSummary::default()
            },
            current_windows,
            ..CostUsageReport::default()
        };

        let summary = local_usage_summary_from_report("claude", &report, &[], &specs)
            .expect("local usage summary");

        let ids: Vec<&str> = summary
            .comparison_periods
            .iter()
            .map(|period| period.id.as_str())
            .collect();
        assert_eq!(ids, vec!["five-hours", "seven-days"]);
        assert!(
            summary
                .comparison_periods
                .iter()
                .all(|period| period.current_tokens > 0 && period.previous_tokens > 0),
            "both the current and prior window must carry totals"
        );
    }

    /// A raw `now` would move the window every call and defeat the chart cache.
    #[test]
    fn rolling_windows_snap_to_the_minute_so_the_cache_can_hold() {
        let base = Utc::now()
            .with_second(0)
            .unwrap()
            .with_nanosecond(0)
            .unwrap();
        let (_, first) = comparison_period_specs(base + chrono::Duration::seconds(7));
        let (_, second) = comparison_period_specs(base + chrono::Duration::seconds(51));

        let ends: Vec<_> = first.iter().map(|window| window.ends_at).collect();
        let later: Vec<_> = second.iter().map(|window| window.ends_at).collect();
        assert_eq!(ends, later, "same minute must yield identical windows");
    }

    #[test]
    fn reset_aligned_windows_include_priced_cost_from_report() {
        let mut current_windows = std::collections::HashMap::new();
        current_windows.insert(
            "primary".to_string(),
            CostSummary {
                input_tokens: 1_000,
                output_tokens: 500,
                total_cost_usd: 12.4,
                sessions_count: 1,
                ..CostSummary::default()
            },
        );
        current_windows.insert(
            "secondary".to_string(),
            CostSummary {
                input_tokens: 10_000,
                output_tokens: 2_000,
                total_cost_usd: 0.0,
                sessions_count: 1,
                ..CostSummary::default()
            },
        );
        let report = CostUsageReport {
            thirty_days: CostSummary {
                sessions_count: 1,
                total_cost_usd: 12.4,
                ..CostSummary::default()
            },
            current_windows,
            ..CostUsageReport::default()
        };
        let requests = vec![
            LocalUsageWindowRequest {
                id: "primary".into(),
                label: "5-hour window".into(),
                starts_at: "2026-07-20T00:00:00Z".into(),
                ends_at: "2026-07-20T05:00:00Z".into(),
            },
            LocalUsageWindowRequest {
                id: "secondary".into(),
                label: "Weekly window".into(),
                starts_at: "2026-07-14T00:00:00Z".into(),
                ends_at: "2026-07-21T00:00:00Z".into(),
            },
        ];

        let summary = local_usage_summary_from_report("claude", &report, &requests, &[])
            .expect("local usage summary");

        assert_eq!(summary.current_windows.len(), 2);
        assert_eq!(summary.current_windows[0].id, "primary");
        assert_eq!(summary.current_windows[0].cost, Some(12.4));
        assert_eq!(summary.current_windows[1].id, "secondary");
        assert_eq!(summary.current_windows[1].cost, None);
    }

    #[test]
    fn claude_token_breakdown_includes_cache_reads_and_writes() {
        let summary = CostSummary {
            input_tokens: 2_000_000,
            output_tokens: 14_000_000,
            cached_tokens: 4_930_000_000,
            cache_read_tokens: 4_810_000_000,
            cache_write_tokens: 120_000_000,
            ..CostSummary::default()
        };

        assert_eq!(
            token_breakdown("claude", &summary),
            LocalTokenBreakdown {
                processed_tokens: 4_946_000_000,
                fresh_input_tokens: 2_000_000,
                output_tokens: 14_000_000,
                cache_read_tokens: 4_810_000_000,
                cache_write_tokens: 120_000_000,
                reasoning_tokens: 0,
            }
        );
    }

    #[test]
    fn codex_token_breakdown_does_not_double_count_cached_input() {
        let summary = CostSummary {
            input_tokens: 835_000_000,
            output_tokens: 2_000_000,
            cached_tokens: 808_000_000,
            cache_read_tokens: 808_000_000,
            ..CostSummary::default()
        };

        assert_eq!(
            token_breakdown("codex", &summary),
            LocalTokenBreakdown {
                processed_tokens: 837_000_000,
                fresh_input_tokens: 27_000_000,
                output_tokens: 2_000_000,
                cache_read_tokens: 808_000_000,
                cache_write_tokens: 0,
                reasoning_tokens: 0,
            }
        );
    }

    #[test]
    fn grok_token_breakdown_folds_cache_and_surfaces_reasoning() {
        let summary = CostSummary {
            input_tokens: 1000,
            output_tokens: 100,
            cached_tokens: 800,
            cache_read_tokens: 800,
            reasoning_tokens: 40,
            ..CostSummary::default()
        };
        assert_eq!(
            token_breakdown("grok", &summary),
            LocalTokenBreakdown {
                processed_tokens: 1100, // fresh 200 + output 100 + cache 800
                fresh_input_tokens: 200,
                output_tokens: 100,
                cache_read_tokens: 800,
                cache_write_tokens: 0,
                reasoning_tokens: 40,
            }
        );
    }

    #[test]
    fn english_estimate_note_is_localized() {
        assert_eq!(
            localized_estimate_note("codex", Language::English),
            "API-equivalent estimate from local logs; not subscription spend"
        );
        assert_eq!(
            localized_estimate_note("claude", Language::English),
            "API-equivalent estimate from local Claude logs; not subscription spend"
        );
    }
    #[test]
    fn parse_api_value_custom_range_accepts_inclusive_local_days() {
        let today = NaiveDate::from_ymd_opt(2026, 7, 28).unwrap();
        let (start, end) =
            parse_api_value_custom_range("2026-07-01", "2026-07-07", today).expect("range");
        assert_eq!(
            start.with_timezone(&Local).date_naive(),
            NaiveDate::from_ymd_opt(2026, 7, 1).unwrap()
        );
        // Exclusive end is the day after the inclusive until date.
        assert_eq!(
            end.with_timezone(&Local).date_naive(),
            NaiveDate::from_ymd_opt(2026, 7, 8).unwrap()
        );
    }

    #[test]
    fn parse_api_value_custom_range_rejects_inverted_and_future() {
        let today = NaiveDate::from_ymd_opt(2026, 7, 28).unwrap();
        assert!(parse_api_value_custom_range("2026-07-10", "2026-07-01", today).is_err());
        assert!(parse_api_value_custom_range("2026-07-01", "2026-08-01", today).is_err());
        assert!(parse_api_value_custom_range("not-a-date", "2026-07-01", today).is_err());
    }

    #[test]
    fn period_from_daily_series_sums_inclusive_range_only() {
        let series = daily_series_from_report(&[
            ("2026-07-01".into(), 10.0),
            ("2026-07-02".into(), 5.0),
            ("2026-07-15".into(), 1.0),
            ("2026-07-28".into(), 99.0),
        ]);
        let period = period_from_daily_series(
            &series,
            NaiveDate::from_ymd_opt(2026, 7, 1).unwrap(),
            NaiveDate::from_ymd_opt(2026, 7, 15).unwrap(),
        );
        assert!(period.has_data);
        assert!((period.api_value_usd - 16.0).abs() < f64::EPSILON);
    }

    /// SBS-279: today must stay out of its own baseline. Including it lets a
    /// spike raise the bar it is measured against, so the bigger the runaway
    /// the less likely the alert is to fire.
    #[test]
    fn spend_anomaly_reading_excludes_today_from_the_baseline() {
        let today = NaiveDate::from_ymd_opt(2026, 8, 15).unwrap();
        let mut daily = HashMap::new();
        daily.insert("2026-08-15".to_string(), 90.0);
        for offset in 1..=7 {
            let date = (today - chrono::Duration::days(offset))
                .format("%Y-%m-%d")
                .to_string();
            daily.insert(date, 2.0);
        }
        // Outside the window: must not move the median.
        daily.insert("2026-08-01".to_string(), 500.0);

        let reading = spend_anomaly_reading(today, &daily).expect("a reading");

        assert_eq!(reading.day_id, "2026-08-15");
        assert_eq!(reading.today_usd, 90.0);
        assert_eq!(reading.baseline_usd, 2.0);
    }

    /// Days the scan never saw are quiet days, not missing data: a machine that
    /// was off all week has a zero baseline, which the detector treats as "no
    /// comparison" rather than an infinite spike.
    #[test]
    fn spend_anomaly_reading_treats_absent_days_as_zero() {
        let today = NaiveDate::from_ymd_opt(2026, 8, 15).unwrap();
        let reading = spend_anomaly_reading(today, &HashMap::new()).expect("a reading");

        assert_eq!(reading.today_usd, 0.0);
        assert_eq!(reading.baseline_usd, 0.0);
    }

    /// A three-day working week is four quiet days and three real ones. Those
    /// quiet days must not drag the median to zero, because a zero baseline
    /// switches the detector off — the most ordinary schedule there is would
    /// otherwise have disabled the alert entirely.
    #[test]
    fn spend_anomaly_reading_ignores_quiet_days_in_the_baseline() {
        let today = NaiveDate::from_ymd_opt(2026, 8, 15).unwrap();
        let mut daily = HashMap::new();
        daily.insert("2026-08-15".to_string(), 80.0);
        // Three working days last week, the rest absent from the scan.
        for offset in 1..=3 {
            let date = (today - chrono::Duration::days(offset))
                .format("%Y-%m-%d")
                .to_string();
            daily.insert(date, 20.0);
        }

        let reading = spend_anomaly_reading(today, &daily).expect("a reading");

        assert_eq!(reading.today_usd, 80.0);
        assert_eq!(
            reading.baseline_usd, 20.0,
            "the four quiet days must not count as $0 workdays"
        );
    }

    #[test]
    fn period_from_daily_series_empty_when_range_misses() {
        let series = daily_series_from_report(&[("2026-06-01".into(), 10.0)]);
        let period = period_from_daily_series(
            &series,
            NaiveDate::from_ymd_opt(2026, 7, 1).unwrap(),
            NaiveDate::from_ymd_opt(2026, 7, 15).unwrap(),
        );
        assert!(!period.has_data);
        assert_eq!(period.api_value_usd, 0.0);
    }

    /// SBS-277: the day axis comes from the calendar, not from the scan, so an
    /// idle machine still renders a full grid instead of collapsing to nothing.
    #[test]
    fn activity_heatmap_days_span_a_full_window_oldest_first() {
        let days = activity_heatmap_days(NaiveDate::from_ymd_opt(2026, 8, 15).unwrap());

        assert_eq!(days.len(), ACTIVITY_HEATMAP_DAYS as usize);
        assert_eq!(days.first().unwrap(), "2026-07-17");
        assert_eq!(days.last().unwrap(), "2026-08-15");
        let mut sorted = days.clone();
        sorted.sort();
        assert_eq!(sorted, days, "days read oldest first");
    }

    fn hourly_point(hour: u32, summary: CostSummary) -> HourlyActivityPoint {
        HourlyActivityPoint {
            date: NaiveDate::from_ymd_opt(2026, 8, 15).unwrap(),
            hour,
            summary,
        }
    }

    /// An hour from a day the strip does not draw must not reach the grid.
    ///
    /// The two views are the same data asked two ways, so an hour the calendar
    /// cannot show would read as activity on a day that is not there.
    #[test]
    fn heatmap_hours_keep_only_the_days_on_the_axis() {
        let mut busy = CostSummary {
            total_cost_usd: 1.0,
            ..CostSummary::default()
        };
        busy.by_model_tokens.insert(
            "gpt-5".to_string(),
            ModelTokenCounts {
                input_tokens: 10,
                output_tokens: 5,
                calls: 1,
                ..ModelTokenCounts::default()
            },
        );
        let report = CostUsageReport {
            hourly_activity: vec![
                HourlyActivityPoint {
                    date: NaiveDate::from_ymd_opt(2026, 8, 15).unwrap(),
                    hour: 9,
                    summary: busy.clone(),
                },
                HourlyActivityPoint {
                    date: NaiveDate::from_ymd_opt(2026, 6, 1).unwrap(),
                    hour: 9,
                    summary: busy,
                },
            ],
            ..CostUsageReport::default()
        };

        let hours = heatmap_hours_for_days("codex", &report, &["2026-08-15".to_string()]);

        assert_eq!(hours.len(), 1);
        assert_eq!(hours[0].date, "2026-08-15");
    }

    /// An hour with no tokens, dollars, or calls is not activity. Emitting it
    /// would darken a cell that should read as idle.
    #[test]
    fn activity_hours_drop_empty_buckets_and_normalize_tokens() {
        let mut busy = CostSummary {
            total_cost_usd: 1.25,
            input_tokens: 1_000,
            output_tokens: 200,
            // Codex folds cache reads into its input count, so processed tokens
            // must not add these twice.
            cached_tokens: 800,
            ..Default::default()
        };
        busy.by_model_tokens.insert(
            "gpt-5.1-codex".to_string(),
            ModelTokenCounts {
                input_tokens: 1_000,
                output_tokens: 200,
                calls: 3,
                ..Default::default()
            },
        );
        let report = CostUsageReport {
            hourly_activity: vec![
                hourly_point(9, busy),
                hourly_point(10, CostSummary::default()),
            ],
            ..Default::default()
        };

        let hours = activity_hours_for_provider("codex", &report);

        assert_eq!(hours.len(), 1, "the empty 10:00 bucket is dropped");
        let point = &hours[0];
        assert_eq!(point.hour, 9);
        assert_eq!(point.provider_id, "codex");
        assert_eq!(point.date, "2026-08-15");
        assert_eq!(point.calls, 3);
        assert!((point.api_value_usd - 1.25).abs() < f64::EPSILON);
        // 200 fresh input + 200 output + 800 cache read, each counted once.
        assert_eq!(point.tokens, 1_200);
    }

    #[test]
    fn activity_heatmap_merges_providers_into_one_sorted_timeline() {
        let row = |provider_id: &str, date: &str, hour: u32| ActivityHourPoint {
            provider_id: provider_id.to_string(),
            date: date.to_string(),
            hour,
            api_value_usd: 1.0,
            tokens: 10,
            calls: 1,
        };

        let heatmap = assemble_activity_heatmap(
            vec!["2026-08-14".to_string(), "2026-08-15".to_string()],
            vec![
                vec![
                    row("codex", "2026-08-15", 9),
                    row("codex", "2026-08-14", 22),
                ],
                // A provider with nothing to show must not appear as a chip.
                Vec::new(),
                vec![row("grok", "2026-08-14", 22)],
            ],
            "UTC+00:00".to_string(),
        );

        assert_eq!(heatmap.provider_ids, vec!["codex", "grok"]);
        assert_eq!(
            heatmap
                .hours
                .iter()
                .map(|point| (point.date.as_str(), point.hour, point.provider_id.as_str()))
                .collect::<Vec<_>>(),
            vec![
                ("2026-08-14", 22, "codex"),
                ("2026-08-14", 22, "grok"),
                ("2026-08-15", 9, "codex"),
            ],
        );
    }
}
