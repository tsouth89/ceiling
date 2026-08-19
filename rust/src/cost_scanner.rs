//! Local cost-usage scanner for Codex, Claude, and Grok Build.
//!
//! Grok dollars come from session `costUsdTicks` (API-equivalent), not a
//! fabricated SuperGrok rate card.
//!
//! Scans local JSONL log files to aggregate token usage and calculate costs

use chrono::{DateTime, Duration, Local, NaiveDate, Timelike, Utc};
use serde::Deserialize;
use std::collections::{HashMap, HashSet};
use std::fs::{self, File};
use std::io::{BufRead, BufReader, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use crate::codex_cost_speed::{self, CodexCostSpeed};
#[cfg(test)]
use crate::codex_costs::scan_codex_file_cost;
use crate::codex_costs::{
    add_codex_record_to_summary, add_codex_records_to_summary, codex_period_start,
    codex_scan_dates, project_bucket, scan_codex_file_cost_for_range,
};
use crate::codex_sessions::{codex_sessions_dir_candidates, default_wsl_roots};
use crate::core::{
    CodexUsageRecord, ConfiguredAccounts, CostUsageDayRange, CostUsagePricing, JsonlScanner,
    ProviderId,
};
use crate::grok_costs::{
    GrokUsageRecord, discover_grok_session_dirs, grok_sessions_dir, load_session_meta,
    parse_grok_updates_file, should_count_grok_record,
};
use crate::settings::Settings;
use crate::usage_index::{FileFacts, IndexStore, Lookup, NewEntry, file_facts};

/// Config directories for every directory-backed account of `provider`.
///
/// Estimated API value and other unscoped totals must include these homes;
/// capacity multi-account setup alone does not put them in
/// `codex_custom_sessions_dirs`.
fn configured_account_homes(provider: ProviderId) -> Vec<PathBuf> {
    ConfiguredAccounts::load()
        .targets_for(provider)
        .into_iter()
        .map(|target| target.config_dir)
        .filter(|dir| !dir.as_os_str().is_empty())
        .collect()
}

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
    /// Reasoning / thinking tokens reported by the provider (display-only;
    /// often a subset of output, so not added into processed totals).
    pub reasoning_tokens: u64,
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
    /// Token split by the subscription plan in force when each delta was
    /// billed, keyed "unattributed" when the log never declared one. Local
    /// logs carry no account identity, so this is the only signal that a
    /// machine's activity spans more than one account.
    pub by_plan_tokens: HashMap<String, ModelTokenCounts>,
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
    /// Codex cost speed tier applied to dollar fields (`standard` / `fast`).
    /// `None` for non-Codex summaries.
    pub codex_cost_speed: Option<String>,
    /// Raw `service_tier` from Codex config when discovered (e.g. `priority`).
    pub codex_service_tier: Option<String>,
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
    /// Exact token totals and priced cost for caller-supplied reset windows,
    /// keyed by ID.
    pub current_windows: HashMap<String, CostSummary>,
    /// Local clock-hour buckets for the peak-hours heatmap, oldest first.
    /// Hours with no activity are absent rather than zero-filled.
    pub hourly_activity: Vec<HourlyActivityPoint>,
}

/// One local clock-hour of observed activity.
///
/// This is a strict refinement of `daily_costs`: a record is only credited to
/// an hour when its local day is already inside the scanned range, so summing
/// the hours of a day always reproduces that day's total. Buckets use the
/// machine's local clock, because "when do I actually code" is a question about
/// wall-clock hours, not UTC.
#[derive(Debug, Clone)]
pub struct HourlyActivityPoint {
    /// Local calendar date.
    pub date: NaiveDate,
    /// Local hour of day, 0-23.
    pub hour: u32,
    pub summary: CostSummary,
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
    // Codex and Grok report cache-read tokens inside the input total.
    matches!(provider_id.to_ascii_lowercase().as_str(), "codex" | "grok")
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

/// Midnight at the start of `day` in the local timezone, as UTC.
///
/// Used so Claude `--days N` is an inclusive local calendar window (matching
/// Codex / ccusage) instead of a rolling UTC duration.
fn local_day_start_utc(day: NaiveDate) -> DateTime<Utc> {
    day.and_hms_opt(0, 0, 0)
        .expect("valid midnight")
        .and_local_timezone(Local)
        .earliest()
        .or_else(|| {
            day.and_hms_opt(0, 0, 0)
                .expect("valid midnight")
                .and_local_timezone(Local)
                .latest()
        })
        .map(|local| local.with_timezone(&Utc))
        .unwrap_or_else(|| {
            DateTime::<Utc>::from_naive_utc_and_offset(
                day.and_hms_opt(0, 0, 0).expect("valid midnight"),
                Utc,
            )
        })
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

/// One priced Claude usage event.
///
/// `pub(crate)` so [`crate::usage_index`] can store and rebuild these; the
/// dollar figure is computed here at parse time, which is why the index
/// invalidates itself when prices change.
#[derive(Debug, Clone)]
pub(crate) struct ClaudeUsageRecord {
    pub(crate) model: String,
    pub(crate) timestamp: Option<DateTime<Utc>>,
    pub(crate) dedup_key: Option<String>,
    /// Project/repo (basename of the line's cwd), for per-project cost.
    pub(crate) project: Option<String>,
    pub(crate) input: u64,
    pub(crate) output: u64,
    pub(crate) cache_create: u64,
    pub(crate) cache_read: u64,
    pub(crate) cost: f64,
}

/// Cost usage scanner
pub struct CostScanner {
    days: u32,
    /// When set, scan only this provider config directory's logs instead of
    /// every candidate home. This is what makes one account's charts distinct
    /// from another's: each account is its own directory.
    scoped_home: Option<PathBuf>,
    /// Codex list-rate vs priority/fast pricing (ccusage `--speed` parity).
    codex_speed: CodexCostSpeed,
    /// When set, use these account config homes instead of loading
    /// [`ConfiguredAccounts`] (unit tests inject temp dirs). Production leaves
    /// this `None` so live multi-account setup is always read at scan time.
    account_homes_override: Option<Vec<PathBuf>>,
    /// When set, stands in for the ambient config home of whichever provider is
    /// being scanned: `CLAUDE_CONFIG_DIR`, `CODEX_HOME`, or `GROK_HOME`.
    ///
    /// Tests used to `set_var` these instead. Mutating process environment is
    /// undefined behaviour while any other thread reads it, and the rest of the
    /// suite reads both constantly, which made unrelated scans intermittently
    /// resolve the wrong home. Injecting the value keeps it thread-local to the
    /// scanner. Production leaves this `None` and reads the real environment.
    ambient_home_override: Option<PathBuf>,
    /// Whether to bucket records by local clock-hour as well as by day.
    ///
    /// Off by default. Filling the hourly buckets means running each record's
    /// full accounting a second time, and only the activity heatmap reads them
    /// — the charts, the reset windows, and the API-value card do not, and
    /// those run on every refresh over transcript trees that reach gigabytes.
    collect_hourly: bool,
}

impl CostScanner {
    /// Create a new scanner for the last N days.
    ///
    /// Codex dollars follow ccusage auto speed: `service_tier = "priority"` in
    /// `~/.codex/config.toml` prices at the fast (2×) tier.
    ///
    /// Unscoped scans include every configured directory-backed account home
    /// (Codex/Claude multi-account) so Estimated API value matches total usage.
    pub fn new(days: u32) -> Self {
        Self {
            days,
            scoped_home: None,
            codex_speed: CodexCostSpeed::resolve(None),
            account_homes_override: None,
            ambient_home_override: None,
            collect_hourly: false,
        }
    }

    /// Scanner with an explicit Codex cost speed (`standard` / `fast` / `auto`).
    pub fn with_codex_speed(days: u32, speed_override: Option<&str>) -> Self {
        Self {
            days,
            scoped_home: None,
            codex_speed: CodexCostSpeed::resolve(speed_override),
            account_homes_override: None,
            ambient_home_override: None,
            collect_hourly: false,
        }
    }

    /// A scanner that reads only `home`'s logs (an account's config directory).
    pub fn scoped_to(days: u32, home: PathBuf) -> Self {
        Self {
            days,
            scoped_home: Some(home),
            codex_speed: CodexCostSpeed::resolve(None),
            account_homes_override: None,
            ambient_home_override: None,
            collect_hourly: false,
        }
    }

    /// Also bucket records by local clock-hour (the activity heatmap).
    pub fn with_hourly_activity(mut self) -> Self {
        self.collect_hourly = true;
        self
    }

    /// Stand in for the ambient config home on an already-built scanner.
    #[cfg(test)]
    pub(crate) fn with_ambient_home(mut self, home: PathBuf) -> Self {
        self.ambient_home_override = Some(home);
        self
    }

    /// Treats `homes` as the only configured account directories (no live
    /// [`ConfiguredAccounts`] load) and stands in for the ambient config
    /// directory, instead of mutating the process environment.
    #[cfg(test)]
    fn with_ambient_and_account_homes(days: u32, ambient: PathBuf, homes: Vec<PathBuf>) -> Self {
        Self {
            days,
            scoped_home: None,
            codex_speed: CodexCostSpeed::resolve(None),
            account_homes_override: Some(homes),
            ambient_home_override: Some(ambient),
            collect_hourly: false,
        }
    }

    fn account_homes(&self, provider: ProviderId) -> Vec<PathBuf> {
        if let Some(homes) = &self.account_homes_override {
            return homes.clone();
        }
        // Unit-test builds must not pull the developer's real multi-account
        // store into fixtures that set CODEX_HOME. Multi-account coverage uses
        // `with_account_homes`. Production always loads ConfiguredAccounts.
        if cfg!(test) {
            return Vec::new();
        }
        configured_account_homes(provider)
    }

    /// Codex cost speed tier this scanner applies to dollar totals.
    pub fn codex_speed(&self) -> CodexCostSpeed {
        self.codex_speed
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

        codex_cost_speed::apply_speed_to_summary(&mut summary, self.codex_speed);
        summary
    }

    /// Scan Grok Build local session logs (`~/.grok/sessions`).
    ///
    /// Dollars come from session `costUsdTicks` (API-equivalent), the same
    /// figure Charts already shows. SBS-934: `codexbar cost` and serve `/cost`
    /// must call this instead of treating Grok as unsupported.
    pub fn scan_grok(&self) -> CostSummary {
        scan_grok_report(self, self.days, &[]).thirty_days
    }

    /// True when local session logs can be priced for this provider.
    ///
    /// Charts, `codexbar cost`, serve `/cost`, and `codexbar mcp` get_spend
    /// must agree. Cursor, Gemini, and Copilot stay unsupported.
    pub fn supports_local_scan(provider: ProviderId) -> bool {
        matches!(
            provider,
            ProviderId::Codex | ProviderId::Claude | ProviderId::Grok
        )
    }

    /// Scan one provider's local logs, or `None` when that provider has none.
    pub fn scan_provider(&self, provider: ProviderId) -> Option<CostSummary> {
        match provider {
            ProviderId::Codex => Some(self.scan_codex()),
            ProviderId::Claude => Some(self.scan_claude()),
            ProviderId::Grok => Some(self.scan_grok()),
            _ => None,
        }
    }

    /// Scan Claude local logs
    pub fn scan_claude(&self) -> CostSummary {
        self.scan_claude_with_cancel(None)
    }

    /// Scan Claude local logs, stopping early when the caller cancels the scan.
    pub fn scan_claude_with_cancel(&self, cancel: Option<&AtomicBool>) -> CostSummary {
        let projects_dirs = self
            .get_claude_projects_dirs()
            .into_iter()
            .filter(|dir| dir.is_dir())
            .collect::<Vec<_>>();
        if projects_dirs.is_empty() {
            return CostSummary::default();
        }

        let mut summary = CostSummary::default();
        // Inclusive local calendar window — same semantics as Codex `--days N`
        // and ccusage `--since`/`--until` on the local machine.
        let today = Local::now().date_naive();
        let start_date = codex_period_start(today, self.days);
        let cutoff = local_day_start_utc(start_date);

        summary.period_start = Some(start_date);
        summary.period_end = Some(today);

        // Parse independent transcript files in parallel, then apply the
        // cross-file de-duplication in deterministic file order. One `seen`
        // set spans every account home so the same record is never double
        // counted if two roots overlap.
        let mut files = Vec::new();
        for projects_dir in &projects_dirs {
            if is_cancelled(cancel) {
                break;
            }
            files.extend(self.claude_files_since(projects_dir, &cutoff, cancel));
        }
        files.sort();
        files.dedup();
        let mut seen = HashSet::new();
        for_each_claude_file(&files, &cutoff, cancel, |_, records| {
            let mut counted = 0;
            for record in records {
                if !should_count_claude_record(record, &cutoff, &mut seen) {
                    continue;
                }
                counted += 1;
                add_claude_record_to_summary(&mut summary, record);
            }
            if counted > 0 {
                summary.sessions_count += 1;
            }
        });

        summary
    }

    fn get_codex_sessions_dirs(&self) -> Vec<PathBuf> {
        if let Some(home) = &self.scoped_home {
            // Only this account's directory. No ambient home, no custom dirs, no
            // WSL roots, or the scan would pull in other accounts' logs.
            return codex_sessions_dir_candidates(
                None,
                Some(home.to_string_lossy().into_owned()),
                &[],
                &[],
            );
        }
        let settings = Settings::load();
        let codex_home = match &self.ambient_home_override {
            Some(home) => Some(home.to_string_lossy().into_owned()),
            None => std::env::var("CODEX_HOME").ok(),
        };
        // Merge custom session roots with every configured multi-account
        // CODEX_HOME. `codex_sessions_dir_candidates` dedups by path so an
        // account that is also ambient / listed in custom dirs is not double
        // scanned; rollouts are also deduped by filename across homes.
        let mut custom_dirs = settings.codex_custom_sessions_dirs.clone();
        for home in self.account_homes(ProviderId::Codex) {
            custom_dirs.push(home.to_string_lossy().into_owned());
        }
        codex_sessions_dir_candidates(
            dirs::home_dir(),
            codex_home,
            &custom_dirs,
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
        let files = codex_session_files(sessions_dir, range, seen, cancel);
        parse_codex_files_into(&files, range, summary, cancel);
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
        let files = codex_archived_files(archived_dir, range, seen, cancel);
        parse_codex_files_into(&files, range, summary, cancel);
    }

    /// Every Claude `projects/` root to scan unscoped: ambient config, then
    /// each configured multi-account `CLAUDE_CONFIG_DIR`, deduped by path.
    fn get_claude_projects_dirs(&self) -> Vec<PathBuf> {
        if let Some(home) = &self.scoped_home {
            return vec![home.join("projects")];
        }

        let mut dirs = Vec::new();
        let mut seen = HashSet::new();
        let push = |dirs: &mut Vec<PathBuf>, seen: &mut HashSet<PathBuf>, path: PathBuf| {
            // Existing directories canonicalize to a stable spelling and
            // separator form on Windows. Unix keeps case-distinct paths
            // distinct instead of collapsing them through lowercasing.
            let key = fs::canonicalize(&path).unwrap_or_else(|_| path.clone());
            if seen.insert(key) {
                dirs.push(path);
            }
        };

        let ambient = match &self.ambient_home_override {
            Some(home) => Ok(home.to_string_lossy().into_owned()),
            None => std::env::var("CLAUDE_CONFIG_DIR"),
        };
        if let Ok(claude_config) = ambient {
            let trimmed = claude_config.trim();
            if !trimmed.is_empty() {
                push(
                    &mut dirs,
                    &mut seen,
                    PathBuf::from(trimmed).join("projects"),
                );
            }
        } else {
            let home = dirs::home_dir().unwrap_or_else(|| PathBuf::from("."));
            let primary = home.join(".claude").join("projects");
            if primary.exists() {
                push(&mut dirs, &mut seen, primary);
            } else {
                push(
                    &mut dirs,
                    &mut seen,
                    home.join(".config").join("claude").join("projects"),
                );
            }
        }

        for account_home in self.account_homes(ProviderId::Claude) {
            push(&mut dirs, &mut seen, account_home.join("projects"));
        }

        dirs
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
            // `entry.file_type()` and `entry.metadata()` reuse what the
            // directory read already returned on Windows. `path.is_dir()` plus
            // `fs::metadata(&path)` re-stat every entry instead, which on a
            // projects tree of a thousand transcripts is three syscalls per
            // file where one will do.
            let Ok(file_type) = entry.file_type() else {
                continue;
            };
            let path = entry.path();
            if file_type.is_dir() {
                self.walk_claude_files(&path, cutoff, cancel, on_file);
            } else if path.extension().is_some_and(|e| e == "jsonl") {
                // Only files touched inside the window can hold in-window
                // records, so the mtime is the cheap gate before opening one.
                if let Ok(metadata) = entry.metadata()
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

/// Rollout paths in `range` under a date-nested `sessions/` tree, deduped by
/// file name, in scan order.
///
/// Collected before anything is parsed so the files can be read concurrently.
/// The walk itself only touches directory metadata; the reading is what costs.
fn codex_session_files(
    sessions_dir: &Path,
    range: &CostUsageDayRange,
    seen: &mut HashSet<String>,
    cancel: Option<&AtomicBool>,
) -> Vec<PathBuf> {
    let mut files = Vec::new();
    // Walk the date-based directory structure with one day of padding on each
    // side. Codex JSONL timestamps are UTC while the tray presents local
    // calendar days; the parser filters back to `range`.
    for date in codex_scan_dates(range) {
        if is_cancelled(cancel) {
            break;
        }
        let day_dir = sessions_dir
            .join(date.format("%Y").to_string())
            .join(date.format("%m").to_string())
            .join(date.format("%d").to_string());
        let Ok(entries) = fs::read_dir(&day_dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().is_some_and(|e| e == "jsonl") && mark_unseen_rollout(&path, seen) {
                files.push(path);
            }
        }
    }
    files
}

/// Rollout paths in `range` from a flat `archived_sessions/` dir, deduped by
/// file name. Files are `rollout-<date>-<id>.jsonl`; the date prefix is what
/// keeps rollouts far outside the window from being read at all.
fn codex_archived_files(
    archived_dir: &Path,
    range: &CostUsageDayRange,
    seen: &mut HashSet<String>,
    cancel: Option<&AtomicBool>,
) -> Vec<PathBuf> {
    let mut files = Vec::new();
    let Ok(entries) = fs::read_dir(archived_dir) else {
        return files;
    };
    for entry in entries.flatten() {
        if is_cancelled(cancel) {
            break;
        }
        let path = entry.path();
        if path.extension().is_none_or(|e| e != "jsonl") {
            continue;
        }
        let in_range = path
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| archived_rollout_day_in_range(name, range));
        if in_range && mark_unseen_rollout(&path, seen) {
            files.push(path);
        }
    }
    files
}

/// The widest day range the parser accepts.
///
/// Rollouts are parsed against this rather than the caller's window so one
/// index entry serves every window: a narrower range would bake the caller's
/// dates into the stored records. Every fold filters by its own range anyway,
/// so a wider parse only ever supplies a superset.
///
/// The end deliberately stops short of year 9999. [`CostUsageDayRange`] pads
/// its bounds by a day and compares the formatted keys as text, and a padded
/// year 10000 formats as `+10000-01-01`, which sorts *below* every real date
/// and would exclude everything.
fn unbounded_day_range() -> CostUsageDayRange {
    CostUsageDayRange::new(
        NaiveDate::from_ymd_opt(1970, 1, 1).expect("valid date"),
        NaiveDate::from_ymd_opt(9000, 1, 1).expect("valid date"),
    )
}

/// Stream each rollout's records to `fold`, in file order, using the index.
///
/// A rollout's token counts are cumulative and the parser carries state across
/// its lines, so a changed rollout is re-read whole rather than resumed. In
/// practice only the session being written right now changes.
fn for_each_codex_file<F>(files: &[PathBuf], cancel: Option<&AtomicBool>, mut fold: F)
where
    F: FnMut(&Path, &[CodexUsageRecord]),
{
    let range = unbounded_day_range();
    if !index_enabled() {
        for_each_parsed_file(
            files,
            cancel,
            |path| {
                JsonlScanner::parse_codex_file(path, &range, 0, None, None)
                    .map(|parsed| parsed.records)
                    .unwrap_or_default()
            },
            |path, records| fold(path, &records),
        );
        return;
    }

    for_each_indexed_file(
        files,
        &crate::usage_index::CODEX_INDEX,
        cancel,
        |index, path| -> IndexedFile<'_, CodexUsageRecord> {
            let Some(facts) = file_facts(path) else {
                return (FileRecords::Parsed(Vec::new()), IndexOutcome::Incomplete);
            };
            // A rollout is read whole or not at all, so its entry covers
            // whatever the file holds and no scan can out-reach it.
            match index.lookup(path, &facts, i64::MIN) {
                Lookup::Hit(records) => (FileRecords::Cached(records), IndexOutcome::Reused),
                Lookup::Append { .. } | Lookup::Miss => {
                    // A rollout is parsed whole or not at all, so there is no
                    // partial read to guard against. A read that failed is a
                    // different matter: storing it would record "this rollout
                    // has no usage" against a file that was simply busy.
                    let Ok(parsed) = JsonlScanner::parse_codex_file(path, &range, 0, None, None)
                    else {
                        return (FileRecords::Parsed(Vec::new()), IndexOutcome::Incomplete);
                    };
                    (
                        FileRecords::Parsed(parsed.records),
                        IndexOutcome::Store {
                            facts,
                            parsed_bytes: facts.len,
                            covers_from_ms: i64::MIN,
                        },
                    )
                }
            }
        },
        |path, records| fold(path, records),
    );
}

/// Parse already-deduped rollout paths and fold them into one summary.
fn parse_codex_files_into(
    files: &[PathBuf],
    range: &CostUsageDayRange,
    summary: &mut CostSummary,
    cancel: Option<&AtomicBool>,
) {
    for_each_codex_file(files, cancel, |_, records| {
        let (session_cost, has_tokens) = add_codex_records_to_summary(summary, records, range);
        if has_tokens {
            summary.total_cost_usd += session_cost;
            summary.sessions_count += 1;
        }
    });
}

/// Files handed to the pool before any of their results are folded, per worker.
///
/// Parsing is the expensive half of a scan and is embarrassingly parallel; the
/// fold has to stay sequential and in file order because cross-file
/// de-duplication depends on that order. Parsing *every* file before folding
/// any of them is what made a 90-day scan hold every record of gigabytes of
/// transcripts at once, so the work runs a batch at a time: enough files to
/// keep every worker busy, few enough that only one batch is live.
const PARSE_BATCH_PER_WORKER: usize = 4;

/// Threads to parse with. Capped at 8 because these scans run behind a UI on a
/// machine that is also running the agents that write these logs.
fn parse_workers(files: usize) -> usize {
    std::thread::available_parallelism()
        .map(usize::from)
        .unwrap_or(1)
        .clamp(1, 8)
        .min(files.max(1))
}

/// Parse `files` on a worker pool, handing each result to `fold` in file order.
///
/// `fold` runs on the calling thread, so callers keep one `seen` set, one
/// summary, and one deterministic order without any locking of their own.
fn for_each_parsed_file<T, P, F>(
    files: &[PathBuf],
    cancel: Option<&AtomicBool>,
    parse: P,
    mut fold: F,
) where
    P: Fn(&Path) -> T + Sync,
    T: Send,
    F: FnMut(&Path, T),
{
    if files.is_empty() {
        return;
    }
    let workers = parse_workers(files.len());
    let batch = workers.saturating_mul(PARSE_BATCH_PER_WORKER).max(1);
    for chunk in files.chunks(batch) {
        if is_cancelled(cancel) {
            return;
        }
        for (path, parsed) in parse_batch(chunk, workers, &parse, cancel) {
            fold(path, parsed);
        }
    }
}

/// Parse one batch concurrently and return the results in `chunk` order.
///
/// Workers pull the next index rather than taking a fixed slice each: transcript
/// sizes differ by orders of magnitude, and a fixed split leaves every other
/// worker idle behind the one that drew the largest file.
fn parse_batch<'a, T, P>(
    chunk: &'a [PathBuf],
    workers: usize,
    parse: &P,
    cancel: Option<&AtomicBool>,
) -> Vec<(&'a Path, T)>
where
    P: Fn(&Path) -> T + Sync,
    T: Send,
{
    if workers <= 1 || chunk.len() == 1 {
        return chunk
            .iter()
            .take_while(|_| !is_cancelled(cancel))
            .map(|path| (path.as_path(), parse(path)))
            .collect();
    }

    let next = AtomicUsize::new(0);
    let done: Mutex<Vec<(usize, T)>> = Mutex::new(Vec::with_capacity(chunk.len()));
    std::thread::scope(|scope| {
        for _ in 0..workers {
            scope.spawn(|| {
                loop {
                    if is_cancelled(cancel) {
                        break;
                    }
                    let index = next.fetch_add(1, Ordering::Relaxed);
                    let Some(path) = chunk.get(index) else {
                        break;
                    };
                    let parsed = parse(path);
                    // A poisoned lock means another worker panicked. Keep the
                    // results already collected rather than losing the batch.
                    done.lock()
                        .unwrap_or_else(|err| err.into_inner())
                        .push((index, parsed));
                }
            });
        }
    });

    let mut parsed = done.into_inner().unwrap_or_else(|err| err.into_inner());
    parsed.sort_by_key(|(index, _)| *index);
    parsed
        .into_iter()
        .map(|(index, value)| (chunk[index].as_path(), value))
        .collect()
}

/// A file's records, either borrowed from the index or just parsed.
enum FileRecords<'a, R> {
    Cached(&'a [R]),
    Parsed(Vec<R>),
}

impl<R> FileRecords<'_, R> {
    fn as_slice(&self) -> &[R] {
        match self {
            Self::Cached(records) => records,
            Self::Parsed(records) => records,
        }
    }
}

/// What the index should do with one file once a scan has read it.
enum IndexOutcome {
    /// Served from the index. Keep the entry alive.
    Reused,
    /// Read in full. Store it under these facts.
    Store {
        facts: FileFacts,
        parsed_bytes: u64,
        covers_from_ms: i64,
    },
    /// The read was cut short, so these records are only part of the file.
    ///
    /// Storing them would record the whole file's length and mtime against a
    /// truncated set of records, and the next scan would match on exactly those
    /// and serve the short version as a hit. Every card would then under-report
    /// that transcript until it was appended to or replaced.
    Incomplete,
}

/// One file's outcome: its records, and what to do with its index entry.
type IndexedFile<'a, R> = (FileRecords<'a, R>, IndexOutcome);

/// Classify a read that has just finished.
///
/// A cancelled scan is thrown away by its caller, so its records going nowhere
/// costs nothing. What must not happen is storing them: they would be indexed
/// against the whole file's length and mtime, and the next scan would match on
/// exactly those and serve the truncated set as a hit.
fn read_outcome(
    cancel: Option<&AtomicBool>,
    read: &StreamedRecords,
    facts: FileFacts,
    covers_from_ms: i64,
) -> IndexOutcome {
    if is_cancelled(cancel) || !read.reached_end {
        return IndexOutcome::Incomplete;
    }
    IndexOutcome::Store {
        facts,
        parsed_bytes: read.parsed_bytes,
        covers_from_ms,
    }
}

/// Whether scans consult the on-disk record index.
///
/// Off under `cfg(test)` unless a test opts in: the suite scans throwaway
/// fixture directories, and it must neither read nor write the developer's
/// real index to do it.
fn index_enabled() -> bool {
    #[cfg(test)]
    {
        TEST_INDEX_ENABLED.with(std::cell::Cell::get)
    }
    #[cfg(not(test))]
    {
        true
    }
}

#[cfg(test)]
thread_local! {
    static TEST_INDEX_ENABLED: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
    /// File counts handed to [`IndexStore::apply`] during one scan. A cold
    /// scan that still buffers every Store until the end records a single
    /// value equal to the corpus (SBS-951).
    static INDEX_APPLY_SIZES: std::cell::RefCell<Vec<usize>> =
        const { std::cell::RefCell::new(Vec::new()) };
}

#[cfg(test)]
fn note_index_apply(file_count: usize) {
    if file_count > 0 {
        INDEX_APPLY_SIZES.with(|sizes| sizes.borrow_mut().push(file_count));
    }
}

/// How many newly-parsed files may sit in RAM before they are handed to the
/// index. Matches one parse batch so the live set is the same size the
/// parser already keeps (SBS-951).
fn index_flush_file_limit(file_count: usize) -> usize {
    parse_workers(file_count)
        .saturating_mul(PARSE_BATCH_PER_WORKER)
        .max(1)
}

/// Stream files through the index in parse-sized batches, applying each
/// batch's Stores before the next parse starts.
///
/// A cold scan used to collect every `Store` into one `updates` vec and
/// `commit` after the last file. That put the whole corpus in RAM twice:
/// once in `updates`, once in the index. Applying at the same batch
/// boundary the parser already uses keeps only one batch of new records
/// live outside the index (SBS-951).
fn for_each_indexed_file<R, P, F>(
    files: &[PathBuf],
    store: &'static IndexStore<R>,
    cancel: Option<&AtomicBool>,
    parse: P,
    mut fold: F,
) where
    R: crate::usage_index::IndexedRecord,
    P: for<'idx> Fn(&'idx crate::usage_index::UsageIndex<R>, &Path) -> IndexedFile<'idx, R> + Sync,
    F: FnMut(&Path, &[R]),
{
    if files.is_empty() {
        return;
    }
    let workers = parse_workers(files.len());
    let batch = index_flush_file_limit(files.len());
    let mut touched = Vec::new();
    for chunk in files.chunks(batch) {
        if is_cancelled(cancel) {
            break;
        }
        let mut updates = Vec::new();
        {
            let index = store.read();
            for (path, (records, outcome)) in
                parse_batch(chunk, workers, &|path| parse(&index, path), cancel)
            {
                fold(path, records.as_slice());
                match (outcome, records) {
                    (
                        IndexOutcome::Store {
                            facts,
                            parsed_bytes,
                            covers_from_ms,
                        },
                        FileRecords::Parsed(records),
                    ) => updates.push(NewEntry {
                        path: path.to_path_buf(),
                        facts,
                        parsed_bytes,
                        covers_from_ms,
                        records,
                    }),
                    (IndexOutcome::Reused, _) => touched.push(path.to_path_buf()),
                    // Incomplete, or a shape that cannot occur: leave the
                    // index exactly as it was.
                    _ => {}
                }
            }
        }
        #[cfg(test)]
        note_index_apply(updates.len());
        store.apply(updates);
    }
    store.persist(&touched);
}

/// How far back a transcript is indexed.
///
/// A project transcript can be appended to for a year, and indexing all of it
/// costs real time on the pass that builds the index — for records no window in
/// this app asks about. The longest supported lookback is a 366-day custom
/// range on Estimated API value, so a year plus a month of slack covers every
/// window a card can request. A scan that asks for more (the CLI takes any
/// `--days`) sees the entry as a miss and reads the file in full, so the bound
/// costs time, never accuracy.
const INDEX_HORIZON_DAYS: i64 = 400;

/// The oldest instant newly indexed records are kept from.
fn index_horizon(now: DateTime<Utc>) -> DateTime<Utc> {
    now - chrono::Duration::days(INDEX_HORIZON_DAYS)
}

/// Stream each Claude transcript's records to `fold`, in file order.
///
/// Files already in the record index are served from it; a file that grew since
/// it was indexed is resumed from its last byte offset rather than re-read. The
/// caller still applies the window and the cross-file `seen` set, so what the
/// fold sees is exactly what a cold parse would have produced.
fn for_each_claude_file<F>(
    files: &[PathBuf],
    needs_from: &DateTime<Utc>,
    cancel: Option<&AtomicBool>,
    mut fold: F,
) where
    F: FnMut(&Path, &[ClaudeUsageRecord]),
{
    if !index_enabled() {
        for_each_parsed_file(
            files,
            cancel,
            |path| {
                let mut records = Vec::new();
                stream_claude_records(path, 0, cancel, |record| records.push(record));
                records
            },
            |path, records| fold(path, &records),
        );
        return;
    }

    // Never index less than the scan itself needs, or the entry it writes would
    // be a miss for the very scan that just built it.
    let horizon = index_horizon(Utc::now()).min(*needs_from);
    let horizon_ms = horizon.timestamp_millis();
    let keep = |record: &ClaudeUsageRecord| {
        // Undated records are kept regardless: the folds report them separately
        // and there is no timestamp to judge them by.
        record.timestamp.is_none_or(|at| at >= horizon)
    };

    for_each_indexed_file(
        files,
        &crate::usage_index::CLAUDE_INDEX,
        cancel,
        |index, path| -> IndexedFile<'_, ClaudeUsageRecord> {
            let Some(facts) = file_facts(path) else {
                return (FileRecords::Parsed(Vec::new()), IndexOutcome::Incomplete);
            };
            match index.lookup(path, &facts, needs_from.timestamp_millis()) {
                Lookup::Hit(records) => (FileRecords::Cached(records), IndexOutcome::Reused),
                Lookup::Append {
                    from,
                    prior,
                    covers_from_ms,
                } => {
                    let mut records = prior.to_vec();
                    let read = stream_claude_records(path, from, cancel, |record| {
                        if keep(&record) {
                            records.push(record);
                        }
                    });
                    // The kept prior records already reach further back than a
                    // fresh horizon would, so the entry keeps its own claim.
                    (
                        FileRecords::Parsed(records),
                        read_outcome(cancel, &read, facts, covers_from_ms),
                    )
                }
                Lookup::Miss => {
                    let mut records = Vec::new();
                    let read = stream_claude_records(path, 0, cancel, |record| {
                        if keep(&record) {
                            records.push(record);
                        }
                    });
                    (
                        FileRecords::Parsed(records),
                        read_outcome(cancel, &read, facts, horizon_ms),
                    )
                }
            }
        },
        |path, records| fold(path, records),
    );
}

/// Stream every usage record in one transcript file, starting at byte
/// `from`, and report the offset reading stopped at.
///
/// No filtering happens here: the caller decides what is in window and what is
/// a duplicate. That split is what lets the record index store a file's records
/// once and serve scans with different windows from the same entry.
///
/// The offset is only meaningful because Claude appends whole JSON lines and
/// never rewrites them, so resuming there picks up exactly where this stopped.
struct StreamedRecords {
    /// Byte offset the read stopped at.
    parsed_bytes: u64,
    /// Whether the read got to the end of the file under its own steam.
    ///
    /// False when the file could not be opened, sought, or read. A read that
    /// failed holds part of a transcript, and storing that part against the
    /// whole file's size and mtime would serve the short version forever: an
    /// antivirus or sync client holding a file open for a moment would cost
    /// that transcript from every total until it was rewritten.
    reached_end: bool,
}

fn stream_claude_records<F>(
    path: &Path,
    from: u64,
    cancel: Option<&AtomicBool>,
    mut on_record: F,
) -> StreamedRecords
where
    F: FnMut(ClaudeUsageRecord),
{
    let Ok(file) = File::open(path) else {
        return StreamedRecords {
            parsed_bytes: from,
            reached_end: false,
        };
    };
    let mut reader = BufReader::new(file);
    if from > 0 && reader.seek(SeekFrom::Start(from)).is_err() {
        return StreamedRecords {
            parsed_bytes: from,
            reached_end: false,
        };
    }

    let mut at = from;
    let mut reached_end = true;
    let mut line = Vec::with_capacity(16 * 1024);
    loop {
        line.clear();
        let Ok(bytes_read) = reader.read_until(b'\n', &mut line) else {
            // A read that errored part way holds an unknown remainder.
            reached_end = false;
            break;
        };
        if bytes_read == 0 {
            break;
        }
        // A trailing line with no newline is a half-written record. Stop before
        // it and leave the offset there, so the next scan reads it whole.
        if !line.ends_with(b"\n") {
            break;
        }
        // Cancel before claiming the bytes. A line counted as parsed but never
        // handed to `on_record` is lost to every later resume.
        if is_cancelled(cancel) {
            break;
        }
        at += bytes_read as u64;
        // Claude transcripts contain large user/tool payloads that can never
        // contribute token usage. Avoid allocating a String and asking serde
        // to parse those lines; only assistant events with a usage object can
        // produce a record.
        if !contains_bytes(&line, b"\"assistant\"") || !contains_bytes(&line, b"\"usage\"") {
            continue;
        }
        if let Ok(event) = serde_json::from_slice::<ClaudeEvent>(&line)
            && let Some(record) = claude_usage_record_from_event(&event)
        {
            on_record(record);
        }
    }
    StreamedRecords {
        parsed_bytes: at,
        reached_end,
    }
}

/// Stream the de-duplicated, in-window usage records from one transcript
/// file into `on_record`. Both the summary scan and the daily-history scan
/// consume this single reader, so Claude log semantics live in one place.
/// Returns the number of records consumed, so callers can tell whether the
/// file contributed anything.
///
/// Records are handed over by value: the collecting caller keeps them, and the
/// summing callers borrow what they are given. Passing a reference meant every
/// kept record was cloned, strings and all, on the hottest path in the scan.
fn for_each_claude_usage_record<F>(
    path: &Path,
    cutoff: &DateTime<Utc>,
    seen: &mut HashSet<String>,
    cancel: Option<&AtomicBool>,
    mut on_record: F,
) -> usize
where
    F: FnMut(ClaudeUsageRecord),
{
    let mut counted = 0;
    stream_claude_records(path, 0, cancel, |record| {
        if should_count_claude_record(&record, cutoff, seen) {
            counted += 1;
            on_record(record);
        }
    });
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
    // Price with the local calendar day so date-aware rates match the same
    // window the scanner uses for inclusion (not the UTC date alone).
    let usage_date = timestamp
        .map(|recorded_at| recorded_at.with_timezone(&Local).date_naive())
        .unwrap_or_else(|| Local::now().date_naive());
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

/// The part of folding a record that does not depend on which summary it lands
/// in.
///
/// One record is folded into a dozen summaries — the file, every caller window,
/// its day, its hour — and all of this used to be redone for each: a pricing
/// lookup that reads the clock and normalizes the model name, plus a fresh
/// allocation of the project bucket. Over a real 60-day scan that is millions
/// of repeats of work whose answer never changes.
struct PreparedClaudeRecord<'a> {
    record: &'a ClaudeUsageRecord,
    project: String,
    unknown_model: bool,
}

fn prepare_claude_record(record: &ClaudeUsageRecord) -> PreparedClaudeRecord<'_> {
    PreparedClaudeRecord {
        record,
        project: crate::codex_costs::project_bucket(record.project.as_deref()),
        unknown_model: CostUsagePricing::claude_cost_usd(&record.model, 0, 0, 0, 0).is_none(),
    }
}

/// Add `cost` under `key`.
///
/// `HashMap::entry` takes an owned key, so it allocates on every call, hit or
/// miss. These maps hold a handful of models and projects and are written to
/// millions of times, so the string is only built when the key is genuinely new.
fn add_keyed_cost(map: &mut HashMap<String, f64>, key: &str, cost: f64) {
    match map.get_mut(key) {
        Some(total) => *total += cost,
        None => {
            map.insert(key.to_string(), cost);
        }
    }
}

fn add_keyed_claude_tokens(
    map: &mut HashMap<String, ModelTokenCounts>,
    key: &str,
    record: &ClaudeUsageRecord,
) {
    let counts = match map.get_mut(key) {
        Some(counts) => counts,
        None => map.entry(key.to_string()).or_default(),
    };
    counts.input_tokens += record.input;
    counts.output_tokens += record.output;
    counts.cached_tokens += record.cache_create + record.cache_read;
    counts.cache_read_tokens += record.cache_read;
    counts.cache_write_tokens += record.cache_create;
    counts.calls += 1;
}

fn add_prepared_claude_record(summary: &mut CostSummary, prepared: &PreparedClaudeRecord<'_>) {
    let record = prepared.record;
    if prepared.unknown_model && !summary.unknown_models.contains(&record.model) {
        summary.unknown_models.insert(record.model.clone());
    }

    summary.input_tokens += record.input;
    summary.output_tokens += record.output;
    summary.cached_tokens += record.cache_create + record.cache_read;
    summary.cache_read_tokens += record.cache_read;
    summary.cache_write_tokens += record.cache_create;
    summary.total_cost_usd += record.cost;

    add_keyed_cost(&mut summary.by_model, &record.model, record.cost);
    add_keyed_claude_tokens(&mut summary.by_model_tokens, &record.model, record);
    add_keyed_cost(&mut summary.by_project, &prepared.project, record.cost);
    add_keyed_claude_tokens(&mut summary.by_project_tokens, &prepared.project, record);
}

fn add_claude_record_to_summary(summary: &mut CostSummary, record: &ClaudeUsageRecord) {
    add_prepared_claude_record(summary, &prepare_claude_record(record));
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
        || scanner
            .get_claude_projects_dirs()
            .iter()
            .any(|dir| dir.exists())
        || grok_sessions_dir(None).is_some_and(|dir| dir.exists())
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

/// As [`get_cost_usage_report`], but also fills [`CostUsageReport::hourly_activity`].
///
/// Separate entry point because bucketing every record by clock-hour runs its
/// accounting a second time. Only the activity heatmap needs it; the charts,
/// reset windows, and API-value card all scan the same trees on every refresh
/// and would pay for buckets they never read.
pub fn get_cost_usage_report_hourly(provider: &str, days: u32) -> Option<CostUsageReport> {
    let days = days.max(1);
    let scanner = CostScanner::new(days).with_hourly_activity();
    match provider {
        "codex" => Some(scan_codex_report(&scanner, days, &[])),
        "claude" => Some(scan_claude_report(&scanner, days, &[])),
        "grok" => Some(scan_grok_report(&scanner, days, &[])),
        _ => None,
    }
}

pub fn get_cost_usage_report_with_windows(
    provider: &str,
    days: u32,
    current_windows: &[CurrentUsageWindow],
) -> Option<CostUsageReport> {
    get_cost_usage_report_scoped(provider, days, current_windows, None)
}

/// As [`get_cost_usage_report_with_windows`], but scoped to one account's config
/// directory when `scoped_home` is given, so its charts reflect only its own
/// logs rather than every account's on the machine.
pub fn get_cost_usage_report_scoped(
    provider: &str,
    days: u32,
    current_windows: &[CurrentUsageWindow],
    scoped_home: Option<PathBuf>,
) -> Option<CostUsageReport> {
    let days = days.max(1);
    let scanner = match scoped_home {
        Some(home) => CostScanner::scoped_to(days, home),
        None => CostScanner::new(days),
    };
    match provider {
        "codex" => Some(scan_codex_report(&scanner, days, current_windows)),
        "claude" => Some(scan_claude_report(&scanner, days, current_windows)),
        "grok" => Some(scan_grok_report(&scanner, days, current_windows)),
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

/// Local clock-hour buckets, keyed by `(local date, hour of day)`.
type HourlySummaries = HashMap<(NaiveDate, u32), CostSummary>;

/// Credit one record to its local clock-hour bucket.
///
/// Mirrors [`add_to_current_windows`], but buckets are created on demand: an
/// idle machine keeps an empty map instead of 720 zeroed summaries. Callers
/// must only reach this once they know the record's day is in range, so the
/// hourly series never drifts from `daily_costs`.
fn add_to_hourly<F>(
    enabled: bool,
    hourly: &mut HourlySummaries,
    timestamp: Option<DateTime<Utc>>,
    mut add: F,
) where
    F: FnMut(&mut CostSummary),
{
    if !enabled {
        return;
    }
    let Some(local) = timestamp.map(|value| value.with_timezone(&Local)) else {
        return;
    };
    add(hourly
        .entry((local.date_naive(), local.hour()))
        .or_default());
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
    target.reasoning_tokens += source.reasoning_tokens;
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
    for (plan, tokens) in &source.by_plan_tokens {
        target
            .by_plan_tokens
            .entry(plan.clone())
            .or_default()
            .merge_from(tokens);
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
    hourly: HourlySummaries,
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

    let mut hourly_activity: Vec<HourlyActivityPoint> = hourly
        .into_iter()
        .map(|((date, hour), summary)| HourlyActivityPoint {
            date,
            hour,
            summary,
        })
        .collect();
    hourly_activity.sort_by_key(|point| (point.date, point.hour));

    CostUsageReport {
        daily_costs,
        today: today_summary,
        seven_days: seven_day_summary,
        thirty_days: period_summary,
        latest_session,
        current_windows,
        hourly_activity,
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
    hourly: HourlySummaries,
    collect_hourly: bool,
    current_windows: HashMap<String, CostSummary>,
    latest: Option<(std::time::SystemTime, CostSummary)>,
    today_sessions: u32,
    seven_day_sessions: u32,
    period_sessions: u32,
}

impl<'a> CodexReportRollups<'a> {
    fn new(
        days: u32,
        range: &'a CostUsageDayRange,
        windows: &'a [CurrentUsageWindow],
        today: NaiveDate,
        collect_hourly: bool,
    ) -> Self {
        Self {
            range,
            windows,
            today,
            seven_day_start: today - Duration::days(6),
            daily: empty_daily_summaries(days),
            hourly: HourlySummaries::new(),
            collect_hourly,
            current_windows: empty_current_window_summaries(windows),
            latest: None,
            today_sessions: 0,
            seven_day_sessions: 0,
            period_sessions: 0,
        }
    }

    /// Fold one already-parsed rollout in. Callers dedup by rollout file name
    /// while collecting the paths, so the same rollout reaching this twice is
    /// a caller bug rather than something to filter here.
    fn ingest_parsed(&mut self, path: &Path, records: &[CodexUsageRecord]) {
        let mut file_summary = CostSummary::default();
        let mut contributed_today = false;
        let mut contributed_seven_days = false;
        for record in records.iter().filter(|record| {
            CostUsageDayRange::is_in_range(
                &record.day_key,
                &self.range.since_key,
                &self.range.until_key,
            )
        }) {
            // Always credit caller windows first. A missing daily bucket must not
            // drop custom/reset-window totals (that is what broke Estimated API
            // value custom ranges when a day key was absent from the map).
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
            let Some(day_summary) = self.daily.get_mut(&record.day_key) else {
                continue;
            };
            if let Some(cost) = add_codex_record_to_summary(day_summary, record) {
                day_summary.total_cost_usd += cost;
            }
            add_to_hourly(
                self.collect_hourly,
                &mut self.hourly,
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
            self.hourly,
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
    let mut rollups = CodexReportRollups::new(days, &range, windows, today, scanner.collect_hourly);

    // Collect every rollout first, then read them concurrently. One rollout can
    // appear in the sessions tree, the archive, and across homes, so the dedup
    // set spans both passes.
    let mut seen = HashSet::new();
    let mut files = Vec::new();
    for sessions_dir in scanner.get_codex_sessions_dirs() {
        files.extend(codex_session_files(&sessions_dir, &range, &mut seen, None));
    }
    // The archive is a flat dir, so gate on the rollout's filename date rather
    // than walking a date tree that does not exist there.
    for archived_dir in scanner.get_codex_archived_dirs() {
        files.extend(codex_archived_files(&archived_dir, &range, &mut seen, None));
    }

    for_each_codex_file(&files, None, |path, records| {
        rollups.ingest_parsed(path, records);
    });

    let mut report = rollups.finish(days);
    // Apply ccusage-compatible priority/fast multiplier to every Codex window.
    codex_cost_speed::apply_speed_to_summary(&mut report.today, scanner.codex_speed);
    codex_cost_speed::apply_speed_to_summary(&mut report.seven_days, scanner.codex_speed);
    codex_cost_speed::apply_speed_to_summary(&mut report.thirty_days, scanner.codex_speed);
    if let Some(latest) = report.latest_session.as_mut() {
        codex_cost_speed::apply_speed_to_summary(latest, scanner.codex_speed);
    }
    for summary in report.current_windows.values_mut() {
        codex_cost_speed::apply_speed_to_summary(summary, scanner.codex_speed);
    }
    for point in report.hourly_activity.iter_mut() {
        codex_cost_speed::apply_speed_to_summary(&mut point.summary, scanner.codex_speed);
    }
    for (_day, cost) in report.daily_costs.iter_mut() {
        *cost *= scanner.codex_speed.multiplier();
    }
    report
}

fn add_grok_record_to_summary(summary: &mut CostSummary, record: &GrokUsageRecord) {
    // Prefer Grok's logged costUsdTicks (API-equivalent $). Partial / bare
    // fallback rows have no ticks and stay unpriced — never invent a rate.
    if record.partial {
        tracing::trace!(
            model = %record.model,
            project = ?record.project,
            "Grok local usage row is partial (fallback telemetry)"
        );
    }

    summary.input_tokens += record.input;
    summary.output_tokens += record.output;
    summary.cached_tokens += record.cache_read;
    summary.cache_read_tokens += record.cache_read;
    summary.reasoning_tokens += record.reasoning;

    // modelCalls when present; else one call per usage row with tokens so
    // cost-per-call averages stay defined for older logs.
    let calls = if record.model_calls > 0 {
        record.model_calls
    } else if record.input > 0 || record.output > 0 || record.cache_read > 0 || record.reasoning > 0
    {
        1
    } else {
        0
    };

    let model_tokens = summary
        .by_model_tokens
        .entry(record.model.clone())
        .or_default();
    model_tokens.input_tokens += record.input;
    model_tokens.output_tokens += record.output;
    model_tokens.cached_tokens += record.cache_read;
    model_tokens.cache_read_tokens += record.cache_read;
    model_tokens.calls += calls;

    let effort = match record
        .effort
        .as_deref()
        .map(str::trim)
        .filter(|e| !e.is_empty())
    {
        Some(effort) => effort.to_ascii_lowercase(),
        None => "unknown".to_string(),
    };
    let effort_tokens = summary.by_effort_tokens.entry(effort.clone()).or_default();
    effort_tokens.input_tokens += record.input;
    effort_tokens.output_tokens += record.output;
    effort_tokens.cached_tokens += record.cache_read;
    effort_tokens.cache_read_tokens += record.cache_read;
    effort_tokens.calls += calls;

    let project = project_bucket(record.project.as_deref());
    let project_tokens = summary
        .by_project_tokens
        .entry(project.clone())
        .or_default();
    project_tokens.input_tokens += record.input;
    project_tokens.output_tokens += record.output;
    project_tokens.cached_tokens += record.cache_read;
    project_tokens.cache_read_tokens += record.cache_read;
    project_tokens.calls += calls;

    if let Some(cost) = record.cost_usd.filter(|c| *c > 0.0) {
        summary.total_cost_usd += cost;
        *summary.by_model.entry(record.model.clone()).or_insert(0.0) += cost;
        *summary.by_effort.entry(effort).or_insert(0.0) += cost;
        *summary.by_project.entry(project).or_insert(0.0) += cost;
        // A later priced row for the same model clears any earlier unpriced flag
        // so coverage does not treat the whole model as unpriced when ticks exist.
        summary.unknown_models.remove(&record.model);
    } else if !summary.by_model.contains_key(&record.model) {
        // No ticks (or partial fallback): tokens still count toward coverage.
        // Skip if this model already has logged dollars from another row.
        summary.unknown_models.insert(record.model.clone());
    }
}

fn scan_grok_report(
    scanner: &CostScanner,
    days: u32,
    windows: &[CurrentUsageWindow],
) -> CostUsageReport {
    let mut daily = empty_daily_summaries(days);
    let mut hourly = HourlySummaries::new();
    let mut current_windows = empty_current_window_summaries(windows);
    // Honour an injected home so tests need not export `GROK_HOME`.
    let ambient = scanner.ambient_home_override.clone();
    let Some(sessions_root) = grok_sessions_dir(ambient.as_deref()) else {
        return finish_report(daily, hourly, days, None, (0, 0, 0), None, current_windows);
    };
    if !sessions_root.exists() {
        return finish_report(daily, hourly, days, None, (0, 0, 0), None, current_windows);
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

    for session_dir in discover_grok_session_dirs(&sessions_root) {
        let meta = load_session_meta(&session_dir);
        let updates = session_dir.join("updates.jsonl");
        let records = parse_grok_updates_file(&updates, &meta, cutoff);
        if records.is_empty() {
            continue;
        }

        let mut file_summary = CostSummary::default();
        let mut latest_recorded_at: Option<DateTime<Utc>> = None;
        let mut contributed_today = false;
        let mut contributed_seven_days = false;
        let mut counted = 0u32;

        for record in &records {
            if !should_count_grok_record(record, cutoff, &mut seen) {
                continue;
            }
            counted += 1;
            add_grok_record_to_summary(&mut file_summary, record);
            add_to_current_windows(&mut current_windows, windows, record.timestamp, |summary| {
                add_grok_record_to_summary(summary, record)
            });
            if let Some(timestamp) = record.timestamp {
                let date = timestamp.with_timezone(&Local).date_naive();
                let day = date.format("%Y-%m-%d").to_string();
                if let Some(day_summary) = daily.get_mut(&day) {
                    add_grok_record_to_summary(day_summary, record);
                    add_to_hourly(
                        scanner.collect_hourly,
                        &mut hourly,
                        record.timestamp,
                        |summary| add_grok_record_to_summary(summary, record),
                    );
                }
                contributed_today |= date == today;
                contributed_seven_days |= date >= seven_day_start;
                if latest_recorded_at.is_none_or(|seen_at| timestamp > seen_at) {
                    latest_recorded_at = Some(timestamp);
                }
            } else {
                add_grok_record_to_summary(&mut undated, record);
            }
        }

        if counted == 0 {
            continue;
        }
        file_summary.sessions_count = 1;
        period_sessions += 1;
        today_sessions += u32::from(contributed_today);
        seven_day_sessions += u32::from(contributed_seven_days);

        let fallback_modified = fs::metadata(&updates)
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
        hourly,
        days,
        latest.map(|(_, summary)| summary),
        (today_sessions, seven_day_sessions, period_sessions),
        Some(&undated),
        current_windows,
    )
}

fn scan_claude_report(
    scanner: &CostScanner,
    days: u32,
    windows: &[CurrentUsageWindow],
) -> CostUsageReport {
    let projects_dirs = scanner.get_claude_projects_dirs();
    let mut daily = empty_daily_summaries(days);
    let mut hourly = HourlySummaries::new();
    let mut current_windows = empty_current_window_summaries(windows);

    let today = Local::now().date_naive();
    let seven_day_start = today - Duration::days(6);
    // Inclusive local calendar window (matches CLI cost + Codex + ccusage).
    let period_start = codex_period_start(today, days);
    let cutoff = local_day_start_utc(period_start);

    // Union files across ambient + every configured Claude account home.
    let mut files = Vec::new();
    for projects_dir in projects_dirs.iter().filter(|dir| dir.exists()) {
        files.extend(scanner.claude_files_since(projects_dir, &cutoff, None));
    }
    files.sort();
    files.dedup();
    if files.is_empty() {
        return finish_report(daily, hourly, days, None, (0, 0, 0), None, current_windows);
    }

    let mut seen = HashSet::new();
    let mut undated = CostSummary::default();
    let mut latest: Option<(DateTime<Utc>, CostSummary)> = None;
    let mut today_sessions = 0;
    let mut seven_day_sessions = 0;
    let mut period_sessions = 0;

    for_each_claude_file(&files, &cutoff, None, |path, records| {
        let mut file_summary = CostSummary::default();
        let mut latest_recorded_at: Option<DateTime<Utc>> = None;
        let mut contributed_today = false;
        let mut contributed_seven_days = false;
        let mut counted = 0;
        for record in records {
            if !should_count_claude_record(record, &cutoff, &mut seen) {
                continue;
            }
            counted += 1;
            // Prepared once, then folded into the file, the windows, the day,
            // and the hour.
            let prepared = prepare_claude_record(record);
            add_prepared_claude_record(&mut file_summary, &prepared);
            add_to_current_windows(&mut current_windows, windows, record.timestamp, |summary| {
                add_prepared_claude_record(summary, &prepared)
            });
            if let Some(timestamp) = record.timestamp {
                let date = timestamp.with_timezone(&Local).date_naive();
                let day = date.format("%Y-%m-%d").to_string();
                if let Some(day_summary) = daily.get_mut(&day) {
                    add_prepared_claude_record(day_summary, &prepared);
                    add_to_hourly(
                        scanner.collect_hourly,
                        &mut hourly,
                        record.timestamp,
                        |summary| add_prepared_claude_record(summary, &prepared),
                    );
                }
                contributed_today |= date == today;
                contributed_seven_days |= date >= seven_day_start;
                if latest_recorded_at.is_none_or(|seen_at| timestamp > seen_at) {
                    latest_recorded_at = Some(timestamp);
                }
            } else {
                add_prepared_claude_record(&mut undated, &prepared);
            }
        }
        if counted == 0 {
            return;
        }
        file_summary.sessions_count = 1;
        period_sessions += 1;
        today_sessions += u32::from(contributed_today);
        seven_day_sessions += u32::from(contributed_seven_days);
        let fallback_modified = fs::metadata(path)
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
    });

    finish_report(
        daily,
        hourly,
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
            // Scan Codex logs by day across ambient, multi-account, custom, and
            // WSL session roots. Dedup by rollout filename so a shared copy
            // across homes cannot inflate the day total.
            let sessions_dirs = scanner.get_codex_sessions_dirs();
            let speed_mult = scanner.codex_speed.multiplier();
            for days_ago in 0..days {
                let date = today - Duration::days(days_ago as i64);
                let date_str = date.format("%Y-%m-%d").to_string();
                let range = CostUsageDayRange::new(date, date);
                let mut day_cost = 0.0;
                // Fresh per day: padded neighbor folders may re-list a rollout
                // when computing an adjacent day, and each day range must parse
                // it for its own window. Within a day, multi-home copies share
                // one filename and must count once.
                let mut seen = HashSet::new();

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
                                if path.extension().is_some_and(|e| e == "jsonl")
                                    && mark_unseen_rollout(&path, &mut seen)
                                {
                                    day_cost += scan_codex_file_cost_for_range(&path, &range);
                                }
                            }
                        }
                    }
                }
                daily_costs.insert(date_str, day_cost * speed_mult);
            }
        }
        "claude" => {
            // Real per-day breakdown: walk every projects root once,
            // de-duplicating records across files and account homes.
            let period_start = codex_period_start(today, days);
            let cutoff = local_day_start_utc(period_start);
            let mut seen = HashSet::new();
            let mut handle_file = |path: &Path| {
                for_each_claude_usage_record(path, &cutoff, &mut seen, None, |record| {
                    add_claude_record_to_daily_costs(&mut daily_costs, &record);
                });
            };
            for projects_dir in scanner
                .get_claude_projects_dirs()
                .iter()
                .filter(|dir| dir.exists())
            {
                scanner.walk_claude_files(projects_dir, &cutoff, None, &mut handle_file);
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
    use crate::core::ProviderId;
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

    /// SOU-296: the report scanner behind the Charts page, the reset windows,
    /// and the API value card only walked `sessions/`. Archiving a Codex task
    /// therefore shrank every total, while the summary scanner still counted
    /// it. The archived rollout must be included, and a rollout present in both
    /// places must still count once.
    /// Two accounts live in two config directories. A scoped scan must read only
    /// its own directory, or both accounts' charts show the same machine-wide
    /// totals (the reported bug: identical stats on both Codex tabs).
    #[test]
    fn a_scoped_scan_reads_only_its_own_account_directory() {
        let today = Local::now().date_naive();
        let day = today.format("%Y-%m-%d").to_string();
        let ts = format!("{day}T10:00:00.000Z");
        let line = |input: u32| {
            format!(
                r#"{{"timestamp":"{ts}","type":"event_msg","payload":{{"type":"token_count","info":{{"last_token_usage":{{"input_tokens":{input},"cached_input_tokens":0,"output_tokens":0}}}}}}}}"#
            )
        };

        fn day_dir(home: &Path, today: chrono::NaiveDate) -> PathBuf {
            home.join("sessions")
                .join(today.format("%Y").to_string())
                .join(today.format("%m").to_string())
                .join(today.format("%d").to_string())
        }

        // Personal home: one rollout of 1000 input tokens.
        let personal = tempfile::tempdir().unwrap();
        let pd = day_dir(personal.path(), today);
        std::fs::create_dir_all(&pd).unwrap();
        std::fs::write(
            pd.join(format!(
                "rollout-{day}-11111111-1111-1111-1111-111111111111.jsonl"
            )),
            line(1000),
        )
        .unwrap();

        // Work home: one rollout of 7000 input tokens.
        let work = tempfile::tempdir().unwrap();
        let wd = day_dir(work.path(), today);
        std::fs::create_dir_all(&wd).unwrap();
        std::fs::write(
            wd.join(format!(
                "rollout-{day}-22222222-2222-2222-2222-222222222222.jsonl"
            )),
            line(7000),
        )
        .unwrap();

        let personal_report = scan_codex_report(
            &CostScanner::scoped_to(2, personal.path().to_path_buf()),
            2,
            &[],
        );
        let work_report = scan_codex_report(
            &CostScanner::scoped_to(2, work.path().to_path_buf()),
            2,
            &[],
        );

        // Each account sees only its own logs, not the other's, and not the sum.
        assert_eq!(personal_report.thirty_days.input_tokens, 1000);
        assert_eq!(work_report.thirty_days.input_tokens, 7000);
        assert_eq!(personal_report.thirty_days.sessions_count, 1);
        assert_eq!(work_report.thirty_days.sessions_count, 1);
    }

    /// Estimated API value / unscoped cost totals must sum every configured
    /// multi-account home. Capacity accounts alone used to be invisible to
    /// the machine-wide scanner (only ambient CODEX_HOME + custom session
    /// dirs were walked).
    #[test]
    fn unscoped_scan_sums_configured_account_homes() {
        let today = Local::now().date_naive();
        let day = today.format("%Y-%m-%d").to_string();
        let ts = format!("{day}T10:00:00.000Z");
        let line = |input: u32| {
            format!(
                r#"{{"timestamp":"{ts}","type":"event_msg","payload":{{"type":"token_count","info":{{"last_token_usage":{{"input_tokens":{input},"cached_input_tokens":0,"output_tokens":0}}}}}}}}"#
            )
        };

        fn day_dir(home: &Path, today: chrono::NaiveDate) -> PathBuf {
            home.join("sessions")
                .join(today.format("%Y").to_string())
                .join(today.format("%m").to_string())
                .join(today.format("%d").to_string())
        }

        let personal = tempfile::tempdir().unwrap();
        let work = tempfile::tempdir().unwrap();
        let pd = day_dir(personal.path(), today);
        let wd = day_dir(work.path(), today);
        std::fs::create_dir_all(&pd).unwrap();
        std::fs::create_dir_all(&wd).unwrap();
        std::fs::write(
            pd.join(format!(
                "rollout-{day}-11111111-1111-1111-1111-111111111111.jsonl"
            )),
            line(1000),
        )
        .unwrap();
        std::fs::write(
            wd.join(format!(
                "rollout-{day}-22222222-2222-2222-2222-222222222222.jsonl"
            )),
            line(7000),
        )
        .unwrap();

        // Ambient home is personal; work is only present as a configured account.
        // Injected rather than exported: mutating process env races the readers
        // in every other test module.
        let ambient = personal.path().to_path_buf();

        // Without account homes, only ambient (personal) is scanned.
        let ambient_only = scan_codex_report(
            &CostScanner::with_ambient_and_account_homes(2, ambient.clone(), Vec::new()),
            2,
            &[],
        );
        assert_eq!(ambient_only.thirty_days.input_tokens, 1000);

        // With the work account configured, totals include both homes.
        let both = scan_codex_report(
            &CostScanner::with_ambient_and_account_homes(
                2,
                ambient.clone(),
                vec![work.path().to_path_buf()],
            ),
            2,
            &[],
        );
        assert_eq!(
            both.thirty_days.input_tokens, 8000,
            "personal 1000 + work 7000 must both count toward unscoped totals"
        );
        assert_eq!(both.thirty_days.sessions_count, 2);

        // Listing the ambient home again as an "account" must not double-count.
        let with_dup_home = scan_codex_report(
            &CostScanner::with_ambient_and_account_homes(
                2,
                ambient,
                vec![personal.path().to_path_buf(), work.path().to_path_buf()],
            ),
            2,
            &[],
        );
        assert_eq!(
            with_dup_home.thirty_days.input_tokens, 8000,
            "same config dir listed as ambient and account must not inflate"
        );
    }

    /// A rollout copied into two homes still counts once (filename dedup).
    #[test]
    fn unscoped_scan_dedups_same_rollout_across_account_homes() {
        let today = Local::now().date_naive();
        let day = today.format("%Y-%m-%d").to_string();
        let ts = format!("{day}T10:00:00.000Z");
        let token_line = format!(
            r#"{{"timestamp":"{ts}","type":"event_msg","payload":{{"type":"token_count","info":{{"last_token_usage":{{"input_tokens":1000,"cached_input_tokens":0,"output_tokens":0}}}}}}}}"#
        );
        let shared = format!("rollout-{day}-aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaaa.jsonl");

        fn day_dir(home: &Path, today: chrono::NaiveDate) -> PathBuf {
            home.join("sessions")
                .join(today.format("%Y").to_string())
                .join(today.format("%m").to_string())
                .join(today.format("%d").to_string())
        }

        let personal = tempfile::tempdir().unwrap();
        let work = tempfile::tempdir().unwrap();
        let pd = day_dir(personal.path(), today);
        let wd = day_dir(work.path(), today);
        std::fs::create_dir_all(&pd).unwrap();
        std::fs::create_dir_all(&wd).unwrap();
        std::fs::write(pd.join(&shared), &token_line).unwrap();
        std::fs::write(wd.join(&shared), &token_line).unwrap();

        let report = scan_codex_report(
            &CostScanner::with_ambient_and_account_homes(
                2,
                personal.path().to_path_buf(),
                vec![work.path().to_path_buf()],
            ),
            2,
            &[],
        );
        assert_eq!(
            report.thirty_days.input_tokens, 1000,
            "identical rollout name across homes must count once"
        );
        assert_eq!(report.thirty_days.sessions_count, 1);
    }

    #[test]
    fn codex_report_counts_archived_rollouts_exactly_once() {
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

        let report = scan_codex_report(
            &CostScanner::with_ambient_and_account_homes(2, tmp.path().to_path_buf(), Vec::new()),
            2,
            &[],
        );

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
    fn codex_report_applies_cost_speed_to_reset_windows() {
        // Weekly/reset windows must use the same speed tier as calendar totals.
        // A post-pass that only multiplies today/7d/30d left Weekly at standard
        // while the API-value ring (fresh scan) showed priority/fast 2x.
        let tmp = tempfile::tempdir().unwrap();
        let event_at = Utc::now() - Duration::minutes(10);
        let local_day = event_at.with_timezone(&Local).date_naive();
        let day_dir = tmp
            .path()
            .join("sessions")
            .join(local_day.format("%Y").to_string())
            .join(local_day.format("%m").to_string())
            .join(local_day.format("%d").to_string());
        std::fs::create_dir_all(&day_dir).unwrap();
        let ts = event_at.format("%Y-%m-%dT%H:%M:%S%.3fZ").to_string();
        let line = format!(
            r#"{{"timestamp":"{ts}","type":"event_msg","payload":{{"type":"token_count","info":{{"model":"gpt-5.6-sol","total_token_usage":{{"input_tokens":1000,"cached_input_tokens":400,"output_tokens":100}}}}}}}}"#
        );
        std::fs::write(
            day_dir.join(format!(
                "rollout-{}-aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaaa.jsonl",
                local_day.format("%Y-%m-%d")
            )),
            line,
        )
        .unwrap();

        let starts_at = Utc::now() - Duration::hours(1);
        let ends_at = Utc::now() + Duration::hours(1);
        let windows = [CurrentUsageWindow {
            id: "primary".into(),
            starts_at,
            ends_at,
        }];

        let ambient = tmp.path().to_path_buf();
        let standard =
            CostScanner::with_codex_speed(2, Some("standard")).with_ambient_home(ambient.clone());
        let fast = CostScanner::with_codex_speed(2, Some("fast")).with_ambient_home(ambient);
        let std_report = scan_codex_report(&standard, 2, &windows);
        let fast_report = scan_codex_report(&fast, 2, &windows);

        let std_window = std_report
            .current_windows
            .get("primary")
            .expect("primary window");
        let fast_window = fast_report
            .current_windows
            .get("primary")
            .expect("primary window");
        assert!(
            std_window.total_cost_usd > 0.0,
            "window should price sol usage"
        );
        assert!(
            (fast_window.total_cost_usd - std_window.total_cost_usd * 2.0).abs() < 1e-9,
            "fast window must be 2x standard (got fast={} std={})",
            fast_window.total_cost_usd,
            std_window.total_cost_usd
        );
        assert_eq!(fast_window.codex_cost_speed.as_deref(), Some("fast"));
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

        let today = Local::now().date_naive();
        let range = CostUsageDayRange::new(codex_period_start(today, 30), today);
        let mut summary = CostSummary::default();
        parse_codex_files_into(std::slice::from_ref(&path), &range, &mut summary, None);

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
    fn unscoped_claude_scan_sums_account_homes_and_dedups_records() {
        let personal = tempfile::tempdir().expect("personal home");
        let work = tempfile::tempdir().expect("work home");
        let personal_projects = personal.path().join("projects").join("personal");
        let work_projects = work.path().join("projects").join("work");
        std::fs::create_dir_all(&personal_projects).expect("personal projects");
        std::fs::create_dir_all(&work_projects).expect("work projects");

        let timestamp = Utc::now().to_rfc3339();
        let shared = format!(
            r#"{{"type":"assistant","timestamp":"{timestamp}","requestId":"req_shared","message":{{"id":"msg_shared","model":"claude-sonnet-4-6","usage":{{"input_tokens":100,"output_tokens":10}}}}}}"#
        );
        let personal_only = format!(
            r#"{{"type":"assistant","timestamp":"{timestamp}","requestId":"req_personal","message":{{"id":"msg_personal","model":"claude-sonnet-4-6","usage":{{"input_tokens":200,"output_tokens":20}}}}}}"#
        );
        let work_only = format!(
            r#"{{"type":"assistant","timestamp":"{timestamp}","requestId":"req_work","message":{{"id":"msg_work","model":"claude-sonnet-4-6","usage":{{"input_tokens":700,"output_tokens":70}}}}}}"#
        );
        std::fs::write(
            personal_projects.join("personal.jsonl"),
            format!("{shared}\n{personal_only}\n"),
        )
        .expect("personal transcript");
        std::fs::write(
            work_projects.join("work.jsonl"),
            format!("{shared}\n{work_only}\n"),
        )
        .expect("work transcript");

        // The ambient home is injected rather than exported: mutating the
        // process environment races every other test that reads it.
        let summary = CostScanner::with_ambient_and_account_homes(
            2,
            personal.path().to_path_buf(),
            vec![work.path().to_path_buf()],
        )
        .scan_claude();
        assert_eq!(
            summary.input_tokens, 1000,
            "shared 100 + personal 200 + work 700"
        );
        assert_eq!(summary.output_tokens, 100);
        assert_eq!(summary.sessions_count, 2);
    }

    #[test]
    fn claude_scan_without_existing_projects_returns_default_summary() {
        let missing = tempfile::tempdir()
            .expect("temp home")
            .path()
            .join("missing");
        let summary = CostScanner::scoped_to(2, missing).scan_claude();

        assert!(summary.period_start.is_none());
        assert!(summary.period_end.is_none());
        assert_eq!(summary.sessions_count, 0);
    }

    /// One Claude assistant event with a unique message id.
    fn claude_event_line(index: u32, output_tokens: u64) -> String {
        let ts = (Utc::now() - Duration::minutes(index as i64))
            .format("%Y-%m-%dT%H:%M:%S%.3fZ")
            .to_string();
        format!(
            r#"{{"type":"assistant","timestamp":"{ts}","requestId":"req-{index}","message":{{"id":"msg-{index}","model":"claude-opus-4-8","usage":{{"input_tokens":10,"output_tokens":{output_tokens},"cache_read_input_tokens":5}}}}}}"#
        )
    }

    #[test]
    fn resuming_an_appended_transcript_reads_the_same_records_as_a_full_parse() {
        // This is what the record index does to a transcript that grew: it
        // keeps the records it already has and reads only the new bytes. If the
        // two ever disagree, every dollar on every card is wrong.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("session.jsonl");
        let mut file = File::create(&path).unwrap();
        for index in 0..3 {
            writeln!(file, "{}", claude_event_line(index, 100)).unwrap();
            // Bulk payloads the scanner must skip, between the events.
            writeln!(file, r#"{{"type":"user","message":{{"content":"hello"}}}}"#).unwrap();
        }
        drop(file);

        let mut first_pass = Vec::new();
        let offset =
            stream_claude_records(&path, 0, None, |record| first_pass.push(record)).parsed_bytes;
        assert_eq!(first_pass.len(), 3);
        assert_eq!(offset, fs::metadata(&path).unwrap().len());

        let mut file = fs::OpenOptions::new().append(true).open(&path).unwrap();
        for index in 3..6 {
            writeln!(file, "{}", claude_event_line(index, 250)).unwrap();
        }
        drop(file);

        let mut resumed = first_pass.clone();
        let end =
            stream_claude_records(&path, offset, None, |record| resumed.push(record)).parsed_bytes;
        let mut whole = Vec::new();
        stream_claude_records(&path, 0, None, |record| whole.push(record));

        assert_eq!(end, fs::metadata(&path).unwrap().len());
        assert_eq!(resumed.len(), whole.len());
        for (resumed, whole) in resumed.iter().zip(whole.iter()) {
            assert_eq!(resumed.dedup_key, whole.dedup_key);
            assert_eq!(resumed.output, whole.output);
            assert_eq!(resumed.cost, whole.cost);
            assert_eq!(resumed.timestamp, whole.timestamp);
        }
    }

    /// A cancelled read must not swallow the line it stopped on.
    ///
    /// The offset it reports is where the next read resumes, and it used to be
    /// advanced before the cancellation check, so the record on that line was
    /// never emitted and never read again. Combined with an index entry stored
    /// under the whole file's facts, that silently dropped usage from every
    /// total until the file changed.
    #[test]
    fn a_cancelled_read_leaves_the_line_it_stopped_on_for_the_next_one() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("session.jsonl");
        let mut file = File::create(&path).unwrap();
        for index in 0..3 {
            writeln!(file, "{}", claude_event_line(index, 100)).unwrap();
        }
        drop(file);

        // Cancel as soon as the first record is in hand.
        let cancel = AtomicBool::new(false);
        let mut taken = Vec::new();
        let at = stream_claude_records(&path, 0, Some(&cancel), |record| {
            taken.push(record);
            cancel.store(true, Ordering::Relaxed);
        })
        .parsed_bytes;

        assert_eq!(taken.len(), 1, "cancellation must stop the read");
        assert!(at < fs::metadata(&path).unwrap().len());

        let mut rest = Vec::new();
        stream_claude_records(&path, at, None, |record| rest.push(record));

        assert_eq!(
            rest.len(),
            2,
            "the two records after the cancellation point must survive"
        );
        let dedup_keys: Vec<_> = taken
            .iter()
            .chain(rest.iter())
            .map(|record| record.dedup_key.clone())
            .collect();
        let mut unique = dedup_keys.clone();
        unique.sort();
        unique.dedup();
        assert_eq!(dedup_keys.len(), unique.len(), "no record read twice");
        assert_eq!(unique.len(), 3, "every record accounted for exactly once");
    }

    #[test]
    fn a_half_written_line_is_left_for_the_next_read() {
        // Transcripts are appended to while a scan runs. Stopping before a line
        // with no newline keeps the offset on a record boundary, so the partial
        // record is read once, whole, next time.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("session.jsonl");
        let mut file = File::create(&path).unwrap();
        writeln!(file, "{}", claude_event_line(0, 100)).unwrap();
        write!(file, "{}", &claude_event_line(1, 200)[..40]).unwrap();
        drop(file);

        let mut records = Vec::new();
        let offset =
            stream_claude_records(&path, 0, None, |record| records.push(record)).parsed_bytes;

        assert_eq!(records.len(), 1);
        assert!(offset < fs::metadata(&path).unwrap().len());

        // Finish the truncated line; the resumed read picks it up in full.
        let mut file = fs::OpenOptions::new().append(true).open(&path).unwrap();
        writeln!(file, "{}", &claude_event_line(1, 200)[40..]).unwrap();
        drop(file);

        let mut resumed = Vec::new();
        stream_claude_records(&path, offset, None, |record| resumed.push(record));
        assert_eq!(resumed.len(), 1);
        assert_eq!(resumed[0].output, 200);
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

    /// Opt a test into the index path and reset both stores afterwards.
    ///
    /// The suite keeps the index off so it cannot write the developer's real
    /// snapshot. Tests that pin SBS-951 turn it on for one thread and wipe
    /// the process-wide stores on drop.
    struct TestIndexGuard;

    impl TestIndexGuard {
        fn enter() -> Self {
            INDEX_APPLY_SIZES.with(|sizes| sizes.borrow_mut().clear());
            crate::usage_index::CLAUDE_INDEX.reset_for_test();
            crate::usage_index::CODEX_INDEX.reset_for_test();
            TEST_INDEX_ENABLED.with(|enabled| enabled.set(true));
            Self
        }
    }

    impl Drop for TestIndexGuard {
        fn drop(&mut self) {
            TEST_INDEX_ENABLED.with(|enabled| enabled.set(false));
            crate::usage_index::CLAUDE_INDEX.reset_for_test();
            crate::usage_index::CODEX_INDEX.reset_for_test();
            INDEX_APPLY_SIZES.with(|sizes| sizes.borrow_mut().clear());
        }
    }

    /// SBS-951: a cold scan that Stores every file must not keep every
    /// record vector in one `updates` buf until the last file.
    #[test]
    fn a_cold_claude_index_scan_flushes_stores_per_parse_batch() {
        let _guard = TestIndexGuard::enter();
        let n: usize = 40;
        let dir = tempfile::tempdir().unwrap();
        let mut files = Vec::new();
        for i in 0..n {
            let path = dir.path().join(format!("t{i}.jsonl"));
            std::fs::write(&path, claude_event_line(i as u32, 100) + "\n").unwrap();
            files.push(path);
        }

        let cutoff = Utc::now() - Duration::days(30);
        let mut folded = 0usize;
        for_each_claude_file(&files, &cutoff, None, |_, records| folded += records.len());

        let sizes = INDEX_APPLY_SIZES.with(|s| s.borrow().clone());
        let limit = index_flush_file_limit(n);
        assert_eq!(folded, n, "every transcript still reaches the fold");
        assert!(
            sizes.len() >= 2,
            "cold scan of {n} files flushed {sizes:?}; holding every Store until the end is SBS-951"
        );
        assert!(
            sizes.iter().all(|&s| s <= limit),
            "a flush held {sizes:?} files; one parse batch is {limit}"
        );
        assert_eq!(sizes.iter().sum::<usize>(), n);
    }

    /// The Codex index path had the same accumulate-then-commit shape.
    #[test]
    fn a_cold_codex_index_scan_flushes_stores_per_parse_batch() {
        let _guard = TestIndexGuard::enter();
        let n: usize = 40;
        let today = Local::now().date_naive();
        let ts = format!("{}T10:00:00.000Z", today.format("%Y-%m-%d"));
        let line = format!(
            r#"{{"timestamp":"{ts}","type":"event_msg","payload":{{"type":"token_count","info":{{"last_token_usage":{{"input_tokens":10,"cached_input_tokens":0,"output_tokens":0}}}}}}}}"#
        );
        let dir = tempfile::tempdir().unwrap();
        let mut files = Vec::new();
        for i in 0..n {
            let path = dir.path().join(format!(
                "rollout-{}-{:08}-3333-3333-3333-333333333333.jsonl",
                today.format("%Y-%m-%d"),
                i
            ));
            std::fs::write(&path, format!("{line}\n")).unwrap();
            files.push(path);
        }

        let mut folded = 0usize;
        for_each_codex_file(&files, None, |_, records| folded += records.len());

        let sizes = INDEX_APPLY_SIZES.with(|s| s.borrow().clone());
        let limit = index_flush_file_limit(n);
        assert!(folded > 0, "rollouts still reach the fold");
        assert!(
            sizes.len() >= 2,
            "cold scan of {n} files flushed {sizes:?}; holding every Store until the end is SBS-951"
        );
        assert!(
            sizes.iter().all(|&s| s <= limit),
            "a flush held {sizes:?} files; one parse batch is {limit}"
        );
        assert_eq!(sizes.iter().sum::<usize>(), n);
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
                add_claude_record_to_daily_costs(&mut daily_costs, &record);
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

    #[test]
    fn grok_report_rolls_up_tokens_cache_effort_and_project() {
        let home = tempfile::tempdir().unwrap();
        let session = home
            .path()
            .join("sessions")
            .join("proj")
            .join("019f-session");
        std::fs::create_dir_all(&session).unwrap();
        let now = Utc::now();
        let ts = now.timestamp() as f64;
        let ms = now.timestamp_millis();
        // 5_912_850_000 ticks = $0.591285
        let updates = format!(
            r#"{{"timestamp":{ts},"method":"_x.ai/session/update","params":{{"sessionId":"s1","_meta":{{"eventId":"e1","agentTimestampMs":{ms}}},"update":{{"sessionUpdate":"turn_completed","prompt_id":"p1","usage":{{"inputTokens":1000,"outputTokens":100,"cachedReadTokens":800,"reasoningTokens":40,"modelCalls":17,"costUsdTicks":5912850000,"modelUsage":{{"grok-4.5-build":{{"inputTokens":1000,"outputTokens":100,"cachedReadTokens":800,"reasoningTokens":40,"modelCalls":17,"costUsdTicks":5912850000}}}}}}}}}}}}"#
        );
        std::fs::write(session.join("updates.jsonl"), updates).unwrap();
        std::fs::write(
            session.join("summary.json"),
            r#"{"info":{"cwd":"C:\\projects\\personal\\ceiling"},"reasoning_effort":"high","current_model_id":"grok-4.5"}"#,
        )
        .unwrap();

        // SAFETY: test-only env override; restored after the scan.
        let report = scan_grok_report(
            &CostScanner::new(7).with_ambient_home(home.path().to_path_buf()),
            7,
            &[],
        );

        assert_eq!(report.thirty_days.sessions_count, 1);
        assert_eq!(report.thirty_days.input_tokens, 1000);
        assert_eq!(report.thirty_days.output_tokens, 100);
        assert_eq!(report.thirty_days.cache_read_tokens, 800);
        assert_eq!(report.thirty_days.reasoning_tokens, 40);
        assert!(report.thirty_days.by_effort_tokens.contains_key("high"));
        assert!(report.thirty_days.by_project_tokens.contains_key("ceiling"));
        assert!(
            report
                .thirty_days
                .by_model_tokens
                .contains_key("grok-4.5-build")
        );
        assert!(
            (report.thirty_days.total_cost_usd - 0.591285).abs() < 1e-9,
            "expected API-equivalent $ from costUsdTicks, got {}",
            report.thirty_days.total_cost_usd
        );
        assert!(
            !report.thirty_days.unknown_models.contains("grok-4.5-build"),
            "priced Grok rows must not count as unpriced coverage"
        );
        assert!((report.thirty_days.by_model["grok-4.5-build"] - 0.591285).abs() < 1e-9);
        assert!((report.thirty_days.by_effort["high"] - 0.591285).abs() < 1e-9);
        assert!((report.thirty_days.by_project["ceiling"] - 0.591285).abs() < 1e-9);
        assert_eq!(
            report.thirty_days.by_model_tokens["grok-4.5-build"].calls,
            17
        );
        let normalized = report.thirty_days.normalized_tokens("grok");
        assert_eq!(normalized.fresh_input_tokens, 200);
        assert_eq!(normalized.cache_read_tokens, 800);

        // Same GROK_HOME: partial session without ticks must not invent dollars
        // and must keep those tokens in the unpriced coverage set.
        let partial = home
            .path()
            .join("sessions")
            .join("proj")
            .join("019f-partial");
        std::fs::create_dir_all(&partial).unwrap();
        let bare = format!(
            r#"{{"timestamp":{ts},"method":"_x.ai/session/update","params":{{"sessionId":"s2","_meta":{{"eventId":"bare","agentTimestampMs":{ms}}},"update":{{"sessionUpdate":"turn_completed","prompt_id":"p2","stop_reason":"end_turn"}}}}}}
{{"timestamp":{ts},"method":"_x.ai/session/update","params":{{"sessionId":"s2","_meta":{{"eventId":"sa","agentTimestampMs":{ms}}},"update":{{"sessionUpdate":"subagent_finished","subagent_id":"c1","tokens_used":50000,"status":"ok"}}}}}}
"#
        );
        std::fs::write(partial.join("updates.jsonl"), bare).unwrap();
        std::fs::write(
            partial.join("summary.json"),
            r#"{"info":{"cwd":"C:\\projects\\personal\\toolport"},"current_model_id":"grok-4.5"}"#,
        )
        .unwrap();

        let mixed = scan_grok_report(
            &CostScanner::new(7).with_ambient_home(home.path().to_path_buf()),
            7,
            &[],
        );

        assert_eq!(mixed.thirty_days.input_tokens, 51_000);
        // Only the priced turn contributes dollars.
        assert!((mixed.thirty_days.total_cost_usd - 0.591285).abs() < 1e-9);
        // Partial model id from summary is unpriced.
        assert!(mixed.thirty_days.unknown_models.contains("grok-4.5"));
        // Priced model remains priced / not unknown.
        assert!(!mixed.thirty_days.unknown_models.contains("grok-4.5-build"));
    }

    /// SBS-934: CLI/serve used to treat Grok as unsupported even though the
    /// report scanner already walked `~/.grok/sessions`.
    #[test]
    fn scan_provider_supports_grok_and_not_cursor() {
        assert!(CostScanner::supports_local_scan(ProviderId::Grok));
        assert!(CostScanner::supports_local_scan(ProviderId::Codex));
        assert!(CostScanner::supports_local_scan(ProviderId::Claude));
        assert!(!CostScanner::supports_local_scan(ProviderId::Cursor));
        assert!(!CostScanner::supports_local_scan(ProviderId::Gemini));
        assert!(!CostScanner::supports_local_scan(ProviderId::Copilot));
        assert!(
            CostScanner::new(1)
                .scan_provider(ProviderId::Grok)
                .is_some()
        );
        assert!(
            CostScanner::new(1)
                .scan_provider(ProviderId::Cursor)
                .is_none()
        );
    }

    /// Buckets are keyed by the machine's local clock, not UTC, and are created
    /// only for hours that saw activity. A record with no timestamp cannot be
    /// placed on a clock and must be dropped rather than landing in an
    /// arbitrary hour.
    #[test]
    fn add_to_hourly_buckets_by_local_clock_hour() {
        let mut hourly = HourlySummaries::new();
        let at = |offset_hours: i64| Some(Utc::now() - Duration::hours(offset_hours));
        let local_key = |timestamp: Option<DateTime<Utc>>| {
            let local = timestamp.unwrap().with_timezone(&Local);
            (local.date_naive(), local.hour())
        };

        add_to_hourly(true, &mut hourly, at(2), |summary| {
            summary.input_tokens += 10
        });
        add_to_hourly(true, &mut hourly, at(2), |summary| {
            summary.input_tokens += 5
        });
        add_to_hourly(true, &mut hourly, at(5), |summary| {
            summary.input_tokens += 7
        });
        add_to_hourly(true, &mut hourly, None, |summary| {
            summary.input_tokens += 999
        });
        // Gated off, so the scans that never read these buckets pay nothing.
        add_to_hourly(false, &mut hourly, at(9), |summary| {
            summary.input_tokens += 1
        });

        assert_eq!(hourly.len(), 2);
        assert_eq!(hourly[&local_key(at(2))].input_tokens, 15);
        assert_eq!(hourly[&local_key(at(5))].input_tokens, 7);
        assert!(!hourly.values().any(|summary| summary.input_tokens == 999));
    }

    /// SBS-277: the heatmap must be a strict refinement of the daily series.
    /// Every hour bucket sums back to the day it belongs to, so the calendar
    /// view and the peak-hours view can never disagree about a total.
    #[test]
    fn hourly_activity_sums_back_to_the_daily_totals() {
        // Derive fixtures from the current local clock so the test holds in any
        // timezone: both records stay inside the two-day scan range whatever
        // the machine's offset, and the expected hours come from the same
        // conversion the scanner performs.
        let recent = Utc::now() - Duration::hours(2);
        let earlier = Utc::now() - Duration::hours(5);
        let line = |timestamp: DateTime<Utc>, input: u32| {
            format!(
                r#"{{"timestamp":"{}","type":"event_msg","payload":{{"type":"token_count","info":{{"last_token_usage":{{"input_tokens":{input},"cached_input_tokens":0,"output_tokens":0}}}}}}}}"#,
                timestamp.to_rfc3339()
            )
        };

        let today = Local::now().date_naive();
        let home = tempfile::tempdir().unwrap();
        let day_dir = home
            .path()
            .join("sessions")
            .join(today.format("%Y").to_string())
            .join(today.format("%m").to_string())
            .join(today.format("%d").to_string());
        std::fs::create_dir_all(&day_dir).unwrap();
        std::fs::write(
            day_dir.join(format!(
                "rollout-{}-33333333-3333-3333-3333-333333333333.jsonl",
                today.format("%Y-%m-%d")
            )),
            format!("{}\n{}\n", line(earlier, 300), line(recent, 1200)),
        )
        .unwrap();

        // The charts, reset windows, and API-value card all scan the same
        // trees on every refresh. They must not pay for buckets they never
        // read, so hourly work only happens when a caller asks for it.
        let without_hourly = scan_codex_report(
            &CostScanner::scoped_to(2, home.path().to_path_buf()),
            2,
            &[],
        );
        assert!(
            without_hourly.hourly_activity.is_empty(),
            "hourly bucketing is opt-in"
        );
        assert_eq!(
            without_hourly.thirty_days.input_tokens, 1_500,
            "and opting out changes nothing else"
        );

        let report = scan_codex_report(
            &CostScanner::scoped_to(2, home.path().to_path_buf()).with_hourly_activity(),
            2,
            &[],
        );

        let expected: Vec<(NaiveDate, u32)> = [earlier, recent]
            .iter()
            .map(|timestamp| {
                let local = timestamp.with_timezone(&Local);
                (local.date_naive(), local.hour())
            })
            .collect();
        let actual: Vec<(NaiveDate, u32)> = report
            .hourly_activity
            .iter()
            .map(|point| (point.date, point.hour))
            .collect();
        assert_eq!(actual, expected, "hours are emitted oldest first");

        // Each hour carries only its own record.
        let tokens: Vec<u64> = report
            .hourly_activity
            .iter()
            .map(|point| point.summary.input_tokens)
            .collect();
        assert_eq!(tokens, vec![300, 1200]);

        // The invariant: hours reconstruct the day, and the period, exactly.
        let hourly_total: u64 = tokens.iter().sum();
        assert_eq!(hourly_total, report.thirty_days.input_tokens);
        let hourly_cost: f64 = report
            .hourly_activity
            .iter()
            .map(|point| point.summary.total_cost_usd)
            .sum();
        let daily_cost: f64 = report.daily_costs.iter().map(|(_, cost)| cost).sum();
        assert!(
            (hourly_cost - daily_cost).abs() < 1e-9,
            "hourly {hourly_cost} vs daily {daily_cost}"
        );
    }
}
