//! Provider chart data commands and DTOs.
//!
//! Cost history comes from the shared JSONL cost scanner and is available for
//! every provider. Credits history + usage breakdowns currently only apply to
//! the Codex / OpenAI dashboard cache and require an `account_email` to scope
//! reads to the right cached bundle.

use chrono::{DateTime, Datelike, Local, LocalResult, NaiveDate, TimeZone, Utc};
use codexbar::core::OpenAIDashboardCacheStore;
use codexbar::cost_scanner::{
    CostScanner, CostSummary, CostUsageReport, CurrentUsageWindow,
    get_cost_usage_report_with_windows,
};
use codexbar::locale::{self, LocaleKey};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::PathBuf;
use std::sync::{
    Arc, Mutex, OnceLock,
    atomic::{AtomicBool, Ordering},
};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

const LOCAL_USAGE_TTL: Duration = Duration::from_secs(30);
const CHART_CACHE_TTL: Duration = Duration::from_secs(5 * 60);
// Version 5 aligns the provider summary's seven-day total with the exact
// rolling 168-hour window used by Compare. Older entries used calendar days.
const CHART_CACHE_VERSION: u8 = 5;

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
    pub thirty_day_cost: Option<f64>,
    pub thirty_day_tokens: Option<u64>,
    #[serde(default)]
    pub thirty_day_token_breakdown: Option<LocalTokenBreakdown>,
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
    pub quota_history: Vec<crate::usage_history::UsageHistoryPoint>,
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

#[tauri::command]
pub async fn get_provider_chart_data(
    provider_id: String,
    account_email: Option<String>,
    source_label: Option<String>,
    usage_windows: Option<Vec<LocalUsageWindowRequest>>,
) -> ProviderChartData {
    let usage_windows = usage_windows.unwrap_or_default();
    let cache_key = chart_cache_key(
        &provider_id,
        account_email.as_deref(),
        source_label.as_deref(),
        &usage_windows,
    );
    if let Some(mut cached) = cached_chart_data(&cache_key) {
        cached.data.quota_history =
            crate::usage_history::provider_history(&provider_id, account_email.as_deref());
        if current_unix_ms().saturating_sub(cached.refreshed_at_ms)
            > CHART_CACHE_TTL.as_millis() as i64
        {
            schedule_chart_cache_refresh(cache_key, provider_id, account_email, usage_windows);
        }
        return cached.data;
    }

    let quota_history =
        crate::usage_history::provider_history(&provider_id, account_email.as_deref());
    if !quota_history.is_empty() {
        let mut immediate = ProviderChartData::empty(provider_id.clone());
        immediate.quota_history = quota_history;
        schedule_chart_cache_refresh(cache_key, provider_id, account_email, usage_windows);
        return immediate;
    }

    let fallback_provider_id = provider_id.clone();
    let cancel = register_chart_scan(&provider_id);
    let result = tauri::async_runtime::spawn_blocking(move || {
        build_provider_chart_data_with_cancel(
            provider_id,
            account_email,
            usage_windows,
            Some(cancel),
        )
    })
    .await
    .unwrap_or_else(|err| {
        tracing::warn!("Provider chart data worker failed: {}", err);
        ProviderChartData::empty(fallback_provider_id)
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
        key,
        CachedProviderChartData {
            refreshed_at_ms: current_unix_ms(),
            data,
        },
    );
    persist_chart_cache(&guard);
}

fn schedule_chart_cache_refresh(
    key: String,
    provider_id: String,
    account_email: Option<String>,
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
            build_provider_chart_data_with_cancel(provider_id, account_email, usage_windows, None)
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
    source_label: Option<&str>,
    usage_windows: &[LocalUsageWindowRequest],
) -> String {
    let identity = account_email
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or("anonymous")
        .to_ascii_lowercase();
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
    fs::read(path)
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .filter(|cache: &PersistedChartCache| cache.version == CHART_CACHE_VERSION)
        .unwrap_or_default()
}

fn persist_chart_cache(cache: &PersistedChartCache) {
    let Some(path) = chart_cache_path() else {
        return;
    };
    if let Some(parent) = path.parent()
        && let Err(error) = fs::create_dir_all(parent)
    {
        tracing::warn!("failed to create chart cache directory: {error}");
        return;
    }
    match serde_json::to_vec(cache) {
        Ok(bytes) => {
            if let Err(error) = fs::write(path, bytes) {
                tracing::warn!("failed to persist chart cache: {error}");
            }
        }
        Err(error) => tracing::warn!("failed to serialize chart cache: {error}"),
    }
}

/// Providers that expose token-derived local usage for the aggregate card.
/// Inclusion is by capability, not by merely having some other dollar balance.
const API_VALUE_PROVIDERS: [&str; 2] = ["codex", "claude"];

/// Aggregate one period's usage for one provider into the card shape.
fn api_value_period(provider_id: &str, summary: &CostSummary) -> LocalApiValuePeriod {
    let processed = token_breakdown(provider_id, summary).processed_tokens;
    let total_tokens: u64 = summary
        .by_model_tokens
        .values()
        .map(|counts| counts.total())
        .sum();
    // Unpriced models are exactly those the scanner flagged as unknown; their
    // tokens still count toward the total but not toward priced coverage.
    let unpriced_tokens: u64 = summary
        .unknown_models
        .iter()
        .filter_map(|model| summary.by_model_tokens.get(model))
        .map(|counts| counts.total())
        .sum();
    let priced_tokens = total_tokens.saturating_sub(unpriced_tokens);
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

#[tauri::command]
pub async fn get_local_api_value_totals() -> Result<Vec<LocalApiValueProvider>, String> {
    // A worker panic/cancel must surface as an error, not an empty result —
    // "unavailable" and "genuinely no data" are distinct on this card.
    tauri::async_runtime::spawn_blocking(|| load_local_api_value_totals(Local::now()))
        .await
        .map_err(|err| {
            tracing::warn!("Local API-value totals worker failed: {}", err);
            "Unable to load local API-value totals.".to_string()
        })
}

fn load_local_api_value_totals(now: DateTime<Local>) -> Vec<LocalApiValueProvider> {
    let today = now.date_naive();
    let (yesterday_start, yesterday_end) = local_yesterday_window_utc(now);
    // Exact [start, end) windows so thirty-day and prior-thirty stay adjacent
    // and each spans exactly 30 calendar days (including today for "thirty").
    // [today-29, tomorrow) = today-29 … today; [today-59, today-29) = prior 30.
    let thirty_start = local_midnight_utc(today - chrono::Duration::days(29));
    let thirty_end = local_midnight_utc(today + chrono::Duration::days(1));
    let prior_start = local_midnight_utc(today - chrono::Duration::days(59));
    API_VALUE_PROVIDERS
        .iter()
        .filter_map(|provider_id| {
            let windows = vec![
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
            let report = get_cost_usage_report_with_windows(provider_id, 60, &windows)?;
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
            let provider = LocalApiValueProvider {
                provider_id: provider_id.to_string(),
                today: api_value_period(provider_id, &report.today),
                yesterday: api_value_period(provider_id, &yesterday),
                thirty_days: api_value_period(provider_id, &thirty_days),
                prior_thirty_days: api_value_period(provider_id, &prior_thirty_days),
            };
            // Omit providers with no source data in any period.
            (provider.today.has_data
                || provider.yesterday.has_data
                || provider.thirty_days.has_data
                || provider.prior_thirty_days.has_data)
                .then_some(provider)
        })
        .collect()
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

#[tauri::command]
pub async fn get_cursor_model_activity() -> Vec<CursorModelActivityRow> {
    tauri::async_runtime::spawn_blocking(|| {
        codexbar::cursor_activity::cursor_model_activity(current_unix_ms(), 30)
            .into_iter()
            .map(|activity| CursorModelActivityRow {
                model: activity.model,
                contributions: activity.contributions,
                requests: activity.requests,
            })
            .collect()
    })
    .await
    .unwrap_or_else(|err| {
        tracing::warn!("Cursor model-activity worker failed: {}", err);
        Vec::new()
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
    build_provider_chart_data_with_cancel(provider_id, account_email, Vec::new(), None)
}

fn build_provider_chart_data_with_cancel(
    provider_id: String,
    account_email: Option<String>,
    usage_window_requests: Vec<LocalUsageWindowRequest>,
    cancel: Option<Arc<AtomicBool>>,
) -> ProviderChartData {
    let usage_windows = usage_window_requests
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
    let report = get_cost_usage_report_with_windows(&provider_id, 30, &usage_windows);
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
        report
            .as_ref()
            .and_then(|report| {
                local_usage_summary_from_report(&provider_id, report, &usage_window_requests)
            })
            .or_else(|| load_local_usage_summary_cached(&provider_id, cancel.as_deref()))
    };

    ProviderChartData {
        quota_history: crate::usage_history::provider_history(
            &provider_id,
            account_email.as_deref(),
        ),
        provider_id,
        cost_history,
        credits_history,
        usage_breakdown,
        local_usage,
    }
}

fn local_usage_summary_from_report(
    provider_id: &str,
    report: &CostUsageReport,
    usage_window_requests: &[LocalUsageWindowRequest],
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
    let current_windows = usage_window_requests
        .iter()
        .filter_map(|window| {
            let summary = report.current_windows.get(&window.id)?;
            let token_breakdown = token_breakdown(provider_id, summary);
            Some(LocalUsageWindowSummary {
                id: window.id.clone(),
                label: window.label.clone(),
                starts_at: window.starts_at.clone(),
                ends_at: window.ends_at.clone(),
                tokens: token_breakdown.processed_tokens,
                token_breakdown,
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
        thirty_day_cost: non_zero_f64(report.thirty_days.total_cost_usd),
        thirty_day_tokens: non_zero_u64(thirty_day_tokens),
        thirty_day_token_breakdown: Some(thirty_day_breakdown),
        current_windows,
        comparison_periods: Vec::new(),
        latest_tokens: non_zero_u64(last_session_tokens),
        top_model: top_model(&report.thirty_days),
        model_breakdown: model_breakdown(&report.thirty_days),
        effort_breakdown: effort_breakdown(&report.thirty_days),
        project_breakdown: project_breakdown(&report.thirty_days),
        estimate_note: localized_estimate_note(provider_id, lang),
        token_cost_updated_at_ms: current_unix_ms(),
    })
}

impl ProviderChartData {
    fn empty(provider_id: String) -> Self {
        Self {
            provider_id,
            cost_history: Vec::new(),
            credits_history: Vec::new(),
            usage_breakdown: Vec::new(),
            local_usage: None,
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
    (
        Some(ProviderLocalUsageSummary {
            today_cost: non_zero_f64(today.total_cost_usd),
            last_session_cost: None,
            last_session_tokens: non_zero_u64(latest_tokens),
            last_session_token_breakdown: Some(latest_breakdown),
            seven_day_cost: None,
            seven_day_tokens: None,
            seven_day_token_breakdown: None,
            thirty_day_cost: non_zero_f64(thirty_day.total_cost_usd),
            thirty_day_tokens: non_zero_u64(thirty_day_tokens),
            thirty_day_token_breakdown: Some(thirty_day_breakdown),
            current_windows: Vec::new(),
            comparison_periods: Vec::new(),
            latest_tokens: non_zero_u64(latest_tokens),
            top_model: top_model(&thirty_day),
            model_breakdown: model_breakdown(&thirty_day),
            effort_breakdown: effort_breakdown(&thirty_day),
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
        _ => None,
    }
}

fn token_breakdown(provider_id: &str, summary: &CostSummary) -> LocalTokenBreakdown {
    let is_codex = provider_id.eq_ignore_ascii_case("codex");
    let cache_read_tokens =
        summary
            .cache_read_tokens
            .max(if is_codex { summary.cached_tokens } else { 0 });
    let cache_write_tokens = summary.cache_write_tokens;
    let fresh_input_tokens = if is_codex {
        summary.input_tokens.saturating_sub(cache_read_tokens)
    } else {
        summary.input_tokens
    };
    let processed_tokens = fresh_input_tokens
        .saturating_add(summary.output_tokens)
        .saturating_add(cache_read_tokens)
        .saturating_add(cache_write_tokens);
    LocalTokenBreakdown {
        processed_tokens,
        fresh_input_tokens,
        output_tokens: summary.output_tokens,
        cache_read_tokens,
        cache_write_tokens,
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
fn model_breakdown(summary: &CostSummary) -> Vec<LocalModelCost> {
    let mut rows: Vec<LocalModelCost> = summary
        .by_model_tokens
        .iter()
        .map(|(model, counts)| {
            let cost = summary.by_model.get(model).copied();
            let processed = counts.processed();
            let cache_read_percent = (processed > 0)
                .then_some((counts.cache_read_tokens as f64 / processed as f64) * 100.0);
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
        CostFetchFailure, LocalEffortCost, LocalModelCost, LocalProjectCost, LocalTokenBreakdown,
        ProviderLocalUsageSummary, api_value_period, cost_fetch_failure_allows_early_retry,
        effort_breakdown, format_cost_csv, local_midnight_in_tz, local_usage_summary_from_report,
        local_yesterday_window_utc, localized_estimate_note, model_breakdown, project_breakdown,
        spend_budget_period_details, token_breakdown, token_cost_cache_is_fresh,
    };
    use crate::commands::is_provider_cache_fresh;
    use chrono::{Local, LocalResult, NaiveDate, NaiveTime, TimeZone, Utc};
    use codexbar::cost_scanner::{CostSummary, CostUsageReport, ModelTokenCounts};
    use codexbar::settings::Language;
    use std::time::{Duration, Instant};

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
            thirty_day_cost: Some(2.0),
            thirty_day_tokens: Some(300),
            thirty_day_token_breakdown: None,
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

        let rows = model_breakdown(&summary);

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

    #[test]
    fn seven_day_summary_stays_a_calendar_period_without_rolling_comparison() {
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

        let summary =
            local_usage_summary_from_report("claude", &report, &[]).expect("local usage summary");

        assert_eq!(summary.seven_day_tokens, Some(4_500_000_000));
        assert!(summary.comparison_periods.is_empty());
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
}
