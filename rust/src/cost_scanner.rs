//! Local cost-usage scanner for Codex and Claude
//!
//! Scans local JSONL log files to aggregate token usage and calculate costs

use chrono::{DateTime, Duration, Local, NaiveDate, Utc};
use serde::Deserialize;
use std::collections::{HashMap, HashSet};
use std::fs::{self, File};
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

#[cfg(test)]
use crate::codex_costs::scan_codex_file_cost;
use crate::codex_costs::{
    add_codex_record_to_summary, add_codex_records_to_summary, codex_period_start,
    codex_scan_dates, scan_codex_file_cost_for_range,
};
use crate::codex_sessions::{codex_sessions_dir_candidates, default_wsl_roots};
use crate::core::{CostUsageDayRange, CostUsagePricing, JsonlScanner};
use crate::settings::Settings;

/// Cost summary from scanning local logs
#[derive(Debug, Clone, Default)]
pub struct CostSummary {
    /// Total cost in USD for the period
    pub total_cost_usd: f64,
    /// Total input tokens
    pub input_tokens: u64,
    /// Total output tokens
    pub output_tokens: u64,
    /// Total cached input tokens
    pub cached_tokens: u64,
    /// Cached input tokens read by the provider.
    pub cache_read_tokens: u64,
    /// Input tokens written into a provider cache.
    pub cache_write_tokens: u64,
    /// Number of sessions/conversations scanned
    pub sessions_count: u32,
    /// Cost breakdown by model
    pub by_model: HashMap<String, f64>,
    /// Token breakdown by model
    pub by_model_tokens: HashMap<String, ModelTokenCounts>,
    /// Codex cost split by reasoning-effort tier (e.g. medium/high/xhigh) from
    /// the rollout `turn_context`; keyed "unknown" when the log omits it.
    pub by_effort: HashMap<String, f64>,
    /// Codex token split by reasoning-effort tier, matching `by_effort`.
    pub by_effort_tokens: HashMap<String, ModelTokenCounts>,
    /// Cost split by project/repo (basename of the session `cwd`); keyed
    /// "unknown" when the log has no usable working directory.
    pub by_project: HashMap<String, f64>,
    /// Token split by project/repo, matching `by_project`.
    pub by_project_tokens: HashMap<String, ModelTokenCounts>,
    /// Model IDs that were priced with fallback rates because no canonical rate is available.
    pub unknown_models: HashSet<String>,
    /// Period start date
    pub period_start: Option<NaiveDate>,
    /// Period end date
    pub period_end: Option<NaiveDate>,
}

/// Cost and token usage assembled from one pass over a provider's local logs.
///
/// This is intentionally richer than `get_daily_cost_history`: callers can
/// render the chart and the period summary without rereading large transcript
/// trees for each number.
#[derive(Debug, Clone, Default)]
pub struct CostUsageReport {
    pub daily_costs: Vec<(String, f64)>,
    pub today: CostSummary,
    pub seven_days: CostSummary,
    pub thirty_days: CostSummary,
    pub latest_session: Option<CostSummary>,
    /// Exact token totals for caller-supplied reset windows, keyed by ID.
    pub current_windows: HashMap<String, CostSummary>,
}

/// A provider reset window whose local-log usage should be aggregated exactly.
#[derive(Debug, Clone)]
pub struct CurrentUsageWindow {
    pub id: String,
    pub starts_at: DateTime<Utc>,
    pub ends_at: DateTime<Utc>,
}

/// Per-model token counts
#[derive(Debug, Clone, Default)]
pub struct ModelTokenCounts {
    pub input_tokens: u64,
    pub output_tokens: u64,
    /// Cache read + write combined (legacy aggregate).
    pub cached_tokens: u64,
    /// Cache-read tokens alone (for cache-hit rate).
    pub cache_read_tokens: u64,
    /// Cache-write / creation tokens alone.
    pub cache_write_tokens: u64,
    /// Number of usage records attributed to this model (for per-call averages).
    pub calls: u64,
}

/// True when a provider reports cached input *inside* its input count.
///
/// Codex does; Claude reports cache reads as their own bucket. Any total that
/// adds the cache bucket to a Codex input count therefore counts those tokens
/// twice.
pub fn provider_folds_cache_into_input(provider_id: &str) -> bool {
    provider_id.eq_ignore_ascii_case("codex")
}

/// Token buckets with each token counted exactly once, whatever the provider's
/// reporting convention.
///
/// This exists because the same normalization was previously written twice, and
/// only one copy handled Codex. The other reported a 97%-cached model as 49%,
/// since `97 / (100 + 97)` double-counts the cached tokens sitting in both the
/// input and cache buckets. Build every ratio from this type.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct NormalizedTokens {
    pub fresh_input_tokens: u64,
    pub output_tokens: u64,
    pub cache_read_tokens: u64,
    pub cache_write_tokens: u64,
}

impl NormalizedTokens {
    /// Fresh input + output + cache read + cache write, each counted once.
    pub fn processed(&self) -> u64 {
        self.fresh_input_tokens
            .saturating_add(self.output_tokens)
            .saturating_add(self.cache_read_tokens)
            .saturating_add(self.cache_write_tokens)
    }

    /// Share of processed tokens served from cache. `None` with no activity.
    pub fn cache_read_percent(&self) -> Option<f64> {
        let processed = self.processed();
        (processed > 0).then(|| (self.cache_read_tokens as f64 / processed as f64) * 100.0)
    }
}

fn normalize_tokens(
    provider_id: &str,
    input_tokens: u64,
    output_tokens: u64,
    cached_tokens: u64,
    cache_read_tokens: u64,
    cache_write_tokens: u64,
) -> NormalizedTokens {
    let folds_cache = provider_folds_cache_into_input(provider_id);
    // Some sources only populate the legacy aggregate, so take whichever cache
    // figure is larger rather than trusting one field to be present.
    let cache_read_tokens = cache_read_tokens.max(if folds_cache { cached_tokens } else { 0 });
    let fresh_input_tokens = if folds_cache {
        input_tokens.saturating_sub(cache_read_tokens)
    } else {
        input_tokens
    };
    NormalizedTokens {
        fresh_input_tokens,
        output_tokens,
        cache_read_tokens,
        cache_write_tokens,
    }
}

impl CostSummary {
    /// Provider-normalized buckets for this period.
    pub fn normalized_tokens(&self, provider_id: &str) -> NormalizedTokens {
        normalize_tokens(
            provider_id,
            self.input_tokens,
            self.output_tokens,
            self.cached_tokens,
            self.cache_read_tokens,
            self.cache_write_tokens,
        )
    }
}

impl ModelTokenCounts {
    pub fn total(&self) -> u64 {
        self.input_tokens + self.output_tokens
    }

    /// Provider-normalized buckets for this model/effort/project row.
    pub fn normalized(&self, provider_id: &str) -> NormalizedTokens {
        normalize_tokens(
            provider_id,
            self.input_tokens,
            self.output_tokens,
            self.cached_tokens,
            self.cache_read_tokens,
            self.cache_write_tokens,
        )
    }

    pub fn merge_from(&mut self, other: &Self) {
        self.input_tokens += other.input_tokens;
        self.output_tokens += other.output_tokens;
        self.cached_tokens += other.cached_tokens;
        self.cache_read_tokens += other.cache_read_tokens;
        self.cache_write_tokens += other.cache_write_tokens;
        self.calls += other.calls;
    }
}

#[cfg(test)]
mod model_token_counts_tests {
    use super::ModelTokenCounts;

    /// Claude reports cache reads outside its input count, so nothing is
    /// subtracted and every bucket is summed as reported.
    #[test]
    fn normalized_sums_every_bucket_for_a_provider_that_separates_cache() {
        let counts = ModelTokenCounts {
            input_tokens: 10,
            output_tokens: 20,
            cached_tokens: 999, // legacy aggregate; must not be added in
            cache_read_tokens: 3,
            cache_write_tokens: 4,
            calls: 2,
        };
        let normalized = counts.normalized("claude");

        assert_eq!(normalized.fresh_input_tokens, 10);
        assert_eq!(normalized.processed(), 37);
        assert_eq!(counts.total(), 30);
    }

    /// Codex folds cached input into `input_tokens`, so the cache bucket must
    /// come back out of the input before anything is summed. Counting it in
    /// both places is what rendered a 97%-cached model as 49%.
    #[test]
    fn normalized_removes_cached_input_for_codex_so_cache_rate_is_honest() {
        let counts = ModelTokenCounts {
            input_tokens: 100,
            output_tokens: 0,
            cached_tokens: 97,
            cache_read_tokens: 97,
            cache_write_tokens: 0,
            calls: 1,
        };
        let normalized = counts.normalized("codex");

        assert_eq!(normalized.fresh_input_tokens, 3, "100 input less 97 cached");
        assert_eq!(normalized.processed(), 100, "not 197");
        let percent = normalized.cache_read_percent().expect("cache percent");
        assert!((percent - 97.0).abs() < 0.001, "got {percent}");
    }

    #[test]
    fn cache_read_percent_is_none_without_activity() {
        assert!(
            ModelTokenCounts::default()
                .normalized("codex")
                .cache_read_percent()
                .is_none()
        );
    }

    #[test]
    fn merge_from_accumulates_all_buckets_and_calls() {
        let mut left = ModelTokenCounts {
            input_tokens: 1,
            output_tokens: 2,
            cached_tokens: 3,
            cache_read_tokens: 4,
            cache_write_tokens: 5,
            calls: 6,
        };
        left.merge_from(&ModelTokenCounts {
            input_tokens: 10,
            output_tokens: 20,
            cached_tokens: 30,
            cache_read_tokens: 40,
            cache_write_tokens: 50,
            calls: 60,
        });
        assert_eq!(left.input_tokens, 11);
        assert_eq!(left.output_tokens, 22);
        assert_eq!(left.cached_tokens, 33);
        assert_eq!(left.cache_read_tokens, 44);
        assert_eq!(left.cache_write_tokens, 55);
        assert_eq!(left.calls, 66);
    }
}

impl CostSummary {
    pub fn format_total(&self) -> String {
        format!("${:.2}", self.total_cost_usd)
    }
}

fn is_cancelled(cancel: Option<&AtomicBool>) -> bool {
    cancel.is_some_and(|flag| flag.load(Ordering::Relaxed))
}

/// Record a Codex rollout by file name and report whether it is new. The same
/// rollout can live in `sessions/`, `archived_sessions/`, and across homes/WSL
/// roots; deduping by name (not full path) counts it once. An unnamed path is
/// always parsed rather than silently dropped.
fn mark_unseen_rollout(path: &Path, seen: &mut HashSet<String>) -> bool {
    match path.file_name().and_then(|name| name.to_str()) {
        Some(name) => seen.insert(name.to_string()),
        None => true,
    }
}

/// Friendly project name from a session working directory: the last path
/// segment, e.g. `C:\a\b\my-repo` or `\\wsl.localhost\d\home\me\my-repo` ->
/// `my-repo`. Returns `None` for a blank/rootless path so callers can bucket it
/// as "unknown" rather than inventing a project.
pub(crate) fn project_from_cwd(cwd: &str) -> Option<String> {
    let trimmed = cwd.trim().trim_end_matches(['/', '\\']);
    let segment = trimmed.rsplit(['/', '\\']).find(|s| !s.is_empty())?;
    // A bare filesystem root carries no project name: a drive letter ("C:") or
    // a path that was only separators. Treat those as unknown.
    if is_drive_root(segment) {
        return None;
    }
    Some(segment.to_string())
}

/// True for a bare Windows drive root segment like `C:` or `d:`.
fn is_drive_root(segment: &str) -> bool {
    let bytes = segment.as_bytes();
    bytes.len() == 2 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':'
}

/// Cheap date gate for the flat archived dir: files are `rollout-YYYY-MM-DD…`.
/// An unrecognized name falls through to the parser's own timestamp filter.
fn archived_rollout_day_in_range(name: &str, range: &CostUsageDayRange) -> bool {
    match name
        .strip_prefix("rollout-")
        .and_then(|rest| rest.get(0..10))
    {
        // Only gate on a real calendar date. A date-shaped but invalid name
        // (e.g. rollout-2026-99-99) must fall through so the parser's own
        // timestamp filter decides, rather than being skipped lexicographically.
        Some(day) if CostUsageDayRange::parse_day_key(day).is_some() => {
            CostUsageDayRange::is_in_range(day, &range.scan_since_key, &range.scan_until_key)
        }
        _ => true,
    }
}

/// Fallback Claude model used when a scanned model isn't in the canonical
/// pricing table (unknown or retired IDs). Prices as Sonnet 4.6.
const FALLBACK_CLAUDE_MODEL: &str = "claude-sonnet-4-6";

/// Claude cost calculation for the usage scanner.
///
/// Per-token rates come from the canonical `CostUsagePricing::claude_cost_usd`
/// table (the single source of truth for Claude pricing). The only
/// scanner-specific piece is the one-hour cache-write premium, which the
/// canonical cost function doesn't model: one-hour cache writes bill at 2x the
/// input rate.
struct ClaudePricing;

impl ClaudePricing {
    #[cfg(test)]
    fn cost_usd_with_cache_ttl(
        model: &str,
        input: u64,
        cache_create: u64,
        cache_create_1h: u64,
        cache_read: u64,
        output: u64,
    ) -> f64 {
        Self::cost_usd_with_cache_ttl_on_date(
            model,
            input,
            cache_create,
            cache_create_1h,
            cache_read,
            output,
            Utc::now().date_naive(),
        )
    }

    fn cost_usd_with_cache_ttl_on_date(
        model: &str,
        input: u64,
        cache_create: u64,
        cache_create_1h: u64,
        cache_read: u64,
        output: u64,
        usage_date: NaiveDate,
    ) -> f64 {
        let cache_create_1h = cache_create_1h.min(cache_create);
        let cache_create_5m = cache_create.saturating_sub(cache_create_1h);

        // Standard buckets (input, cache-read, 5-minute cache-write, output),
        // including any long-context tiering, come from the canonical table.
        // Unknown/retired models fall back to Sonnet pricing.
        let clamp = |v: u64| v.min(i32::MAX as u64) as i32;
        let base = CostUsagePricing::claude_cost_usd_on_date(
            model,
            clamp(input),
            clamp(cache_read),
            clamp(cache_create_5m),
            clamp(output),
            usage_date,
        )
        .or_else(|| {
            CostUsagePricing::claude_cost_usd_on_date(
                FALLBACK_CLAUDE_MODEL,
                clamp(input),
                clamp(cache_read),
                clamp(cache_create_5m),
                clamp(output),
                usage_date,
            )
        })
        .unwrap_or(0.0);

        // Scanner-specific: one-hour cache writes bill at 2x the input rate.
        let input_rate = CostUsagePricing::claude_input_cost_per_token_on_date(model, usage_date)
            .or_else(|| {
                CostUsagePricing::claude_input_cost_per_token_on_date(
                    FALLBACK_CLAUDE_MODEL,
                    usage_date,
                )
            })
            .unwrap_or(0.0);

        base + (cache_create_1h as f64) * input_rate * 2.0
    }
}

/// JSONL event structures for Codex
#[allow(dead_code)]
#[derive(Debug, Deserialize)]
struct CodexEvent {
    #[serde(rename = "type")]
    event_type: Option<String>,
    event_msg: Option<CodexEventMsg>,
}

#[allow(dead_code)]
#[derive(Debug, Deserialize)]
struct CodexEventMsg {
    #[serde(rename = "type")]
    msg_type: Option<String>,
    input_tokens: Option<u64>,
    cached_input_tokens: Option<u64>,
    output_tokens: Option<u64>,
}

/// JSONL event structures for Claude transcripts. Unknown fields are
/// ignored, so lines that are not assistant usage events still parse.
#[derive(Debug, Deserialize)]
struct ClaudeEvent {
    #[serde(rename = "type")]
    event_type: Option<String>,
    timestamp: Option<String>,
    #[serde(rename = "requestId", alias = "request_id")]
    request_id: Option<String>,
    /// Session working directory, used for per-project cost.
    cwd: Option<String>,
    message: Option<ClaudeMessage>,
}

impl ClaudeEvent {
    fn parsed_timestamp(&self) -> Option<DateTime<Utc>> {
        let timestamp = self.timestamp.as_deref()?;
        DateTime::parse_from_rfc3339(timestamp)
            .ok()
            .map(|ts| ts.with_timezone(&Utc))
    }
}

#[derive(Debug, Deserialize)]
struct ClaudeMessage {
    id: Option<String>,
    model: Option<String>,
    usage: Option<ClaudeUsage>,
}

#[derive(Debug, Deserialize)]
struct ClaudeUsage {
    input_tokens: Option<u64>,
    output_tokens: Option<u64>,
    cache_creation_input_tokens: Option<u64>,
    cache_read_input_tokens: Option<u64>,
    cache_creation: Option<ClaudeCacheCreation>,
}

impl ClaudeUsage {
    /// One-hour cache-write tokens, clamped to the total cache-write count.
    fn one_hour_cache_creation_tokens(&self, total: u64) -> u64 {
        self.cache_creation
            .as_ref()
            .and_then(|cache_creation| cache_creation.ephemeral_1h_input_tokens)
            .unwrap_or(0)
            .min(total)
    }
}

/// TTL breakdown of cache writes reported by the API.
#[derive(Debug, Deserialize)]
struct ClaudeCacheCreation {
    ephemeral_1h_input_tokens: Option<u64>,
}

#[derive(Debug, Clone)]
struct ClaudeUsageRecord {
    model: String,
    timestamp: Option<DateTime<Utc>>,
    dedup_key: Option<String>,
    /// Project/repo (basename of the line's cwd), for per-project cost.
    project: Option<String>,
    input: u64,
    output: u64,
    cache_create: u64,
    cache_read: u64,
    cost: f64,
}

/// Cost usage scanner
pub struct CostScanner {
    days: u32,
}

impl CostScanner {
    /// Create a new scanner for the last N days
    pub fn new(days: u32) -> Self {
        Self { days }
    }

    /// Scan Codex local logs
    pub fn scan_codex(&self) -> CostSummary {
        self.scan_codex_with_cancel(None)
    }

    /// Scan Codex local logs, stopping early when the caller cancels the scan.
    pub fn scan_codex_with_cancel(&self, cancel: Option<&AtomicBool>) -> CostSummary {
        let mut summary = CostSummary::default();
        let today = Local::now().date_naive();
        let start_date = codex_period_start(today, self.days);
        let range = CostUsageDayRange::new(start_date, today);

        summary.period_start = Some(start_date);
        summary.period_end = Some(today);

        // One rollout can appear in the date-nested `sessions/` tree, the flat
        // `archived_sessions/` dir, and across multiple homes/WSL roots. Dedup
        // by rollout file name so it is counted exactly once.
        let mut seen = HashSet::new();
        for sessions_dir in self.get_codex_sessions_dirs() {
            if is_cancelled(cancel) {
                break;
            }
            if sessions_dir.exists() {
                self.scan_codex_sessions_dir(
                    &sessions_dir,
                    &range,
                    &mut summary,
                    &mut seen,
                    cancel,
                );
            }
        }
        for archived_dir in self.get_codex_archived_dirs() {
            if is_cancelled(cancel) {
                break;
            }
            if archived_dir.exists() {
                self.scan_codex_archived_dir(
                    &archived_dir,
                    &range,
                    &mut summary,
                    &mut seen,
                    cancel,
                );
            }
        }

        summary
    }

    /// Scan Claude local logs
    pub fn scan_claude(&self) -> CostSummary {
        self.scan_claude_with_cancel(None)
    }

    /// Scan Claude local logs, stopping early when the caller cancels the scan.
    pub fn scan_claude_with_cancel(&self, cancel: Option<&AtomicBool>) -> CostSummary {
        let projects_dir = self.get_claude_projects_dir();
        if !projects_dir.exists() {
            return CostSummary::default();
        }

        let mut summary = CostSummary::default();
        let today = Utc::now().date_naive();
        let start_date = today - Duration::days(self.days as i64);
        let cutoff = Utc::now() - Duration::days(self.days as i64);

        summary.period_start = Some(start_date);
        summary.period_end = Some(today);

        // Parse independent transcript files in parallel, then apply the
        // cross-file de-duplication in deterministic file order.
        let files = self.claude_files_since(&projects_dir, &cutoff, cancel);
        let parsed_files = parse_claude_files(&files, &cutoff, cancel);
        let mut seen = HashSet::new();
        for (_, records) in parsed_files {
            let mut counted = 0;
            for record in records {
                if !should_count_claude_record(&record, &cutoff, &mut seen) {
                    continue;
                }
                counted += 1;
                add_claude_record_to_summary(&mut summary, &record);
            }
            if counted > 0 {
                summary.sessions_count += 1;
            }
        }

        summary
    }

    fn get_codex_sessions_dirs(&self) -> Vec<PathBuf> {
        let settings = Settings::load();
        let codex_home = std::env::var("CODEX_HOME").ok();
        codex_sessions_dir_candidates(
            dirs::home_dir(),
            codex_home,
            &settings.codex_custom_sessions_dirs,
            &default_wsl_roots(),
        )
    }

    /// The `archived_sessions` dir that sits beside each `sessions` candidate.
    /// Codex moves older rollouts here, so skipping it under-counts history.
    fn get_codex_archived_dirs(&self) -> Vec<PathBuf> {
        self.get_codex_sessions_dirs()
            .iter()
            .filter_map(|dir| dir.parent().map(|parent| parent.join("archived_sessions")))
            .collect()
    }

    fn scan_codex_sessions_dir(
        &self,
        sessions_dir: &Path,
        range: &CostUsageDayRange,
        summary: &mut CostSummary,
        seen: &mut HashSet<String>,
        cancel: Option<&AtomicBool>,
    ) {
        // Iterate through the date-based directory structure with one day of
        // padding on each side. Codex JSONL timestamps are UTC, while the tray
        // presents local calendar days; the parser filters back to `range`.
        for date in codex_scan_dates(range) {
            if is_cancelled(cancel) {
                break;
            }
            let year = date.format("%Y").to_string();
            let month = date.format("%m").to_string();
            let day = date.format("%d").to_string();

            let day_dir = sessions_dir.join(&year).join(&month).join(&day);
            if !day_dir.exists() {
                continue;
            }

            if let Ok(entries) = fs::read_dir(&day_dir) {
                for entry in entries.flatten() {
                    if is_cancelled(cancel) {
                        break;
                    }
                    let path = entry.path();
                    if path.extension().is_some_and(|e| e == "jsonl")
                        && mark_unseen_rollout(&path, seen)
                    {
                        self.parse_codex_file(&path, summary, cancel);
                    }
                }
            }
        }
    }

    /// Scan the flat `archived_sessions` dir. Files are `rollout-<date>-<id>.jsonl`;
    /// the date prefix keeps us from parsing rollouts far outside the window.
    fn scan_codex_archived_dir(
        &self,
        archived_dir: &Path,
        range: &CostUsageDayRange,
        summary: &mut CostSummary,
        seen: &mut HashSet<String>,
        cancel: Option<&AtomicBool>,
    ) {
        let Ok(entries) = fs::read_dir(archived_dir) else {
            return;
        };
        for entry in entries.flatten() {
            if is_cancelled(cancel) {
                break;
            }
            let path = entry.path();
            if path.extension().is_none_or(|e| e != "jsonl") {
                continue;
            }
            let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
                continue;
            };
            if !archived_rollout_day_in_range(name, range) {
                continue;
            }
            if !mark_unseen_rollout(&path, seen) {
                continue;
            }
            self.parse_codex_file(&path, summary, cancel);
        }
    }

    fn get_claude_projects_dir(&self) -> PathBuf {
        if let Ok(claude_config) = std::env::var("CLAUDE_CONFIG_DIR") {
            let trimmed = claude_config.trim();
            if !trimmed.is_empty() {
                return PathBuf::from(trimmed).join("projects");
            }
        }

        // Try ~/.claude/projects first
        let home = dirs::home_dir().unwrap_or_else(|| PathBuf::from("."));
        let claude_dir = home.join(".claude").join("projects");
        if claude_dir.exists() {
            return claude_dir;
        }

        // Fallback to ~/.config/claude/projects
        home.join(".config").join("claude").join("projects")
    }

    fn parse_codex_file(
        &self,
        path: &Path,
        summary: &mut CostSummary,
        cancel: Option<&AtomicBool>,
    ) {
        if is_cancelled(cancel) {
            return;
        }
        let today = Local::now().date_naive();
        let start_date = codex_period_start(today, self.days);
        let range = CostUsageDayRange::new(start_date, today);
        let parse_result = match JsonlScanner::parse_codex_file(path, &range, 0, None, None) {
            Ok(result) => result,
            Err(_) => return,
        };

        let (session_cost, has_tokens) =
            add_codex_records_to_summary(summary, &parse_result.records, &range);

        if has_tokens {
            summary.total_cost_usd += session_cost;
            summary.sessions_count += 1;
        }
    }

    fn walk_claude_files<F>(
        &self,
        dir: &Path,
        cutoff: &DateTime<Utc>,
        cancel: Option<&AtomicBool>,
        on_file: &mut F,
    ) where
        F: FnMut(&Path),
    {
        if is_cancelled(cancel) {
            return;
        }
        let entries = match fs::read_dir(dir) {
            Ok(e) => e,
            Err(_) => return,
        };

        for entry in entries.flatten() {
            if is_cancelled(cancel) {
                break;
            }
            let path = entry.path();
            if path.is_dir() {
                self.walk_claude_files(&path, cutoff, cancel, on_file);
            } else if path.extension().is_some_and(|e| e == "jsonl") {
                // Check file modification time
                if let Ok(metadata) = fs::metadata(&path)
                    && let Ok(modified) = metadata.modified()
                {
                    let modified_dt: DateTime<Utc> = modified.into();
                    if modified_dt >= *cutoff {
                        on_file(&path);
                    }
                }
            }
        }
    }

    fn claude_files_since(
        &self,
        projects_dir: &Path,
        cutoff: &DateTime<Utc>,
        cancel: Option<&AtomicBool>,
    ) -> Vec<PathBuf> {
        let mut files = Vec::new();
        self.walk_claude_files(projects_dir, cutoff, cancel, &mut |path| {
            files.push(path.to_path_buf())
        });
        files.sort();
        files
    }
}

fn parse_claude_files(
    files: &[PathBuf],
    cutoff: &DateTime<Utc>,
    cancel: Option<&AtomicBool>,
) -> Vec<(PathBuf, Vec<ClaudeUsageRecord>)> {
    if files.is_empty() {
        return Vec::new();
    }
    let workers = std::thread::available_parallelism()
        .map(usize::from)
        .unwrap_or(1)
        .clamp(1, 8)
        .min(files.len());
    let chunk_size = files.len().div_ceil(workers);
    std::thread::scope(|scope| {
        let handles = files
            .chunks(chunk_size)
            .map(|chunk| {
                scope.spawn(move || {
                    chunk
                        .iter()
                        .map(|path| {
                            let mut records = Vec::new();
                            let mut local_seen = HashSet::new();
                            for_each_claude_usage_record(
                                path,
                                cutoff,
                                &mut local_seen,
                                cancel,
                                |record| records.push(record.clone()),
                            );
                            (path.clone(), records)
                        })
                        .collect::<Vec<_>>()
                })
            })
            .collect::<Vec<_>>();
        handles
            .into_iter()
            .flat_map(|handle| handle.join().unwrap_or_default())
            .collect::<Vec<_>>()
    })
}

/// Stream the de-duplicated, in-window usage records from one transcript
/// file into `on_record`. Both the summary scan and the daily-history scan
/// consume this single reader, so Claude log semantics live in one place.
/// Returns the number of records consumed, so callers can tell whether the
/// file contributed anything.
fn for_each_claude_usage_record<F>(
    path: &Path,
    cutoff: &DateTime<Utc>,
    seen: &mut HashSet<String>,
    cancel: Option<&AtomicBool>,
    mut on_record: F,
) -> usize
where
    F: FnMut(&ClaudeUsageRecord),
{
    let Ok(file) = File::open(path) else {
        return 0;
    };

    let mut counted = 0;
    let mut reader = BufReader::new(file);
    let mut line = Vec::with_capacity(16 * 1024);
    loop {
        line.clear();
        let Ok(bytes_read) = reader.read_until(b'\n', &mut line) else {
            break;
        };
        if bytes_read == 0 {
            break;
        }
        if is_cancelled(cancel) {
            break;
        }
        // Claude transcripts contain large user/tool payloads that can never
        // contribute token usage. Avoid allocating a String and asking serde
        // to parse those lines; only assistant events with a usage object can
        // produce a record.
        if !contains_bytes(&line, b"\"assistant\"") || !contains_bytes(&line, b"\"usage\"") {
            continue;
        }
        if let Ok(event) = serde_json::from_slice::<ClaudeEvent>(&line)
            && let Some(record) = claude_usage_record_from_event(&event)
            && should_count_claude_record(&record, cutoff, seen)
        {
            counted += 1;
            on_record(&record);
        }
    }
    counted
}

fn contains_bytes(haystack: &[u8], needle: &[u8]) -> bool {
    if needle.is_empty() || haystack.len() < needle.len() {
        return false;
    }
    let mut offset = 0;
    while offset + needle.len() <= haystack.len() {
        let Some(relative) = haystack[offset..]
            .iter()
            .position(|byte| *byte == needle[0])
        else {
            return false;
        };
        let start = offset + relative;
        if haystack
            .get(start..start + needle.len())
            .is_some_and(|candidate| candidate == needle)
        {
            return true;
        }
        offset = start + 1;
    }
    false
}

fn claude_usage_record_from_event(event: &ClaudeEvent) -> Option<ClaudeUsageRecord> {
    if event.event_type.as_deref() != Some("assistant") {
        return None;
    }

    let message = event.message.as_ref()?;
    let usage = message.usage.as_ref()?;
    let model = message.model.as_deref().unwrap_or("claude-3-5-sonnet");

    let input = usage.input_tokens.unwrap_or(0);
    let output = usage.output_tokens.unwrap_or(0);
    let cache_create = usage.cache_creation_input_tokens.unwrap_or(0);
    let cache_read = usage.cache_read_input_tokens.unwrap_or(0);

    if input == 0 && output == 0 && cache_create == 0 && cache_read == 0 {
        return None;
    }

    let cache_create_1h = usage.one_hour_cache_creation_tokens(cache_create);
    let timestamp = event.parsed_timestamp();
    let usage_date = timestamp
        .map(|recorded_at| recorded_at.date_naive())
        .unwrap_or_else(|| Utc::now().date_naive());
    let cost = ClaudePricing::cost_usd_with_cache_ttl_on_date(
        model,
        input,
        cache_create,
        cache_create_1h,
        cache_read,
        output,
        usage_date,
    );

    Some(ClaudeUsageRecord {
        model: model.to_string(),
        timestamp,
        dedup_key: claude_usage_dedup_key(message.id.as_deref(), event.request_id.as_deref()),
        project: event.cwd.as_deref().and_then(project_from_cwd),
        input,
        output,
        cache_create,
        cache_read,
        cost,
    })
}

fn claude_usage_dedup_key(message_id: Option<&str>, request_id: Option<&str>) -> Option<String> {
    match (message_id, request_id) {
        (Some(message_id), Some(request_id)) => Some(format!("{message_id}:{request_id}")),
        (Some(message_id), None) => Some(format!("message:{message_id}")),
        (None, Some(request_id)) => Some(format!("request:{request_id}")),
        (None, None) => None,
    }
}

fn should_count_claude_record(
    record: &ClaudeUsageRecord,
    cutoff: &DateTime<Utc>,
    seen: &mut HashSet<String>,
) -> bool {
    if let Some(timestamp) = record.timestamp
        && timestamp < *cutoff
    {
        return false;
    }

    if let Some(key) = &record.dedup_key
        && !seen.insert(key.clone())
    {
        return false;
    }

    true
}

fn add_claude_record_to_summary(summary: &mut CostSummary, record: &ClaudeUsageRecord) {
    if CostUsagePricing::claude_cost_usd(&record.model, 0, 0, 0, 0).is_none() {
        summary.unknown_models.insert(record.model.clone());
    }

    summary.input_tokens += record.input;
    summary.output_tokens += record.output;
    summary.cached_tokens += record.cache_create + record.cache_read;
    summary.cache_read_tokens += record.cache_read;
    summary.cache_write_tokens += record.cache_create;
    summary.total_cost_usd += record.cost;

    *summary.by_model.entry(record.model.clone()).or_insert(0.0) += record.cost;

    let model_tokens = summary
        .by_model_tokens
        .entry(record.model.clone())
        .or_default();
    model_tokens.input_tokens += record.input;
    model_tokens.output_tokens += record.output;
    model_tokens.cached_tokens += record.cache_create + record.cache_read;
    model_tokens.cache_read_tokens += record.cache_read;
    model_tokens.cache_write_tokens += record.cache_create;
    model_tokens.calls += 1;

    let project_bucket = crate::codex_costs::project_bucket(record.project.as_deref());
    *summary
        .by_project
        .entry(project_bucket.clone())
        .or_insert(0.0) += record.cost;
    let project_tokens = summary.by_project_tokens.entry(project_bucket).or_default();
    project_tokens.input_tokens += record.input;
    project_tokens.output_tokens += record.output;
    project_tokens.cached_tokens += record.cache_create + record.cache_read;
    project_tokens.cache_read_tokens += record.cache_read;
    project_tokens.cache_write_tokens += record.cache_create;
    project_tokens.calls += 1;
}

/// Add one usage record to the per-day cost buckets, keyed by the record's
/// own timestamp in the local timezone. Records outside the initialized
/// date range (or without a timestamp) are ignored.
fn add_claude_record_to_daily_costs(
    daily_costs: &mut HashMap<String, f64>,
    record: &ClaudeUsageRecord,
) {
    let Some(timestamp) = record.timestamp else {
        return;
    };
    let date_str = timestamp
        .with_timezone(&Local)
        .date_naive()
        .format("%Y-%m-%d")
        .to_string();
    if let Some(cost) = daily_costs.get_mut(&date_str) {
        *cost += record.cost;
    }
}

/// Check if any cost usage sources are available
#[allow(dead_code)]
pub fn has_cost_usage_sources() -> bool {
    let scanner = CostScanner::new(1);
    scanner
        .get_codex_sessions_dirs()
        .iter()
        .any(|dir| dir.exists())
        || scanner.get_claude_projects_dir().exists()
}

/// Build chart history and period summaries with one transcript pass.
///
/// Codex and Claude logs can grow into gigabytes. The older chart path read
/// the same files once for the bars, again for the 30-day summary, and again
/// for today's values. This report keeps those views consistent and makes the
/// initial load bounded by a single scan.
pub fn get_cost_usage_report(provider: &str, days: u32) -> Option<CostUsageReport> {
    get_cost_usage_report_with_windows(provider, days, &[])
}

pub fn get_cost_usage_report_with_windows(
    provider: &str,
    days: u32,
    current_windows: &[CurrentUsageWindow],
) -> Option<CostUsageReport> {
    let days = days.max(1);
    let scanner = CostScanner::new(days);
    match provider {
        "codex" => Some(scan_codex_report(&scanner, days, current_windows)),
        "claude" => Some(scan_claude_report(&scanner, days, current_windows)),
        _ => None,
    }
}

fn empty_current_window_summaries(windows: &[CurrentUsageWindow]) -> HashMap<String, CostSummary> {
    windows
        .iter()
        .map(|window| (window.id.clone(), CostSummary::default()))
        .collect()
}

fn add_to_current_windows<F>(
    summaries: &mut HashMap<String, CostSummary>,
    windows: &[CurrentUsageWindow],
    timestamp: Option<DateTime<Utc>>,
    mut add: F,
) where
    F: FnMut(&mut CostSummary),
{
    let Some(timestamp) = timestamp else {
        return;
    };
    for window in windows {
        if timestamp >= window.starts_at
            && timestamp < window.ends_at
            && let Some(summary) = summaries.get_mut(&window.id)
        {
            add(summary);
        }
    }
}

fn empty_daily_summaries(days: u32) -> HashMap<String, CostSummary> {
    let today = Local::now().date_naive();
    (0..days)
        .map(|days_ago| {
            let date = today - Duration::days(days_ago as i64);
            (date.format("%Y-%m-%d").to_string(), CostSummary::default())
        })
        .collect()
}

fn merge_summary(target: &mut CostSummary, source: &CostSummary) {
    target.total_cost_usd += source.total_cost_usd;
    target.input_tokens += source.input_tokens;
    target.output_tokens += source.output_tokens;
    target.cached_tokens += source.cached_tokens;
    target.cache_read_tokens += source.cache_read_tokens;
    target.cache_write_tokens += source.cache_write_tokens;
    target.sessions_count += source.sessions_count;
    for (model, cost) in &source.by_model {
        *target.by_model.entry(model.clone()).or_insert(0.0) += cost;
    }
    for (model, tokens) in &source.by_model_tokens {
        target
            .by_model_tokens
            .entry(model.clone())
            .or_default()
            .merge_from(tokens);
    }
    for (effort, cost) in &source.by_effort {
        *target.by_effort.entry(effort.clone()).or_insert(0.0) += cost;
    }
    for (effort, tokens) in &source.by_effort_tokens {
        target
            .by_effort_tokens
            .entry(effort.clone())
            .or_default()
            .merge_from(tokens);
    }
    for (project, cost) in &source.by_project {
        *target.by_project.entry(project.clone()).or_insert(0.0) += cost;
    }
    for (project, tokens) in &source.by_project_tokens {
        target
            .by_project_tokens
            .entry(project.clone())
            .or_default()
            .merge_from(tokens);
    }
    target
        .unknown_models
        .extend(source.unknown_models.iter().cloned());
}

fn finish_report(
    mut daily: HashMap<String, CostSummary>,
    days: u32,
    latest_session: Option<CostSummary>,
    sessions: (u32, u32, u32),
    undated: Option<&CostSummary>,
    current_windows: HashMap<String, CostSummary>,
) -> CostUsageReport {
    let today = Local::now().date_naive();
    let seven_day_start = today - Duration::days(6);
    let period_start = codex_period_start(today, days);
    let mut today_summary = CostSummary::default();
    let mut seven_day_summary = CostSummary::default();
    let mut period_summary = CostSummary::default();

    for (day, summary) in &daily {
        let Some(date) = NaiveDate::parse_from_str(day, "%Y-%m-%d").ok() else {
            continue;
        };
        merge_summary(&mut period_summary, summary);
        if date >= seven_day_start {
            merge_summary(&mut seven_day_summary, summary);
        }
        if date == today {
            merge_summary(&mut today_summary, summary);
        }
    }
    if let Some(undated) = undated {
        merge_summary(&mut period_summary, undated);
    }

    today_summary.sessions_count = sessions.0;
    seven_day_summary.sessions_count = sessions.1;
    period_summary.sessions_count = sessions.2;
    for summary in [
        &mut today_summary,
        &mut seven_day_summary,
        &mut period_summary,
    ] {
        summary.period_end = Some(today);
    }
    today_summary.period_start = Some(today);
    seven_day_summary.period_start = Some(seven_day_start);
    period_summary.period_start = Some(period_start);

    let mut daily_costs: Vec<_> = daily
        .drain()
        .map(|(day, summary)| (day, summary.total_cost_usd))
        .collect();
    daily_costs.sort_by(|left, right| left.0.cmp(&right.0));

    CostUsageReport {
        daily_costs,
        today: today_summary,
        seven_days: seven_day_summary,
        thirty_days: period_summary,
        latest_session,
        current_windows,
    }
}

/// Accumulates Codex rollouts into the report's daily, period, and
/// reset-window buckets, one file at a time.
///
/// Extracted so the date-nested `sessions/` tree and the flat
/// `archived_sessions/` dir go through identical accounting. They previously
/// did not: only `sessions/` was scanned here, so archiving a task quietly
/// shrank every total on the charts, while the summary scanner still counted
/// it. Sharing one ingest path is what keeps the two from drifting again.
struct CodexReportRollups<'a> {
    range: &'a CostUsageDayRange,
    windows: &'a [CurrentUsageWindow],
    today: NaiveDate,
    seven_day_start: NaiveDate,
    daily: HashMap<String, CostSummary>,
    current_windows: HashMap<String, CostSummary>,
    latest: Option<(std::time::SystemTime, CostSummary)>,
    today_sessions: u32,
    seven_day_sessions: u32,
    period_sessions: u32,
    /// Rollout file names already counted. One rollout can appear in the
    /// sessions tree, the archive dir, and across homes/WSL roots.
    seen: HashSet<String>,
}

impl<'a> CodexReportRollups<'a> {
    fn new(
        days: u32,
        range: &'a CostUsageDayRange,
        windows: &'a [CurrentUsageWindow],
        today: NaiveDate,
    ) -> Self {
        Self {
            range,
            windows,
            today,
            seven_day_start: today - Duration::days(6),
            daily: empty_daily_summaries(days),
            current_windows: empty_current_window_summaries(windows),
            latest: None,
            today_sessions: 0,
            seven_day_sessions: 0,
            period_sessions: 0,
            seen: HashSet::new(),
        }
    }

    fn ingest(&mut self, path: &Path) {
        if path
            .extension()
            .is_none_or(|extension| extension != "jsonl")
        {
            return;
        }
        let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
            return;
        };
        if !self.seen.insert(name.to_string()) {
            return;
        }
        let Ok(parsed) = JsonlScanner::parse_codex_file(path, self.range, 0, None, None) else {
            return;
        };

        let mut file_summary = CostSummary::default();
        let mut contributed_today = false;
        let mut contributed_seven_days = false;
        for record in parsed.records.iter().filter(|record| {
            CostUsageDayRange::is_in_range(
                &record.day_key,
                &self.range.since_key,
                &self.range.until_key,
            )
        }) {
            let Some(day_summary) = self.daily.get_mut(&record.day_key) else {
                continue;
            };
            if let Some(cost) = add_codex_record_to_summary(day_summary, record) {
                day_summary.total_cost_usd += cost;
            }
            if let Some(cost) = add_codex_record_to_summary(&mut file_summary, record) {
                file_summary.total_cost_usd += cost;
            }
            add_to_current_windows(
                &mut self.current_windows,
                self.windows,
                record.timestamp,
                |summary| {
                    if let Some(cost) = add_codex_record_to_summary(summary, record) {
                        summary.total_cost_usd += cost;
                    }
                },
            );
            if let Some(date) = CostUsageDayRange::parse_day_key(&record.day_key) {
                contributed_today |= date == self.today;
                contributed_seven_days |= date >= self.seven_day_start;
            }
        }
        if file_summary.input_tokens == 0 && file_summary.output_tokens == 0 {
            return;
        }
        file_summary.sessions_count = 1;
        self.period_sessions += 1;
        self.today_sessions += u32::from(contributed_today);
        self.seven_day_sessions += u32::from(contributed_seven_days);
        let modified = fs::metadata(path)
            .and_then(|metadata| metadata.modified())
            .unwrap_or(std::time::SystemTime::UNIX_EPOCH);
        if self
            .latest
            .as_ref()
            .is_none_or(|(seen, _)| modified > *seen)
        {
            self.latest = Some((modified, file_summary));
        }
    }

    fn finish(self, days: u32) -> CostUsageReport {
        finish_report(
            self.daily,
            days,
            self.latest.map(|(_, summary)| summary),
            (
                self.today_sessions,
                self.seven_day_sessions,
                self.period_sessions,
            ),
            None,
            self.current_windows,
        )
    }
}

fn scan_codex_report(
    scanner: &CostScanner,
    days: u32,
    windows: &[CurrentUsageWindow],
) -> CostUsageReport {
    let today = Local::now().date_naive();
    let start = codex_period_start(today, days);
    let range = CostUsageDayRange::new(start, today);
    let mut rollups = CodexReportRollups::new(days, &range, windows, today);

    for sessions_dir in scanner.get_codex_sessions_dirs() {
        if !sessions_dir.exists() {
            continue;
        }
        for scan_date in codex_scan_dates(&range) {
            let day_dir = sessions_dir
                .join(scan_date.format("%Y").to_string())
                .join(scan_date.format("%m").to_string())
                .join(scan_date.format("%d").to_string());
            let Ok(entries) = fs::read_dir(day_dir) else {
                continue;
            };
            for entry in entries.flatten() {
                rollups.ingest(&entry.path());
            }
        }
    }

    // The archive is a flat dir, so gate on the rollout's filename date rather
    // than walking a date tree that does not exist there.
    for archived_dir in scanner.get_codex_archived_dirs() {
        if !archived_dir.exists() {
            continue;
        }
        let Ok(entries) = fs::read_dir(archived_dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let in_range = path
                .file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| archived_rollout_day_in_range(name, &range));
            if !in_range {
                continue;
            }
            rollups.ingest(&path);
        }
    }

    rollups.finish(days)
}

fn scan_claude_report(
    scanner: &CostScanner,
    days: u32,
    windows: &[CurrentUsageWindow],
) -> CostUsageReport {
    let projects_dir = scanner.get_claude_projects_dir();
    let mut daily = empty_daily_summaries(days);
    let mut current_windows = empty_current_window_summaries(windows);
    if !projects_dir.exists() {
        return finish_report(daily, days, None, (0, 0, 0), None, current_windows);
    }

    let today = Local::now().date_naive();
    let seven_day_start = today - Duration::days(6);
    let cutoff = Utc::now() - Duration::days(days as i64);
    let mut seen = HashSet::new();
    let mut undated = CostSummary::default();
    let mut latest: Option<(DateTime<Utc>, CostSummary)> = None;
    let mut today_sessions = 0;
    let mut seven_day_sessions = 0;
    let mut period_sessions = 0;

    let files = scanner.claude_files_since(&projects_dir, &cutoff, None);
    for (path, records) in parse_claude_files(&files, &cutoff, None) {
        let mut file_summary = CostSummary::default();
        let mut latest_recorded_at: Option<DateTime<Utc>> = None;
        let mut contributed_today = false;
        let mut contributed_seven_days = false;
        let mut counted = 0;
        for record in records {
            if !should_count_claude_record(&record, &cutoff, &mut seen) {
                continue;
            }
            counted += 1;
            add_claude_record_to_summary(&mut file_summary, &record);
            add_to_current_windows(&mut current_windows, windows, record.timestamp, |summary| {
                add_claude_record_to_summary(summary, &record)
            });
            if let Some(timestamp) = record.timestamp {
                let date = timestamp.with_timezone(&Local).date_naive();
                let day = date.format("%Y-%m-%d").to_string();
                if let Some(day_summary) = daily.get_mut(&day) {
                    add_claude_record_to_summary(day_summary, &record);
                }
                contributed_today |= date == today;
                contributed_seven_days |= date >= seven_day_start;
                if latest_recorded_at.is_none_or(|seen_at| timestamp > seen_at) {
                    latest_recorded_at = Some(timestamp);
                }
            } else {
                add_claude_record_to_summary(&mut undated, &record);
            }
        }
        if counted == 0 {
            continue;
        }
        file_summary.sessions_count = 1;
        period_sessions += 1;
        today_sessions += u32::from(contributed_today);
        seven_day_sessions += u32::from(contributed_seven_days);
        let fallback_modified = fs::metadata(&path)
            .and_then(|metadata| metadata.modified())
            .ok()
            .map(DateTime::<Utc>::from)
            .unwrap_or(DateTime::<Utc>::UNIX_EPOCH);
        let recorded_at = latest_recorded_at.unwrap_or(fallback_modified);
        if latest
            .as_ref()
            .is_none_or(|(seen_at, _)| recorded_at > *seen_at)
        {
            latest = Some((recorded_at, file_summary));
        }
    }

    finish_report(
        daily,
        days,
        latest.map(|(_, summary)| summary),
        (today_sessions, seven_day_sessions, period_sessions),
        Some(&undated),
        current_windows,
    )
}

/// Get daily cost history for the last N days
/// Returns Vec of (date_string, cost_usd) sorted by date
pub fn get_daily_cost_history(provider: &str, days: u32) -> Vec<(String, f64)> {
    let scanner = CostScanner::new(days);
    let today = Local::now().date_naive();
    let mut daily_costs: HashMap<String, f64> = HashMap::new();

    // Initialize all days with 0
    for days_ago in 0..days {
        let date = today - Duration::days(days_ago as i64);
        let date_str = date.format("%Y-%m-%d").to_string();
        daily_costs.insert(date_str, 0.0);
    }

    match provider {
        "codex" => {
            // Scan Codex logs by day across Windows and WSL session roots.
            let sessions_dirs = scanner.get_codex_sessions_dirs();
            for days_ago in 0..days {
                let date = today - Duration::days(days_ago as i64);
                let date_str = date.format("%Y-%m-%d").to_string();
                let range = CostUsageDayRange::new(date, date);
                let mut day_cost = 0.0;

                for sessions_dir in sessions_dirs.iter().filter(|dir| dir.exists()) {
                    for scan_date in codex_scan_dates(&range) {
                        let year = scan_date.format("%Y").to_string();
                        let month = scan_date.format("%m").to_string();
                        let day = scan_date.format("%d").to_string();
                        let day_dir = sessions_dir.join(&year).join(&month).join(&day);
                        if !day_dir.exists() {
                            continue;
                        }
                        if let Ok(entries) = fs::read_dir(&day_dir) {
                            for entry in entries.flatten() {
                                let path = entry.path();
                                if path.extension().is_some_and(|e| e == "jsonl") {
                                    day_cost += scan_codex_file_cost_for_range(&path, &range);
                                }
                            }
                        }
                    }
                }
                daily_costs.insert(date_str, day_cost);
            }
        }
        "claude" => {
            // Real per-day breakdown: walk the project logs once,
            // de-duplicating records across files.
            let projects_dir = scanner.get_claude_projects_dir();
            if projects_dir.exists() {
                let cutoff = Utc::now() - Duration::days(days as i64);
                let mut seen = HashSet::new();
                let mut handle_file = |path: &Path| {
                    for_each_claude_usage_record(path, &cutoff, &mut seen, None, |record| {
                        add_claude_record_to_daily_costs(&mut daily_costs, record);
                    });
                };
                scanner.walk_claude_files(&projects_dir, &cutoff, None, &mut handle_file);
            }
        }
        _ => {}
    }

    // Convert to sorted vector
    let mut result: Vec<(String, f64)> = daily_costs.into_iter().collect();
    result.sort_by(|a, b| a.0.cmp(&b.0));
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn project_from_cwd_extracts_basename() {
        assert_eq!(
            project_from_cwd(r"C:\projects\personal\cubby-clipboard").as_deref(),
            Some("cubby-clipboard")
        );
        assert_eq!(
            project_from_cwd(r"\\wsl.localhost\ubuntu-24.04\home\me\burnwatch-app").as_deref(),
            Some("burnwatch-app")
        );
        assert_eq!(
            project_from_cwd("/home/me/projects/ceiling/").as_deref(),
            Some("ceiling")
        );
        assert_eq!(project_from_cwd("   ").as_deref(), None);
        assert_eq!(project_from_cwd("").as_deref(), None);
        // Filesystem roots carry no project name.
        assert_eq!(project_from_cwd(r"C:\").as_deref(), None);
        assert_eq!(project_from_cwd("C:").as_deref(), None);
        assert_eq!(project_from_cwd("/").as_deref(), None);
    }

    #[test]
    fn archived_rollout_day_in_range_gates_by_filename_date() {
        let range = CostUsageDayRange::new(
            NaiveDate::from_ymd_opt(2026, 5, 10).unwrap(),
            NaiveDate::from_ymd_opt(2026, 5, 20).unwrap(),
        );
        assert!(archived_rollout_day_in_range(
            "rollout-2026-05-15T10-00-00-abc.jsonl",
            &range
        ));
        // Outside the padded window.
        assert!(!archived_rollout_day_in_range(
            "rollout-2026-04-01T10-00-00-abc.jsonl",
            &range
        ));
        // Unrecognized names fall through to the parser's own timestamp filter.
        assert!(archived_rollout_day_in_range("weird-name.jsonl", &range));
        // Date-shaped but invalid calendar dates must also fall through, not be
        // skipped lexicographically ("99" would sort past any real month/day).
        assert!(archived_rollout_day_in_range(
            "rollout-2026-99-99T10-00-00-abc.jsonl",
            &range
        ));
    }

    #[test]
    fn mark_unseen_rollout_dedups_by_file_name_not_path() {
        let mut seen = HashSet::new();
        assert!(mark_unseen_rollout(
            Path::new("/a/sessions/2026/05/15/rollout-x.jsonl"),
            &mut seen
        ));
        // Same rollout name in a different directory is a duplicate.
        assert!(!mark_unseen_rollout(
            Path::new("/a/archived_sessions/rollout-x.jsonl"),
            &mut seen
        ));
        assert!(mark_unseen_rollout(
            Path::new("/a/archived_sessions/rollout-y.jsonl"),
            &mut seen
        ));
    }

    fn codex_home_lock() -> &'static std::sync::Mutex<()> {
        static LOCK: std::sync::OnceLock<std::sync::Mutex<()>> = std::sync::OnceLock::new();
        LOCK.get_or_init(|| std::sync::Mutex::new(()))
    }

    /// SOU-296: the report scanner behind the Charts page, the reset windows,
    /// and the API value card only walked `sessions/`. Archiving a Codex task
    /// therefore shrank every total, while the summary scanner still counted
    /// it. The archived rollout must be included, and a rollout present in both
    /// places must still count once.
    #[test]
    fn codex_report_counts_archived_rollouts_exactly_once() {
        let _guard = codex_home_lock().lock().expect("codex home lock");
        let today = Local::now().date_naive();
        let day = today.format("%Y-%m-%d").to_string();
        let ts = format!("{day}T10:00:00.000Z");
        let token_line = format!(
            r#"{{"timestamp":"{ts}","type":"event_msg","payload":{{"type":"token_count","info":{{"last_token_usage":{{"input_tokens":1000,"cached_input_tokens":0,"output_tokens":500}}}}}}}}"#
        );

        let tmp = tempfile::tempdir().unwrap();
        let day_dir = tmp
            .path()
            .join("sessions")
            .join(today.format("%Y").to_string())
            .join(today.format("%m").to_string())
            .join(today.format("%d").to_string());
        std::fs::create_dir_all(&day_dir).unwrap();
        let archived = tmp.path().join("archived_sessions");
        std::fs::create_dir_all(&archived).unwrap();

        let shared = format!("rollout-{day}-11111111-1111-1111-1111-111111111111.jsonl");
        let archived_only = format!("rollout-{day}-22222222-2222-2222-2222-222222222222.jsonl");
        std::fs::write(day_dir.join(&shared), &token_line).unwrap();
        std::fs::write(archived.join(&shared), &token_line).unwrap();
        std::fs::write(archived.join(&archived_only), &token_line).unwrap();

        let previous = std::env::var("CODEX_HOME").ok();
        // SAFETY: serialized by `codex_home_lock`, and restored below.
        unsafe { std::env::set_var("CODEX_HOME", tmp.path()) };
        let report = scan_codex_report(&CostScanner::new(2), 2, &[]);
        unsafe {
            match previous {
                Some(value) => std::env::set_var("CODEX_HOME", value),
                None => std::env::remove_var("CODEX_HOME"),
            }
        }

        assert_eq!(
            report.thirty_days.sessions_count, 2,
            "the archived-only rollout must count, and the shared one only once"
        );
        assert_eq!(
            report.thirty_days.input_tokens, 2000,
            "two rollouts of 1000 input tokens, with no double count"
        );
    }

    #[test]
    fn codex_scan_dedups_same_rollout_across_sessions_and_archived() {
        let today = Local::now().date_naive();
        let day = today.format("%Y-%m-%d").to_string();
        let ts = format!("{day}T10:00:00.000Z");
        let token_line = format!(
            r#"{{"timestamp":"{ts}","type":"event_msg","payload":{{"type":"token_count","info":{{"last_token_usage":{{"input_tokens":1000,"cached_input_tokens":0,"output_tokens":500}}}}}}}}"#
        );

        let tmp = tempfile::tempdir().unwrap();
        let day_dir = tmp
            .path()
            .join("sessions")
            .join(today.format("%Y").to_string())
            .join(today.format("%m").to_string())
            .join(today.format("%d").to_string());
        std::fs::create_dir_all(&day_dir).unwrap();
        let archived = tmp.path().join("archived_sessions");
        std::fs::create_dir_all(&archived).unwrap();

        let shared = format!("rollout-{day}-11111111-1111-1111-1111-111111111111.jsonl");
        let unique = format!("rollout-{day}-22222222-2222-2222-2222-222222222222.jsonl");
        // The same rollout lives in both the nested and archived trees.
        std::fs::write(day_dir.join(&shared), &token_line).unwrap();
        std::fs::write(archived.join(&shared), &token_line).unwrap();
        // A second rollout only in archived/.
        std::fs::write(archived.join(&unique), &token_line).unwrap();

        let scanner = CostScanner::new(2);
        let range = CostUsageDayRange::new(today - Duration::days(1), today);
        let mut summary = CostSummary::default();
        let mut seen = HashSet::new();
        scanner.scan_codex_sessions_dir(
            &tmp.path().join("sessions"),
            &range,
            &mut summary,
            &mut seen,
            None,
        );
        scanner.scan_codex_archived_dir(&archived, &range, &mut summary, &mut seen, None);

        // The shared rollout is counted once, plus the unique one: two sessions.
        assert_eq!(summary.sessions_count, 2);
    }

    #[test]
    fn claude_line_prefilter_accepts_usage_events_and_rejects_other_payloads() {
        let usage = br#"{"type":"assistant","message":{"usage":{"input_tokens":1}}}"#;
        let tool = br#"{"type":"user","message":{"content":"assistant usage"}}"#;

        assert!(contains_bytes(usage, b"\"assistant\""));
        assert!(contains_bytes(usage, b"\"usage\""));
        assert!(!contains_bytes(tool, b"\"assistant\""));
    }

    #[test]
    fn test_unknown_model_falls_back_to_sonnet() {
        // Unknown/retired Claude IDs fall back to Sonnet 4.6 base pricing
        // ($3/1M input, $15/1M output). 100k tokens stay under the 200k tier.
        let cost =
            ClaudePricing::cost_usd_with_cache_ttl("claude-3-5-sonnet", 100_000, 0, 0, 0, 100_000);
        // 100k * $3/M + 100k * $15/M = 0.30 + 1.50 = 1.80
        assert!((cost - 1.80).abs() < 0.001);
    }

    #[test]
    fn records_unknown_claude_model_while_using_fallback_cost() {
        let event: ClaudeEvent = serde_json::from_str(
            r#"{"type":"assistant","timestamp":"2026-01-15T10:00:00Z","requestId":"req_unknown","message":{"id":"msg_unknown","model":"claude-retired-unknown","usage":{"input_tokens":100000,"output_tokens":100000}}}"#,
        )
        .unwrap();
        let record = claude_usage_record_from_event(&event).expect("usage record");
        let mut summary = CostSummary::default();

        add_claude_record_to_summary(&mut summary, &record);

        assert!(summary.total_cost_usd > 0.0);
        assert!(summary.unknown_models.contains("claude-retired-unknown"));
    }

    #[test]
    fn test_claude_fable_5_pricing() {
        let cost = ClaudePricing::cost_usd_with_cache_ttl("claude-fable-5", 100, 10, 0, 20, 5);
        let expected = (100.0 / 1_000_000.0) * 10.00
            + (10.0 / 1_000_000.0) * 12.50
            + (20.0 / 1_000_000.0) * 1.00
            + (5.0 / 1_000_000.0) * 50.00;
        assert!((cost - expected).abs() < f64::EPSILON);
    }

    #[test]
    fn test_claude_one_hour_cache_write_pricing() {
        let cost = ClaudePricing::cost_usd_with_cache_ttl("claude-fable-5", 100, 30, 20, 20, 5);
        let expected = (100.0 / 1_000_000.0) * 10.00
            + (10.0 / 1_000_000.0) * 12.50
            + (20.0 / 1_000_000.0) * 20.00
            + (20.0 / 1_000_000.0) * 1.00
            + (5.0 / 1_000_000.0) * 50.00;
        assert!((cost - expected).abs() < f64::EPSILON);
    }

    #[test]
    fn test_claude_sonnet_46_uses_standard_rate_across_full_context() {
        let cost = ClaudePricing::cost_usd_with_cache_ttl("claude-sonnet-4-6", 240_000, 0, 0, 0, 0);
        assert!((cost - 0.72).abs() < 0.001);
    }

    #[test]
    fn test_claude_sonnet_5_pricing_is_date_aware() {
        let promo = ClaudePricing::cost_usd_with_cache_ttl_on_date(
            "claude-sonnet-5",
            1_000_000,
            0,
            0,
            0,
            1_000_000,
            NaiveDate::from_ymd_opt(2026, 8, 31).unwrap(),
        );
        let standard = ClaudePricing::cost_usd_with_cache_ttl_on_date(
            "claude-sonnet-5",
            1_000_000,
            0,
            0,
            0,
            1_000_000,
            NaiveDate::from_ymd_opt(2026, 9, 1).unwrap(),
        );
        assert!((promo - 12.0).abs() < 0.001);
        assert!((standard - 18.0).abs() < 0.001);
    }

    #[test]
    fn test_current_gen_opus_uses_5_25_pricing() {
        // Opus 4.5/4.6/4.7/4.8 bill at $5/1M input + $25/1M output = $30 total.
        // Delegation regression guard: opus-4-8 in particular must resolve
        // through the canonical table (it was missing there before this fix).
        for model in [
            "claude-opus-4-5",
            "claude-opus-4-6",
            "claude-opus-4-7",
            "claude-opus-4-8",
        ] {
            let cost = ClaudePricing::cost_usd_with_cache_ttl(model, 1_000_000, 0, 0, 0, 1_000_000);
            assert!(
                (cost - 30.00).abs() < 0.001,
                "{model} should bill $30 ($5 in + $25 out), got {cost}"
            );
        }
    }

    #[test]
    fn test_legacy_opus_keeps_legacy_pricing() {
        // Legacy Opus 4.0 / 4.1 remain at $15/1M input + $75/1M output = $90 in
        // the canonical table. (Retired IDs absent from the table — e.g. Opus 3
        // `claude-3-opus-...` — fall back to Sonnet instead; they are outside
        // any realistic 30-day scan window.)
        for model in ["claude-opus-4-20250514", "claude-opus-4-1"] {
            let cost = ClaudePricing::cost_usd_with_cache_ttl(model, 1_000_000, 0, 0, 0, 1_000_000);
            assert!(
                (cost - 90.00).abs() < 0.001,
                "{model} should bill $90 ($15 in + $75 out), got {cost}"
            );
        }
    }

    #[test]
    fn test_haiku_45_uses_current_pricing() {
        // Haiku 4.5 bills at $1/1M input + $5/1M output = $6 via the canonical
        // table (previously the scanner under-priced it at the Haiku 3 rate).
        let cost = ClaudePricing::cost_usd_with_cache_ttl(
            "claude-haiku-4-5",
            1_000_000,
            0,
            0,
            0,
            1_000_000,
        );
        assert!(
            (cost - 6.00).abs() < 0.001,
            "haiku-4-5 should bill $6 ($1 in + $5 out), got {cost}"
        );
    }

    #[test]
    fn parses_current_codex_payload_token_count_events() {
        let path = std::env::temp_dir().join(format!(
            "codexbar-current-codex-token-count-{}.jsonl",
            std::process::id()
        ));
        // Use a recent timestamp so the event stays inside the scanner's
        // 30-day window no matter when the test runs. A hardcoded date
        // silently ages out of the window and makes this test fail with 0
        // sessions once it is more than 30 days in the past.
        let recent = (Utc::now() - Duration::hours(1))
            .format("%Y-%m-%dT%H:%M:%S%.3fZ")
            .to_string();
        let mut file = File::create(&path).unwrap();
        writeln!(
            file,
            r#"{{"timestamp":"{ts}","type":"event_msg","payload":{{"type":"token_count","info":{{"model":"gpt-5","total_token_usage":{{"input_tokens":125,"cached_input_tokens":30,"output_tokens":15}}}}}}}}"#,
            ts = recent
        )
        .unwrap();
        drop(file);

        let scanner = CostScanner::new(30);
        let mut summary = CostSummary::default();
        scanner.parse_codex_file(&path, &mut summary, None);

        assert_eq!(summary.sessions_count, 1);
        assert_eq!(summary.input_tokens, 125);
        assert_eq!(summary.cached_tokens, 30);
        assert_eq!(summary.cache_read_tokens, 30);
        assert_eq!(summary.cache_write_tokens, 0);
        assert_eq!(summary.output_tokens, 15);
        assert_eq!(
            summary
                .by_model_tokens
                .get("gpt-5")
                .map(ModelTokenCounts::total),
            Some(140)
        );
        assert!(scan_codex_file_cost(&path) > 0.0);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn derives_claude_dedup_key_from_message_and_request_ids() {
        assert_eq!(
            claude_usage_dedup_key(Some("msg_1"), Some("req_1")).as_deref(),
            Some("msg_1:req_1")
        );
        assert_eq!(
            claude_usage_dedup_key(Some("msg_1"), None).as_deref(),
            Some("message:msg_1")
        );
        assert_eq!(
            claude_usage_dedup_key(None, Some("req_1")).as_deref(),
            Some("request:req_1")
        );
        assert_eq!(claude_usage_dedup_key(None, None), None);
    }

    #[test]
    fn counts_claude_usage_once_across_duplicate_records() {
        // The same API response can be replayed into several transcript files
        // (session resume, sidechains); it must only be counted once.
        let event: ClaudeEvent = serde_json::from_str(
            r#"{"type":"assistant","timestamp":"2026-01-15T10:00:00Z","requestId":"req_1","message":{"id":"msg_1","model":"claude-sonnet-4-6","usage":{"input_tokens":100,"output_tokens":50,"cache_creation_input_tokens":10,"cache_read_input_tokens":20}}}"#,
        )
        .unwrap();

        let record = claude_usage_record_from_event(&event).expect("usage record");
        assert_eq!(record.model, "claude-sonnet-4-6");
        assert_eq!(record.input, 100);
        assert_eq!(record.output, 50);
        assert_eq!(record.cache_create, 10);
        assert_eq!(record.cache_read, 20);
        assert!(record.cost > 0.0);

        let cutoff = DateTime::parse_from_rfc3339("2026-01-01T00:00:00Z")
            .unwrap()
            .with_timezone(&Utc);
        let mut seen = HashSet::new();
        assert!(should_count_claude_record(&record, &cutoff, &mut seen));
        assert!(!should_count_claude_record(&record, &cutoff, &mut seen));
    }

    #[test]
    fn claude_transcript_discovery_is_deterministically_sorted() {
        let dir = tempfile::tempdir().expect("temp directory");
        let projects = dir.path().join("projects");
        std::fs::create_dir_all(&projects).expect("create projects directory");
        for name in ["z-last.jsonl", "a-first.jsonl", "m-middle.jsonl"] {
            std::fs::write(projects.join(name), "{}\n").expect("write transcript");
        }
        let scanner = CostScanner::new(30);
        let cutoff = Utc::now() - Duration::days(1);

        let files = scanner.claude_files_since(&projects, &cutoff, None);
        let names = files
            .iter()
            .filter_map(|path| path.file_name().and_then(|name| name.to_str()))
            .collect::<Vec<_>>();

        assert_eq!(names, ["a-first.jsonl", "m-middle.jsonl", "z-last.jsonl"]);
    }

    #[test]
    fn rejects_claude_records_before_cutoff() {
        let event: ClaudeEvent = serde_json::from_str(
            r#"{"type":"assistant","timestamp":"2025-12-01T10:00:00Z","requestId":"req_old","message":{"id":"msg_old","model":"claude-sonnet-4-6","usage":{"input_tokens":1,"output_tokens":1}}}"#,
        )
        .unwrap();
        let record = claude_usage_record_from_event(&event).expect("usage record");
        let cutoff = DateTime::parse_from_rfc3339("2026-01-01T00:00:00Z")
            .unwrap()
            .with_timezone(&Utc);
        let mut seen = HashSet::new();
        assert!(!should_count_claude_record(&record, &cutoff, &mut seen));
    }

    #[test]
    fn ignores_claude_events_without_countable_usage() {
        // Non-assistant events carry no billable usage.
        let event: ClaudeEvent =
            serde_json::from_str(r#"{"type":"user","message":{"usage":{"input_tokens":5}}}"#)
                .unwrap();
        assert!(claude_usage_record_from_event(&event).is_none());

        // Zero-token usage blocks (e.g. synthetic messages) are not sessions.
        let event: ClaudeEvent = serde_json::from_str(
            r#"{"type":"assistant","message":{"id":"msg_zero","model":"claude-sonnet-4-6","usage":{"input_tokens":0,"output_tokens":0}}}"#,
        )
        .unwrap();
        assert!(claude_usage_record_from_event(&event).is_none());
    }

    fn claude_transcript_line(
        timestamp: &str,
        request_key: &str,
        request_id: &str,
        message_id: &str,
    ) -> String {
        format!(
            r#"{{"type":"assistant","timestamp":"{timestamp}","{request_key}":"{request_id}","message":{{"id":"{message_id}","model":"claude-sonnet-4-6","usage":{{"input_tokens":1000,"output_tokens":500}}}}}}"#
        )
    }

    #[test]
    fn daily_history_dedups_across_files_and_buckets_by_local_day() {
        // End-to-end regression for the daily buckets: two transcript files,
        // two different days, plus a replay of the day-one record in the
        // second file (snake_case request_id, as another writer would emit).
        let dir = std::env::temp_dir();
        let file_a = dir.join(format!(
            "codexbar-claude-daily-a-{}.jsonl",
            std::process::id()
        ));
        let file_b = dir.join(format!(
            "codexbar-claude-daily-b-{}.jsonl",
            std::process::id()
        ));

        // >24h apart guarantees two distinct local calendar days.
        let day_one = Utc::now() - Duration::hours(30);
        let day_two = Utc::now() - Duration::hours(2);
        let ts_one = day_one.format("%Y-%m-%dT%H:%M:%S%.3fZ").to_string();
        let ts_two = day_two.format("%Y-%m-%dT%H:%M:%S%.3fZ").to_string();

        std::fs::write(
            &file_a,
            format!(
                "{}\n{}\n",
                claude_transcript_line(&ts_one, "requestId", "req_1", "msg_1"),
                claude_transcript_line(&ts_two, "requestId", "req_2", "msg_2"),
            ),
        )
        .unwrap();
        std::fs::write(
            &file_b,
            format!(
                "{}\n",
                claude_transcript_line(&ts_one, "request_id", "req_1", "msg_1"),
            ),
        )
        .unwrap();

        let day_key = |ts: &DateTime<Utc>| {
            ts.with_timezone(&Local)
                .date_naive()
                .format("%Y-%m-%d")
                .to_string()
        };
        let mut daily_costs = HashMap::new();
        daily_costs.insert(day_key(&day_one), 0.0);
        daily_costs.insert(day_key(&day_two), 0.0);

        let cutoff = Utc::now() - Duration::days(30);
        let mut seen = HashSet::new();
        for path in [&file_a, &file_b] {
            for_each_claude_usage_record(path, &cutoff, &mut seen, None, |record| {
                add_claude_record_to_daily_costs(&mut daily_costs, record);
            });
        }

        let day_one_cost = daily_costs[&day_key(&day_one)];
        let day_two_cost = daily_costs[&day_key(&day_two)];
        assert!(day_one_cost > 0.0, "day one should carry real cost");
        // Identical usage on both days: equal buckets proves the file-b
        // replay was de-duplicated (a leak would double day one).
        assert!(
            (day_one_cost - day_two_cost).abs() < f64::EPSILON,
            "each day should hold exactly one record's cost, got {day_one_cost} vs {day_two_cost}"
        );

        let _ = std::fs::remove_file(&file_a);
        let _ = std::fs::remove_file(&file_b);
    }
}
