//! Permanent snapshots of completed quota runs (SOU-298).
//!
//! A run is the span from the first live observation after a window is low (or
//! after a prior reset) until a confirmed capacity reset ends it. Snapshots are
//! local, bounded, and honest about partial observation (app restart / away).

use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};

use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};

use crate::capacity_events::{CapacityEventKind, CapacityEventPayload};
use crate::commands::{ProviderUsageSnapshot, RateWindowSnapshot};

const STORE_VERSION: u8 = 2;
/// Keep enough history for run-over-run efficiency (SOU-299) without unbounded growth.
const MAX_RUNS_PER_SCOPE: usize = 40;
const RETENTION_DAYS: i64 = 120;
/// First sample above this is treated as joining mid-cycle (partial run).
const MID_CYCLE_START_USED: f64 = 15.0;
/// Used-percent drop that closes an open run even without a capacity event
/// (defensive; capacity_events is the primary closer).
const USED_DROP_CLOSE: f64 = 20.0;
/// Need a real climb on the meter before tokens-per-1% is meaningful.
const MIN_PEAK_FOR_TOKENS_PER_PERCENT: f64 = 5.0;
/// Suppress wild extrapolations early in a run (SOU-299).
const MIN_PEAK_FOR_PROJECTION: f64 = 25.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum QuotaRunResetKind {
    Scheduled,
    Surprise,
    Partial,
    /// Closed from a usage drop without a classified capacity event.
    ObservedDrop,
}

impl QuotaRunResetKind {
    fn from_capacity(kind: CapacityEventKind) -> Option<Self> {
        match kind {
            CapacityEventKind::ScheduledReset => Some(Self::Scheduled),
            CapacityEventKind::SurpriseReset => Some(Self::Surprise),
            CapacityEventKind::PartialReset => Some(Self::Partial),
            _ => None,
        }
    }
}

/// One completed (or partial) quota run for a single rate window.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct QuotaRunSnapshot {
    pub id: String,
    pub provider_id: String,
    pub display_name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub account_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub account_email: Option<String>,
    pub window_id: String,
    pub window_label: String,
    /// Best-effort start of continuous observation for this cycle.
    pub started_at: String,
    /// When the run ended (reset confirmed or usage drop observed).
    pub ended_at: String,
    /// Peak used % seen during continuous observation (authoritative meter).
    pub peak_used_percent: f64,
    /// Used % on the sample immediately before the ending reset/drop.
    pub end_used_percent: f64,
    /// Used % after the reset, when known from a capacity event.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub after_reset_used_percent: Option<f64>,
    pub reset_kind: QuotaRunResetKind,
    /// Window length when the provider reports it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub window_minutes: Option<u32>,
    /// Wall-clock span of continuous observation in seconds.
    pub observed_duration_seconds: i64,
    /// True when Ceiling watched from a low used % through the ending reset
    /// without a process restart gap.
    pub complete: bool,
    /// True when the ending event was detected after Ceiling was closed.
    #[serde(default)]
    pub while_away: bool,
    /// True when the open run was resumed after a process restart.
    #[serde(default)]
    pub interrupted: bool,
    /// Best-effort local processed tokens for this window during the run.
    /// Machine-wide when logs lack account identity (see Charts disclosure).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub processed_tokens: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fresh_input_tokens: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output_tokens: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache_read_tokens: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache_write_tokens: Option<u64>,
}

/// Efficiency read derived from a completed run (SOU-299).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct QuotaRunEfficiency {
    pub run: QuotaRunSnapshot,
    /// Locally observed processed tokens per 1% of provider-reported used.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tokens_per_percent: Option<f64>,
    /// Cache-read share of processed tokens during the run (0–100).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache_read_percent: Option<f64>,
    /// Extrapolated processed tokens if the meter reached 100% at this rate.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub projected_tokens_at_100: Option<u64>,
    /// Relative change vs previous complete run on the same window
    /// (`-0.2` = 20% fewer tokens per 1%).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub vs_previous_tokens_per_percent: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub previous_run_id: Option<String>,
    /// Honesty label for the UI.
    pub note: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct OpenRun {
    provider_id: String,
    display_name: String,
    #[serde(default)]
    account_id: Option<String>,
    #[serde(default)]
    account_email: Option<String>,
    window_id: String,
    window_label: String,
    started_at: DateTime<Utc>,
    last_observed_at: DateTime<Utc>,
    peak_used_percent: f64,
    last_used_percent: f64,
    #[serde(default)]
    window_minutes: Option<u32>,
    /// Joined the cycle after used was already elevated.
    #[serde(default)]
    mid_cycle_start: bool,
    /// Loaded from disk after a previous process exit.
    #[serde(default)]
    interrupted: bool,
    #[serde(default)]
    processed_tokens: Option<u64>,
    #[serde(default)]
    fresh_input_tokens: Option<u64>,
    #[serde(default)]
    output_tokens: Option<u64>,
    #[serde(default)]
    cache_read_tokens: Option<u64>,
    #[serde(default)]
    cache_write_tokens: Option<u64>,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct QuotaRunStore {
    #[serde(default)]
    version: u8,
    /// Completed runs keyed by observation scope (provider + account identity).
    #[serde(default)]
    runs: HashMap<String, Vec<QuotaRunSnapshot>>,
    /// In-progress runs keyed by `scope:window_id`.
    #[serde(default)]
    open: HashMap<String, OpenRun>,
}

fn store() -> &'static Mutex<QuotaRunStore> {
    static STORE: OnceLock<Mutex<QuotaRunStore>> = OnceLock::new();
    STORE.get_or_init(|| Mutex::new(load_store()))
}

/// Update open-run tracking from a live provider reading.
pub fn record_snapshot(snapshot: &ProviderUsageSnapshot) {
    if snapshot.error.is_some() {
        return;
    }
    let observed_at = DateTime::parse_from_rfc3339(&snapshot.updated_at)
        .map(|value| value.with_timezone(&Utc))
        .unwrap_or_else(|_| Utc::now());
    let scope = scope_key(snapshot);
    let windows = live_windows(snapshot);
    if windows.is_empty() {
        return;
    }

    let Ok(mut guard) = store().lock() else {
        return;
    };
    guard.version = STORE_VERSION;

    for window in windows {
        let open_key = format!("{}:{}", scope, window.id);
        if let Some(open) = guard.open.get_mut(&open_key) {
            let drop = open.last_used_percent - window.used_percent;
            if drop >= USED_DROP_CLOSE {
                // Capacity events should finalize with a classified kind. If we
                // only see a drop here, still close so runs are not lost.
                let finished = finalize_open(
                    open.clone(),
                    observed_at,
                    window.used_percent,
                    QuotaRunResetKind::ObservedDrop,
                    false,
                    None,
                );
                push_run(&mut guard, &scope, finished);
                guard.open.remove(&open_key);
                // Start a new cycle from the low post-drop reading.
                let mut next = new_open(snapshot, &window, observed_at);
                attach_local_tokens(&mut next);
                guard.open.insert(open_key, next);
                continue;
            }
            open.last_observed_at = observed_at;
            open.last_used_percent = window.used_percent;
            open.peak_used_percent = open.peak_used_percent.max(window.used_percent);
            if open.window_minutes.is_none() {
                open.window_minutes = window.window_minutes;
            }
            // Prefer fresher labels/identity without changing keys.
            open.display_name = snapshot.display_name.clone();
            open.window_label = window.label.clone();
            if open.account_id.is_none() {
                open.account_id = snapshot.account_id.clone();
            }
            if open.account_email.is_none() {
                open.account_email = snapshot.account_email.clone();
            }
            // Keep last pre-reset local totals (post-reset windows read near 0).
            attach_local_tokens(open);
        } else {
            let mut open = new_open(snapshot, &window, observed_at);
            attach_local_tokens(&mut open);
            guard.open.insert(open_key, open);
        }
    }
    persist_store(&guard);
}

/// Finalize open runs when capacity events confirm a reset (SOU-298).
pub fn record_capacity_events(events: &[CapacityEventPayload], snapshot: &ProviderUsageSnapshot) {
    if events.is_empty() {
        return;
    }
    let scope = scope_key(snapshot);
    let Ok(mut guard) = store().lock() else {
        return;
    };
    guard.version = STORE_VERSION;
    let mut wrote = false;
    for event in events {
        let Some(reset_kind) = QuotaRunResetKind::from_capacity(event.kind) else {
            continue;
        };
        if !event
            .provider_id
            .eq_ignore_ascii_case(&snapshot.provider_id)
        {
            continue;
        }
        let open_key = format!("{}:{}", scope, event.window_id);
        let ended_at = DateTime::parse_from_rfc3339(&event.occurred_at)
            .map(|value| value.with_timezone(&Utc))
            .unwrap_or_else(|_| Utc::now());
        let finished = if let Some(open) = guard.open.remove(&open_key) {
            finalize_open(
                open,
                ended_at,
                event.previous_used_percent,
                reset_kind,
                event.while_away,
                Some(event.current_used_percent),
            )
        } else {
            // No open run (restart between samples, or away path). Still record
            // a partial snapshot from the event alone.
            partial_from_event(event, snapshot, ended_at, reset_kind)
        };
        push_run(&mut guard, &scope, finished);
        wrote = true;

        // Open a fresh cycle after a confirmed reset when the current reading
        // is already post-reset.
        if let Some(window) = live_windows(snapshot)
            .into_iter()
            .find(|window| window.id == event.window_id)
        {
            let observed_at = DateTime::parse_from_rfc3339(&snapshot.updated_at)
                .map(|value| value.with_timezone(&Utc))
                .unwrap_or(ended_at);
            let mut next = new_open(snapshot, &window, observed_at);
            attach_local_tokens(&mut next);
            guard.open.insert(open_key, next);
        }
    }
    if wrote {
        persist_store(&guard);
    }
}

/// Completed runs for a provider/account, chronological (oldest first).
pub fn list_runs(provider_id: &str, account_email: Option<&str>) -> Vec<QuotaRunSnapshot> {
    let Ok(guard) = store().lock() else {
        return Vec::new();
    };
    let exact = scope_key_parts(provider_id, account_email, None, None);
    if let Some(runs) = guard.runs.get(&exact) {
        return runs.clone();
    }
    // Fall back to anonymous series for this provider only (never another seat).
    if account_email.is_some() {
        let anonymous = scope_key_parts(provider_id, None, None, None);
        if let Some(runs) = guard.runs.get(&anonymous) {
            return runs.clone();
        }
    }
    Vec::new()
}

/// Efficiency cards for Charts: latest complete runs per window, newest first.
pub fn efficiency_for_provider(
    provider_id: &str,
    account_email: Option<&str>,
) -> Vec<QuotaRunEfficiency> {
    let runs = list_runs(provider_id, account_email);
    if runs.is_empty() {
        return Vec::new();
    }

    // Index previous complete run per window for deltas.
    let mut previous_complete: HashMap<String, &QuotaRunSnapshot> = HashMap::new();
    let mut rows: Vec<QuotaRunEfficiency> = Vec::new();

    for run in &runs {
        let prev = previous_complete.get(&run.window_id).copied();
        let row = efficiency_for_run(run, prev);
        if run.complete {
            previous_complete.insert(run.window_id.clone(), run);
        }
        // Surface complete runs, plus incomplete ones that still have tokens so
        // the user sees something while history accumulates.
        if run.complete || run.processed_tokens.is_some() {
            rows.push(row);
        }
    }

    // Newest first for the UI; keep one card per window (latest complete preferred).
    rows.reverse();
    let mut seen_windows = HashSet::new();
    rows.retain(|row| {
        if !row.run.complete && row.run.processed_tokens.is_none() {
            return false;
        }
        // Prefer first occurrence after reverse = newest.
        if seen_windows.contains(&row.run.window_id) {
            return false;
        }
        seen_windows.insert(row.run.window_id.clone());
        true
    });
    rows
}

fn efficiency_for_run(
    run: &QuotaRunSnapshot,
    previous: Option<&QuotaRunSnapshot>,
) -> QuotaRunEfficiency {
    let rate = tokens_per_percent(run.processed_tokens, run.peak_used_percent);
    let cache_share = cache_read_percent(
        run.processed_tokens,
        run.cache_read_tokens,
        run.fresh_input_tokens,
        run.output_tokens,
        run.cache_write_tokens,
    );
    let projected_tokens_at_100 = if run.peak_used_percent >= MIN_PEAK_FOR_PROJECTION {
        rate.map(|value| (value * 100.0).round() as u64)
    } else {
        None
    };

    let mut vs_previous = None;
    let mut previous_run_id = None;
    if let (Some(rate), Some(prev)) = (rate, previous)
        && let Some(prev_rate) = tokens_per_percent(prev.processed_tokens, prev.peak_used_percent)
        && prev_rate > 0.0
    {
        vs_previous = Some((rate - prev_rate) / prev_rate);
        previous_run_id = Some(prev.id.clone());
    }

    QuotaRunEfficiency {
        run: run.clone(),
        tokens_per_percent: rate,
        cache_read_percent: cache_share,
        projected_tokens_at_100,
        vs_previous_tokens_per_percent: vs_previous,
        previous_run_id,
        note: efficiency_note(run),
    }
}

fn tokens_per_percent(processed: Option<u64>, peak_used: f64) -> Option<f64> {
    let tokens = processed.filter(|value| *value > 0)?;
    if peak_used < MIN_PEAK_FOR_TOKENS_PER_PERCENT {
        return None;
    }
    Some(tokens as f64 / peak_used)
}

fn cache_read_percent(
    processed: Option<u64>,
    cache_read: Option<u64>,
    fresh: Option<u64>,
    output: Option<u64>,
    cache_write: Option<u64>,
) -> Option<f64> {
    let cache = cache_read.unwrap_or(0);
    let total = processed.unwrap_or_else(|| {
        fresh.unwrap_or(0) + output.unwrap_or(0) + cache + cache_write.unwrap_or(0)
    });
    if total == 0 {
        return None;
    }
    Some((cache as f64 / total as f64) * 100.0)
}

fn efficiency_note(run: &QuotaRunSnapshot) -> String {
    if run.processed_tokens.is_none() {
        return "No local token sample was captured for this run yet.".into();
    }
    if !run.complete {
        return "Partial observation · locally observed tokens vs this account's quota %. Not a published allowance.".into();
    }
    "Locally observed tokens vs this account's quota %. Not a published allowance or token cap."
        .into()
}

#[cfg(test)]
pub(crate) fn clear_for_test() {
    let Ok(mut guard) = store().lock() else {
        return;
    };
    *guard = QuotaRunStore::default();
}

fn finalize_open(
    open: OpenRun,
    ended_at: DateTime<Utc>,
    end_used_percent: f64,
    reset_kind: QuotaRunResetKind,
    while_away: bool,
    after_reset_used_percent: Option<f64>,
) -> QuotaRunSnapshot {
    let peak = open.peak_used_percent.max(end_used_percent);
    let duration = (ended_at - open.started_at).num_seconds().max(0);
    let complete = !while_away && !open.interrupted && !open.mid_cycle_start;
    QuotaRunSnapshot {
        id: run_id(
            &open.provider_id,
            &open.window_id,
            open.started_at,
            ended_at,
        ),
        provider_id: open.provider_id,
        display_name: open.display_name,
        account_id: open.account_id,
        account_email: open.account_email,
        window_id: open.window_id,
        window_label: open.window_label,
        started_at: open.started_at.to_rfc3339(),
        ended_at: ended_at.to_rfc3339(),
        peak_used_percent: peak,
        end_used_percent,
        after_reset_used_percent,
        reset_kind,
        window_minutes: open.window_minutes,
        observed_duration_seconds: duration,
        complete,
        while_away,
        interrupted: open.interrupted,
        processed_tokens: open.processed_tokens,
        fresh_input_tokens: open.fresh_input_tokens,
        output_tokens: open.output_tokens,
        cache_read_tokens: open.cache_read_tokens,
        cache_write_tokens: open.cache_write_tokens,
    }
}

fn partial_from_event(
    event: &CapacityEventPayload,
    snapshot: &ProviderUsageSnapshot,
    ended_at: DateTime<Utc>,
    reset_kind: QuotaRunResetKind,
) -> QuotaRunSnapshot {
    // Prefer the previous reset boundary as a nominal start when we never saw
    // the cycle open; still mark incomplete.
    let started_at = DateTime::parse_from_rfc3339(&event.previous_reset_at)
        .ok()
        .map(|value| value.with_timezone(&Utc))
        .and_then(|previous_reset| {
            // previous_reset_at is the *end* of the completed cycle. Without a
            // duration we cannot recover the true start; use last observation
            // gap as a placeholder by walking back one day max only for display.
            // Prefer ended_at - 0 with complete=false rather than inventing a window.
            let _ = previous_reset;
            None
        })
        .unwrap_or(ended_at);
    QuotaRunSnapshot {
        id: run_id(&event.provider_id, &event.window_id, started_at, ended_at),
        provider_id: event.provider_id.clone(),
        display_name: event.display_name.clone(),
        account_id: snapshot.account_id.clone(),
        account_email: snapshot.account_email.clone(),
        window_id: event.window_id.clone(),
        window_label: event.window_label.clone(),
        started_at: started_at.to_rfc3339(),
        ended_at: ended_at.to_rfc3339(),
        peak_used_percent: event.previous_used_percent,
        end_used_percent: event.previous_used_percent,
        after_reset_used_percent: Some(event.current_used_percent),
        reset_kind,
        window_minutes: None,
        observed_duration_seconds: 0,
        complete: false,
        while_away: event.while_away,
        interrupted: true,
        processed_tokens: None,
        fresh_input_tokens: None,
        output_tokens: None,
        cache_read_tokens: None,
        cache_write_tokens: None,
    }
}

fn new_open(
    snapshot: &ProviderUsageSnapshot,
    window: &LiveWindow,
    observed_at: DateTime<Utc>,
) -> OpenRun {
    OpenRun {
        provider_id: snapshot.provider_id.clone(),
        display_name: snapshot.display_name.clone(),
        account_id: snapshot.account_id.clone(),
        account_email: snapshot.account_email.clone(),
        window_id: window.id.clone(),
        window_label: window.label.clone(),
        started_at: observed_at,
        last_observed_at: observed_at,
        peak_used_percent: window.used_percent,
        last_used_percent: window.used_percent,
        window_minutes: window.window_minutes,
        mid_cycle_start: window.used_percent > MID_CYCLE_START_USED,
        interrupted: false,
        processed_tokens: None,
        fresh_input_tokens: None,
        output_tokens: None,
        cache_read_tokens: None,
        cache_write_tokens: None,
    }
}

/// Capture local window tokens while the run is open (pre-reset).
fn attach_local_tokens(open: &mut OpenRun) {
    let Some(summary) = crate::commands::cached_provider_local_usage_summary(&open.provider_id)
    else {
        return;
    };
    for window in &summary.current_windows {
        if !local_window_matches(open, window) {
            continue;
        }
        // Prefer a non-zero sample; after reset the window restarts near 0.
        if window.tokens == 0 && open.processed_tokens.is_some() {
            return;
        }
        open.processed_tokens = Some(window.tokens);
        open.fresh_input_tokens = Some(window.token_breakdown.fresh_input_tokens);
        open.output_tokens = Some(window.token_breakdown.output_tokens);
        open.cache_read_tokens = Some(window.token_breakdown.cache_read_tokens);
        open.cache_write_tokens = Some(window.token_breakdown.cache_write_tokens);
        return;
    }
}

fn local_window_matches(open: &OpenRun, window: &crate::commands::LocalUsageWindowSummary) -> bool {
    if window.id.eq_ignore_ascii_case(&open.window_id) {
        return true;
    }
    let from_label = crate::capacity_events::semantic_window_id(&window.label, open.window_minutes);
    if from_label == open.window_id {
        return true;
    }
    let open_label =
        crate::capacity_events::semantic_window_id(&open.window_label, open.window_minutes);
    from_label == open_label
        || window
            .label
            .eq_ignore_ascii_case(open.window_label.as_str())
}

fn push_run(store: &mut QuotaRunStore, scope: &str, run: QuotaRunSnapshot) {
    let cutoff = Utc::now() - Duration::days(RETENTION_DAYS);
    let series = store.runs.entry(scope.to_string()).or_default();
    // De-dupe by id (confirmation retries / double emit).
    series.retain(|existing| existing.id != run.id);
    series.push(run);
    series.retain(|existing| {
        DateTime::parse_from_rfc3339(&existing.ended_at)
            .map(|value| value.with_timezone(&Utc) >= cutoff)
            .unwrap_or(false)
    });
    // Newest last for append; trim oldest when over cap.
    if series.len() > MAX_RUNS_PER_SCOPE {
        let drop = series.len() - MAX_RUNS_PER_SCOPE;
        series.drain(0..drop);
    }
}

#[derive(Debug, Clone)]
struct LiveWindow {
    id: String,
    label: String,
    used_percent: f64,
    window_minutes: Option<u32>,
}

fn live_windows(snapshot: &ProviderUsageSnapshot) -> Vec<LiveWindow> {
    let mut windows = Vec::new();
    push_live(
        &mut windows,
        snapshot,
        "primary",
        snapshot.primary_label.as_deref(),
        &snapshot.primary,
    );
    if let Some(window) = snapshot.secondary.as_ref() {
        push_live(
            &mut windows,
            snapshot,
            "secondary",
            snapshot.secondary_label.as_deref(),
            window,
        );
    }
    if let Some(window) = snapshot.model_specific.as_ref() {
        push_live(&mut windows, snapshot, "model", Some("Model"), window);
    }
    if let Some(window) = snapshot.tertiary.as_ref() {
        push_live(&mut windows, snapshot, "tertiary", Some("API"), window);
    }
    for extra in &snapshot.extra_rate_windows {
        push_live(
            &mut windows,
            snapshot,
            &extra.id,
            Some(&extra.title),
            &extra.window,
        );
    }
    windows
}

fn push_live(
    windows: &mut Vec<LiveWindow>,
    snapshot: &ProviderUsageSnapshot,
    fallback_id: &str,
    label: Option<&str>,
    window: &RateWindowSnapshot,
) {
    let label = label
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or(fallback_id);
    if crate::capacity_events::ignored_capacity_window(snapshot, fallback_id, label) {
        return;
    }
    // Match capacity_events semantic ids so open keys align with event.window_id.
    let id = crate::capacity_events::semantic_window_id(label, window.window_minutes);
    if id.is_empty() {
        return;
    }
    windows.push(LiveWindow {
        id,
        label: label.to_string(),
        used_percent: window.used_percent.clamp(0.0, 100.0),
        window_minutes: window.window_minutes,
    });
}

fn scope_key(snapshot: &ProviderUsageSnapshot) -> String {
    scope_key_parts(
        &snapshot.provider_id,
        snapshot.account_email.as_deref(),
        snapshot.account_id.as_deref(),
        snapshot.account_organization.as_deref(),
    )
}

fn scope_key_parts(
    provider_id: &str,
    account_email: Option<&str>,
    account_id: Option<&str>,
    organization: Option<&str>,
) -> String {
    // Prefer email, then account id, then org — same idea as capacity observation
    // scope without requiring source_label so list_runs stays callable from UI.
    let identity = account_email
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .or_else(|| account_id.map(str::trim).filter(|value| !value.is_empty()))
        .or_else(|| {
            organization
                .map(str::trim)
                .filter(|value| !value.is_empty())
        })
        .unwrap_or("anonymous")
        .to_ascii_lowercase();
    format!(
        "{}:{:016x}",
        provider_id.to_ascii_lowercase(),
        fnv1a64(identity.as_bytes())
    )
}

fn run_id(
    provider_id: &str,
    window_id: &str,
    started_at: DateTime<Utc>,
    ended_at: DateTime<Utc>,
) -> String {
    format!(
        "{:016x}",
        fnv1a64(
            format!(
                "{provider_id}|{window_id}|{}|{}",
                started_at.timestamp(),
                ended_at.timestamp()
            )
            .as_bytes()
        )
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
            .map(|parent| parent.join("quota-run-history.json"))
    })
}

fn load_store() -> QuotaRunStore {
    let Some(path) = persistence_path() else {
        return QuotaRunStore::default();
    };
    let mut store: QuotaRunStore = fs::read(&path)
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .unwrap_or_default();
    // Open runs from a previous process are incomplete if closed later.
    for open in store.open.values_mut() {
        open.interrupted = true;
    }
    store
}

fn persist_store(store: &QuotaRunStore) {
    let Some(path) = persistence_path() else {
        return;
    };
    if let Some(parent) = path.parent()
        && let Err(error) = fs::create_dir_all(parent)
    {
        tracing::warn!("failed to create quota-run-history directory: {error}");
        return;
    }
    match serde_json::to_vec(store) {
        Ok(bytes) => {
            if let Err(error) = fs::write(path, bytes) {
                tracing::warn!("failed to persist quota run history: {error}");
            }
        }
        Err(error) => tracing::warn!("failed to serialize quota run history: {error}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::capacity_events::CapacityEventPayload;
    use crate::commands::ProviderUsageSnapshot;
    use std::sync::Mutex;

    /// Global store is process-wide; serialize these tests so they do not race.
    fn test_lock() -> std::sync::MutexGuard<'static, ()> {
        static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
        LOCK.get_or_init(|| Mutex::new(()))
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    fn rate(used: f64, minutes: u32, resets_at: DateTime<Utc>) -> RateWindowSnapshot {
        RateWindowSnapshot {
            used_percent: used,
            remaining_percent: 100.0 - used,
            window_minutes: Some(minutes),
            resets_at: Some(resets_at.to_rfc3339()),
            reset_description: None,
            is_exhausted: used >= 100.0,
            reserve_percent: None,
            reserve_description: None,
            reserve_will_last_to_reset: false,
            reserve_eta_seconds: None,
        }
    }

    fn snapshot(
        provider_id: &str,
        email: &str,
        at: DateTime<Utc>,
        used: f64,
        reset: DateTime<Utc>,
    ) -> ProviderUsageSnapshot {
        ProviderUsageSnapshot {
            provider_id: provider_id.into(),
            display_name: provider_id.into(),
            primary: rate(used, 300, reset),
            primary_label: Some("Session".into()),
            secondary: None,
            secondary_label: None,
            model_specific: None,
            tertiary: None,
            extra_rate_windows: Vec::new(),
            inactive_rate_windows: Vec::new(),
            promo_signals: Vec::new(),
            reset_credits_available: None,
            cost: None,
            plan_name: None,
            account_email: Some(email.into()),
            source_label: "test".into(),
            updated_at: at.to_rfc3339(),
            error: None,
            pace: None,
            account_organization: None,
            tray_status_label: None,
            account_id: Some("acct-work".into()),
            account_label: Some("Work".into()),
            account_tint: None,
            fetch_duration_ms: None,
            wayfinder_usage: None,
        }
    }

    fn reset_event(
        provider_id: &str,
        kind: CapacityEventKind,
        previous_used: f64,
        current_used: f64,
        at: DateTime<Utc>,
        while_away: bool,
    ) -> CapacityEventPayload {
        CapacityEventPayload {
            provider_id: provider_id.into(),
            display_name: provider_id.into(),
            window_id: crate::capacity_events::semantic_window_id("Session", Some(300)),
            window_label: "Session".into(),
            window_minutes: Some(300),
            kind,
            previous_used_percent: previous_used,
            current_used_percent: current_used,
            previous_reset_credits: None,
            current_reset_credits: None,
            previous_reset_at: (at - Duration::hours(5)).to_rfc3339(),
            current_reset_at: (at + Duration::hours(5)).to_rfc3339(),
            occurred_at: at.to_rfc3339(),
            while_away,
        }
    }

    #[test]
    fn records_complete_run_when_observed_through_scheduled_reset() {
        let _guard = test_lock();
        clear_for_test();
        let provider = "codex-complete";
        let email = "complete@job.test";
        let start = Utc::now() - Duration::hours(4);
        let mid = start + Duration::hours(2);
        let end = start + Duration::hours(4);
        let reset_boundary = end;

        record_snapshot(&snapshot(provider, email, start, 5.0, reset_boundary));
        record_snapshot(&snapshot(provider, email, mid, 72.0, reset_boundary));
        record_snapshot(&snapshot(provider, email, end, 95.0, reset_boundary));

        let after = end + Duration::minutes(2);
        let snap = snapshot(provider, email, after, 8.0, after + Duration::hours(5));
        let event = reset_event(
            provider,
            CapacityEventKind::ScheduledReset,
            95.0,
            8.0,
            after,
            false,
        );
        record_capacity_events(&[event], &snap);

        let runs = list_runs(provider, Some(email));
        assert_eq!(runs.len(), 1);
        let run = &runs[0];
        assert!(run.complete, "watched from low used through reset");
        assert!(!run.while_away);
        assert!(!run.interrupted);
        assert_eq!(run.reset_kind, QuotaRunResetKind::Scheduled);
        assert!((run.peak_used_percent - 95.0).abs() < 0.01);
        assert!((run.end_used_percent - 95.0).abs() < 0.01);
        assert_eq!(run.after_reset_used_percent, Some(8.0));
        assert_eq!(run.window_minutes, Some(300));
        assert!(run.observed_duration_seconds >= 4 * 3600 - 5);
    }

    #[test]
    fn mid_cycle_start_is_marked_incomplete() {
        let _guard = test_lock();
        clear_for_test();
        let provider = "codex-midcycle";
        let email = "mid@job.test";
        let start = Utc::now() - Duration::hours(1);
        let end = start + Duration::minutes(30);
        record_snapshot(&snapshot(
            provider,
            email,
            start,
            80.0,
            end + Duration::hours(4),
        ));
        let snap = snapshot(provider, email, end, 10.0, end + Duration::hours(5));
        let event = reset_event(
            provider,
            CapacityEventKind::SurpriseReset,
            80.0,
            10.0,
            end,
            false,
        );
        record_capacity_events(&[event], &snap);

        let runs = list_runs(provider, Some(email));
        assert_eq!(runs.len(), 1);
        assert!(!runs[0].complete);
        assert_eq!(runs[0].reset_kind, QuotaRunResetKind::Surprise);
    }

    #[test]
    fn while_away_event_without_open_run_is_partial() {
        let _guard = test_lock();
        clear_for_test();
        let provider = "codex-away";
        let email = "away@job.test";
        let at = Utc::now();
        let snap = snapshot(provider, email, at, 5.0, at + Duration::hours(5));
        let event = reset_event(
            provider,
            CapacityEventKind::ScheduledReset,
            99.0,
            5.0,
            at,
            true,
        );
        record_capacity_events(&[event], &snap);
        let runs = list_runs(provider, Some(email));
        assert_eq!(runs.len(), 1);
        assert!(!runs[0].complete);
        assert!(runs[0].while_away);
        assert!(runs[0].interrupted);
    }

    #[test]
    fn does_not_cross_account_when_listing() {
        let _guard = test_lock();
        clear_for_test();
        let provider = "codex-scope";
        let at = Utc::now();
        let mut work = snapshot(provider, "me@job.test", at, 10.0, at + Duration::hours(5));
        record_snapshot(&work);
        record_snapshot(&snapshot(
            provider,
            "me@job.test",
            at + Duration::minutes(30),
            90.0,
            at + Duration::hours(5),
        ));
        let event = reset_event(
            provider,
            CapacityEventKind::ScheduledReset,
            90.0,
            5.0,
            at + Duration::hours(1),
            false,
        );
        work.primary = rate(5.0, 300, at + Duration::hours(6));
        work.updated_at = (at + Duration::hours(1)).to_rfc3339();
        record_capacity_events(&[event], &work);

        assert_eq!(list_runs(provider, Some("me@job.test")).len(), 1);
        assert!(list_runs(provider, Some("other@home.test")).is_empty());
    }

    #[test]
    fn tokens_per_percent_and_projection_require_enough_peak() {
        assert!(tokens_per_percent(Some(1_000_000), 4.0).is_none());
        let rate = tokens_per_percent(Some(9_500_000), 95.0).unwrap();
        assert!((rate - 100_000.0).abs() < 0.01);
        assert_eq!(
            cache_read_percent(Some(100), Some(80), None, None, None),
            Some(80.0)
        );
    }

    #[test]
    fn efficiency_compares_complete_runs_on_same_window() {
        let prev = QuotaRunSnapshot {
            id: "prev".into(),
            provider_id: "codex".into(),
            display_name: "Codex".into(),
            account_id: None,
            account_email: None,
            window_id: "session".into(),
            window_label: "Session".into(),
            started_at: "2026-07-01T00:00:00Z".into(),
            ended_at: "2026-07-01T05:00:00Z".into(),
            peak_used_percent: 100.0,
            end_used_percent: 100.0,
            after_reset_used_percent: Some(0.0),
            reset_kind: QuotaRunResetKind::Scheduled,
            window_minutes: Some(300),
            observed_duration_seconds: 18_000,
            complete: true,
            while_away: false,
            interrupted: false,
            processed_tokens: Some(20_000_000),
            fresh_input_tokens: Some(2_000_000),
            output_tokens: Some(1_000_000),
            cache_read_tokens: Some(16_000_000),
            cache_write_tokens: Some(1_000_000),
        };
        let next = QuotaRunSnapshot {
            id: "next".into(),
            processed_tokens: Some(10_000_000),
            peak_used_percent: 100.0,
            end_used_percent: 100.0,
            ..prev.clone()
        };
        let eff = efficiency_for_run(&next, Some(&prev));
        assert!((eff.tokens_per_percent.unwrap() - 100_000.0).abs() < 0.01);
        assert!((eff.vs_previous_tokens_per_percent.unwrap() - (-0.5)).abs() < 0.001);
        assert_eq!(eff.previous_run_id.as_deref(), Some("prev"));
        assert_eq!(eff.projected_tokens_at_100, Some(10_000_000));
        assert!(eff.note.contains("Locally observed"));
    }
}
