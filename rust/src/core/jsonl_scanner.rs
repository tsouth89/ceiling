//! JSONL Scanner with Caching
//!
//! Incremental log file parsing for Codex and Claude session logs.
//! Supports file-level caching to avoid re-parsing unchanged files.

#![allow(dead_code)]

use crate::core::{CostUsagePricing, ProviderId};
use chrono::{DateTime, Local, NaiveDate};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashMap;
use std::fs::{self, File};
use std::io::{BufRead, BufReader, Seek, SeekFrom};
use std::path::{Path, PathBuf};

/// Cache for scanned file data
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct CostUsageCache {
    /// Last scan timestamp in milliseconds
    pub last_scan_unix_ms: i64,
    /// Per-file usage data
    pub files: HashMap<String, CostUsageFileUsage>,
    /// Aggregated daily data: day_key -> model -> [input, cached, output]
    pub days: HashMap<String, HashMap<String, Vec<i32>>>,
}

/// Per-file usage tracking
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CostUsageFileUsage {
    /// File modification time in milliseconds
    pub mtime_unix_ms: i64,
    /// File size in bytes
    pub size: i64,
    /// Daily usage data extracted from this file
    pub days: HashMap<String, HashMap<String, Vec<i32>>>,
    /// Bytes parsed so far (for incremental parsing)
    pub parsed_bytes: Option<i64>,
    /// Last model seen (for delta calculations)
    pub last_model: Option<String>,
    /// Last token totals (for delta calculations)
    pub last_totals: Option<CodexTotals>,
}

/// Running totals for Codex token counting
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CodexTotals {
    pub input: i32,
    pub cached: i32,
    pub output: i32,
}

/// Result of parsing a Codex file
#[derive(Debug)]
pub struct CodexParseResult {
    /// Individual token-count deltas used for per-request pricing.
    pub records: Vec<CodexUsageRecord>,
    /// Bytes parsed
    pub parsed_bytes: i64,
    /// Last model seen
    pub last_model: Option<String>,
    /// Last totals seen
    pub last_totals: Option<CodexTotals>,
}

/// A billable Codex token-count delta.
#[derive(Debug, Clone)]
pub struct CodexUsageRecord {
    pub day_key: String,
    pub timestamp: Option<DateTime<chrono::Utc>>,
    pub model: String,
    /// Reasoning effort in force when this delta was billed (from the enclosing
    /// `turn_context`, e.g. "medium"/"high"/"xhigh"). `None` when the log never
    /// declared one.
    pub effort: Option<String>,
    /// Project/repo the session ran in (basename of the `session_meta` cwd).
    /// `None` when the log never declared a working directory.
    pub project: Option<String>,
    pub input: i32,
    pub cached: i32,
    pub output: i32,
}

/// Day range for scanning
pub struct CostUsageDayRange {
    pub since_key: String,
    pub until_key: String,
    pub scan_since_key: String,
    pub scan_until_key: String,
}

impl CostUsageDayRange {
    pub fn new(since: NaiveDate, until: NaiveDate) -> Self {
        let since_minus_one = since - chrono::Duration::days(1);
        let until_plus_one = until + chrono::Duration::days(1);

        Self {
            since_key: Self::day_key(since),
            until_key: Self::day_key(until),
            scan_since_key: Self::day_key(since_minus_one),
            scan_until_key: Self::day_key(until_plus_one),
        }
    }

    pub fn day_key(date: NaiveDate) -> String {
        date.format("%Y-%m-%d").to_string()
    }

    pub fn is_in_range(day_key: &str, since: &str, until: &str) -> bool {
        day_key >= since && day_key <= until
    }

    pub fn parse_day_key(key: &str) -> Option<NaiveDate> {
        NaiveDate::parse_from_str(key, "%Y-%m-%d").ok()
    }
}

/// JSONL Scanner for cost/usage logs
pub struct JsonlScanner;

/// While a child/subagent/fork session replays its parent's cumulative token
/// history at the start of its own log, that history must seed the delta
/// baseline without being emitted as new usage — otherwise the replayed parent
/// totals are counted again (the ~20x Codex inflation). The gate is opened by a
/// child `session_meta` and closed by the child's first live `task_started`.
#[derive(Debug, Clone, Copy)]
enum ReplayGate {
    /// Close when a `task_started.started_at` is at or after the child's
    /// creation epoch (replayed parent task-starts predate it).
    UntilEpoch(i64),
    /// The child's creation timestamp was unparseable; close on the first
    /// `task_started` seen (a rare fallback).
    UntilFirstTaskStarted,
}

struct CodexParserState {
    current_model: Option<String>,
    /// Reasoning effort from the most recent `turn_context`, attached to each
    /// token delta so cost can be split by effort tier.
    current_effort: Option<String>,
    /// Project/repo from the session's `session_meta` cwd, attached to each
    /// token delta so cost can be split by project.
    current_project: Option<String>,
    previous_totals: Option<CodexTotals>,
    records: Vec<CodexUsageRecord>,
    /// `Some` while suppressing a child session's replayed parent history.
    replay_gate: Option<ReplayGate>,
}

#[derive(Debug, Deserialize)]
struct CodexFastLine<'a> {
    #[serde(rename = "type", borrow)]
    event_type: Option<&'a str>,
    #[serde(default, borrow)]
    timestamp: Option<&'a str>,
    #[serde(default, borrow)]
    payload: Option<CodexFastPayload<'a>>,
    #[serde(default, borrow)]
    event_msg: Option<CodexFastPayload<'a>>,
    #[serde(default, borrow)]
    model: Option<&'a str>,
}

#[derive(Debug, Deserialize)]
struct CodexFastPayload<'a> {
    #[serde(rename = "type", borrow)]
    payload_type: Option<&'a str>,
    #[serde(default, borrow)]
    model: Option<&'a str>,
    #[serde(default, borrow)]
    model_name: Option<&'a str>,
    #[serde(default, borrow)]
    effort: Option<&'a str>,
    #[serde(default, borrow)]
    info: Option<CodexFastInfo<'a>>,
    #[serde(default)]
    input_tokens: Option<i32>,
    #[serde(default)]
    cached_input_tokens: Option<i32>,
    #[serde(default)]
    cache_read_input_tokens: Option<i32>,
    #[serde(default)]
    output_tokens: Option<i32>,
}

#[derive(Debug, Deserialize)]
struct CodexFastInfo<'a> {
    #[serde(default, borrow)]
    model: Option<&'a str>,
    #[serde(default, borrow)]
    model_name: Option<&'a str>,
    #[serde(default)]
    total_token_usage: Option<CodexFastTotals>,
    #[serde(default)]
    last_token_usage: Option<CodexFastTotals>,
}

#[derive(Debug, Clone, Copy, Deserialize)]
struct CodexFastTotals {
    #[serde(default)]
    input_tokens: i32,
    #[serde(default)]
    cached_input_tokens: Option<i32>,
    #[serde(default)]
    cache_read_input_tokens: Option<i32>,
    #[serde(default)]
    output_tokens: i32,
}

enum CodexFastEvent<'a> {
    TurnContext {
        model: Option<&'a str>,
        effort: Option<&'a str>,
    },
    TokenCount {
        timestamp: &'a str,
        payload: CodexFastPayload<'a>,
    },
}

impl CodexParserState {
    fn new(initial_model: Option<String>, initial_totals: Option<CodexTotals>) -> Self {
        Self {
            current_model: initial_model,
            current_effort: None,
            current_project: None,
            previous_totals: initial_totals,
            records: Vec::new(),
            replay_gate: None,
        }
    }

    fn process_line(&mut self, line: &str, range: &CostUsageDayRange) {
        if !is_candidate_codex_line(line) {
            return;
        }

        if let Some(event) = parse_codex_fast_event(line) {
            self.process_fast_event(event, range);
            return;
        }

        let Ok(obj) = serde_json::from_str::<Value>(line) else {
            return;
        };

        // The session working directory (for per-project cost) rides on
        // session_meta. Capture it for every session_meta, including a child's,
        // before the gate logic returns early below.
        if obj.get("type").and_then(|v| v.as_str()) == Some("session_meta") {
            self.update_current_project(&obj);
        }

        // Replay-gate transitions must run regardless of the day range, since
        // they change how later token_count lines are counted.
        if let Some(gate) = detect_child_session_gate(&obj) {
            self.replay_gate = Some(gate);
            return;
        }
        if is_task_started(&obj) {
            self.clear_replay_gate_on_task_started(task_started_epoch(&obj));
            return;
        }

        if obj.get("type").and_then(|v| v.as_str()) == Some("turn_context") {
            self.update_current_model(&obj);
            return;
        }

        // The day-range filter is applied inside record_token_count, after the
        // cumulative baseline is seeded — a replayed line outside the range must
        // still advance previous_totals so later in-range deltas are correct.
        if token_count_payload(&obj).is_some() {
            self.record_token_count(&obj, range);
        }
    }

    fn clear_replay_gate_on_task_started(&mut self, started_at: Option<i64>) {
        let clear = match (self.replay_gate, started_at) {
            (Some(ReplayGate::UntilEpoch(epoch)), Some(started)) => started >= epoch,
            // Can't compare without a started_at; wait for a timestamped one.
            (Some(ReplayGate::UntilEpoch(_)), None) => false,
            (Some(ReplayGate::UntilFirstTaskStarted), _) => true,
            (None, _) => false,
        };
        if clear {
            self.replay_gate = None;
        }
    }

    fn process_fast_event(&mut self, event: CodexFastEvent<'_>, range: &CostUsageDayRange) {
        match event {
            CodexFastEvent::TurnContext { model, effort } => {
                if let Some(model) = model.filter(|model| !model.is_empty()) {
                    self.current_model = Some(model.to_string());
                }
                // Effort is a per-turn setting: reset it for every turn_context
                // so a turn that omits it (or leaves it blank) becomes
                // "unknown" rather than inheriting the previous turn's tier.
                self.current_effort = effort
                    .filter(|effort| !effort.trim().is_empty())
                    .map(|effort| effort.to_string());
            }
            CodexFastEvent::TokenCount { timestamp, payload } => {
                self.record_fast_token_count(payload, timestamp, range);
            }
        }
    }

    fn update_current_model(&mut self, obj: &Value) {
        if let Some(model) = obj
            .get("model")
            .or_else(|| obj.get("payload").and_then(|payload| payload.get("model")))
            .or_else(|| {
                obj.get("payload")
                    .and_then(|payload| payload.get("info"))
                    .and_then(|info| info.get("model"))
            })
            .and_then(|v| v.as_str())
        {
            self.current_model = Some(model.to_string());
        }
        // Effort is a per-turn setting: reset it for every turn_context so a
        // turn that omits it (or leaves it blank) becomes "unknown" rather
        // than inheriting the previous turn's tier.
        self.current_effort = obj
            .get("effort")
            .or_else(|| obj.get("payload").and_then(|payload| payload.get("effort")))
            .and_then(|v| v.as_str())
            .filter(|effort| !effort.trim().is_empty())
            .map(|effort| effort.to_string());
    }

    /// Capture the project/repo from a `session_meta` cwd (top-level or under
    /// `payload`). Reset for every session_meta so a child/fork session with no
    /// usable cwd becomes "unknown" rather than inheriting the previous
    /// session's project.
    fn update_current_project(&mut self, obj: &Value) {
        self.current_project = obj
            .get("cwd")
            .or_else(|| obj.get("payload").and_then(|payload| payload.get("cwd")))
            .and_then(|v| v.as_str())
            .and_then(crate::cost_scanner::project_from_cwd);
    }

    fn record_token_count(&mut self, obj: &Value, range: &CostUsageDayRange) {
        let Some(payload) = token_count_payload(obj) else {
            return;
        };
        // Seed `previous_totals` first — even while gated or out of range — so a
        // replayed parent total (often dated before the scan window) advances
        // the baseline instead of being re-emitted by the next in-range line.
        let Some((delta_input, delta_cached, delta_output)) = self.token_deltas(payload) else {
            return;
        };
        if self.replay_gate.is_some() {
            return;
        }
        if delta_input == 0 && delta_cached == 0 && delta_output == 0 {
            return;
        }
        let Some(day_key) = codex_line_day_key(obj, range) else {
            return;
        };

        let info = payload.get("info");
        let model = self.token_model(info, payload, obj);
        let timestamp = obj
            .get("timestamp")
            .and_then(|value| value.as_str())
            .and_then(|value| DateTime::parse_from_rfc3339(value).ok())
            .map(|value| value.with_timezone(&chrono::Utc));
        self.record_usage(
            day_key,
            timestamp,
            &model,
            delta_input,
            delta_cached,
            delta_output,
        );
    }

    fn record_fast_token_count(
        &mut self,
        payload: CodexFastPayload<'_>,
        timestamp: &str,
        range: &CostUsageDayRange,
    ) {
        // Seed the baseline first (even while gated or out of range), then
        // suppress emission for gated / zero / out-of-range lines.
        let Some((delta_input, delta_cached, delta_output)) = self.fast_token_deltas(&payload)
        else {
            return;
        };
        if self.replay_gate.is_some() {
            return;
        }
        if delta_input == 0 && delta_cached == 0 && delta_output == 0 {
            return;
        }
        let Some(day_key) = codex_timestamp_day_key(timestamp) else {
            return;
        };
        if !CostUsageDayRange::is_in_range(&day_key, &range.scan_since_key, &range.scan_until_key) {
            return;
        }

        let model = payload
            .info
            .as_ref()
            .and_then(|info| info.model.or(info.model_name))
            .or(payload.model)
            .or(self.current_model.as_deref())
            .unwrap_or("gpt-5")
            .to_string();
        let timestamp = DateTime::parse_from_rfc3339(timestamp)
            .ok()
            .map(|value| value.with_timezone(&chrono::Utc));
        self.record_usage(
            day_key,
            timestamp,
            &model,
            delta_input,
            delta_cached,
            delta_output,
        );
    }

    fn record_usage(
        &mut self,
        day_key: String,
        timestamp: Option<DateTime<chrono::Utc>>,
        model: &str,
        input: i32,
        cached: i32,
        output: i32,
    ) {
        self.records.push(CodexUsageRecord {
            day_key,
            timestamp,
            model: CostUsagePricing::normalize_codex_model(model),
            effort: self.current_effort.clone(),
            project: self.current_project.clone(),
            input,
            cached: cached.min(input),
            output,
        });
    }

    fn token_model(&self, info: Option<&Value>, payload: &Value, obj: &Value) -> String {
        info.and_then(|i| i.get("model").or(i.get("model_name")))
            .or_else(|| payload.get("model"))
            .or_else(|| obj.get("model"))
            .and_then(|v| v.as_str())
            .map(str::to_string)
            .or_else(|| self.current_model.clone())
            .unwrap_or_else(|| "gpt-5".to_string())
    }

    fn token_deltas(&mut self, payload: &Value) -> Option<(i32, i32, i32)> {
        let info = payload.get("info");
        if let Some(total) = info.and_then(|i| i.get("total_token_usage")) {
            return Some(self.total_usage_delta(total));
        }

        if let Some(last) = info.and_then(|i| i.get("last_token_usage")) {
            return Some(last_usage_delta(last));
        }

        let direct = read_token_totals(payload);
        (direct.input != 0 || direct.cached != 0 || direct.output != 0).then_some((
            direct.input.max(0),
            direct.cached.max(0),
            direct.output.max(0),
        ))
    }

    fn fast_token_deltas(&mut self, payload: &CodexFastPayload<'_>) -> Option<(i32, i32, i32)> {
        if let Some(total) = payload
            .info
            .as_ref()
            .and_then(|info| info.total_token_usage)
        {
            return Some(self.fast_total_usage_delta(total));
        }

        if let Some(last) = payload.info.as_ref().and_then(|info| info.last_token_usage) {
            return Some(fast_last_usage_delta(last));
        }

        let direct = fast_totals_from_payload(payload);
        (direct.input != 0 || direct.cached != 0 || direct.output != 0).then_some((
            direct.input.max(0),
            direct.cached.max(0),
            direct.output.max(0),
        ))
    }

    fn total_usage_delta(&mut self, total: &Value) -> (i32, i32, i32) {
        let totals = read_token_totals(total);
        let previous = self.previous_totals.as_ref();
        let delta_input = (totals.input - previous.map_or(0, |t| t.input)).max(0);
        let delta_cached = (totals.cached - previous.map_or(0, |t| t.cached)).max(0);
        let delta_output = (totals.output - previous.map_or(0, |t| t.output)).max(0);

        self.previous_totals = Some(totals);
        (delta_input, delta_cached, delta_output)
    }

    fn fast_total_usage_delta(&mut self, total: CodexFastTotals) -> (i32, i32, i32) {
        let totals = codex_totals_from_fast(total);
        let previous = self.previous_totals.as_ref();
        let delta_input = (totals.input - previous.map_or(0, |t| t.input)).max(0);
        let delta_cached = (totals.cached - previous.map_or(0, |t| t.cached)).max(0);
        let delta_output = (totals.output - previous.map_or(0, |t| t.output)).max(0);

        self.previous_totals = Some(totals);
        (delta_input, delta_cached, delta_output)
    }
}

fn parse_codex_fast_event(line: &str) -> Option<CodexFastEvent<'_>> {
    let parsed: CodexFastLine<'_> = serde_json::from_str(line).ok()?;
    match parsed.event_type? {
        "turn_context" => {
            let model = parsed
                .payload
                .as_ref()
                .and_then(|payload| {
                    payload.model.or(payload.model_name).or_else(|| {
                        payload
                            .info
                            .as_ref()
                            .and_then(|info| info.model.or(info.model_name))
                    })
                })
                .or(parsed.model);
            let effort = parsed.payload.as_ref().and_then(|payload| payload.effort);
            Some(CodexFastEvent::TurnContext { model, effort })
        }
        "event_msg" => {
            let payload = parsed.payload.or(parsed.event_msg)?;
            (payload.payload_type == Some("token_count")).then_some(CodexFastEvent::TokenCount {
                timestamp: parsed.timestamp?,
                payload,
            })
        }
        _ => None,
    }
}

/// If `obj` is a `session_meta` line for a child/subagent/fork session, return
/// the replay gate to open. Child markers mirror OpenUsage: a non-null
/// `forked_from_id` or `parent_thread_id`, or `thread_source == "subagent"`.
fn detect_child_session_gate(obj: &Value) -> Option<ReplayGate> {
    if obj.get("type").and_then(|v| v.as_str()) != Some("session_meta") {
        return None;
    }
    let payload = obj.get("payload")?;
    let is_child = payload
        .get("forked_from_id")
        .is_some_and(|value| !value.is_null())
        || payload
            .get("parent_thread_id")
            .is_some_and(|value| !value.is_null())
        || payload.get("thread_source").and_then(|v| v.as_str()) == Some("subagent");
    if !is_child {
        return None;
    }
    // Prefer the payload's own creation time; fall back to the line's top-level
    // timestamp before the weaker first-task_started gate.
    let epoch = payload
        .get("timestamp")
        .and_then(|v| v.as_str())
        .or_else(|| obj.get("timestamp").and_then(|v| v.as_str()))
        .and_then(|ts| DateTime::parse_from_rfc3339(ts).ok())
        .map(|ts| ts.timestamp());
    Some(match epoch {
        Some(epoch) => ReplayGate::UntilEpoch(epoch),
        None => ReplayGate::UntilFirstTaskStarted,
    })
}

fn is_task_started(obj: &Value) -> bool {
    obj.get("type").and_then(|v| v.as_str()) == Some("event_msg")
        && obj
            .get("payload")
            .and_then(|p| p.get("type"))
            .and_then(|v| v.as_str())
            == Some("task_started")
}

fn task_started_epoch(obj: &Value) -> Option<i64> {
    obj.get("payload")
        .and_then(|p| p.get("started_at"))
        .and_then(|v| v.as_i64())
}

fn is_candidate_codex_line(line: &str) -> bool {
    // session_meta and task_started drive child-replay gating; token_count
    // carries usage; turn_context carries the model.
    if line.contains("\"type\":\"session_meta\"") {
        return true;
    }
    if !line.contains("\"type\":\"event_msg\"")
        && !line.contains("\"type\":\"turn_context\"")
        && !line.contains("\"event_msg\"")
    {
        return false;
    }
    if line.contains("\"type\":\"event_msg\"") {
        return line.contains("\"token_count\"") || line.contains("\"task_started\"");
    }
    true
}

fn codex_line_day_key(obj: &Value, range: &CostUsageDayRange) -> Option<String> {
    let ts = obj.get("timestamp").and_then(|v| v.as_str())?;
    let day_key = codex_timestamp_day_key(ts)?;

    CostUsageDayRange::is_in_range(&day_key, &range.scan_since_key, &range.scan_until_key)
        .then_some(day_key)
}

fn codex_timestamp_day_key(timestamp: &str) -> Option<String> {
    DateTime::parse_from_rfc3339(timestamp)
        .ok()
        .map(|ts| {
            ts.with_timezone(&Local)
                .date_naive()
                .format("%Y-%m-%d")
                .to_string()
        })
        .or_else(|| timestamp.get(..10).map(str::to_string))
}

fn token_count_payload(obj: &Value) -> Option<&Value> {
    if let Some(payload) = obj.get("payload")
        && payload.get("type").and_then(|v| v.as_str()) == Some("token_count")
    {
        return Some(payload);
    }

    let event_msg = obj.get("event_msg")?;
    (event_msg.get("type").and_then(|v| v.as_str()) == Some("token_count")).then_some(event_msg)
}

fn read_token_totals(value: &Value) -> CodexTotals {
    CodexTotals {
        input: token_i32(value, "input_tokens"),
        cached: value
            .get("cached_input_tokens")
            .or_else(|| value.get("cache_read_input_tokens"))
            .and_then(|v| v.as_i64())
            .unwrap_or(0) as i32,
        output: token_i32(value, "output_tokens"),
    }
}

fn codex_totals_from_fast(value: CodexFastTotals) -> CodexTotals {
    CodexTotals {
        input: value.input_tokens,
        cached: value
            .cached_input_tokens
            .or(value.cache_read_input_tokens)
            .unwrap_or(0),
        output: value.output_tokens,
    }
}

fn fast_totals_from_payload(value: &CodexFastPayload<'_>) -> CodexTotals {
    CodexTotals {
        input: value.input_tokens.unwrap_or(0),
        cached: value
            .cached_input_tokens
            .or(value.cache_read_input_tokens)
            .unwrap_or(0),
        output: value.output_tokens.unwrap_or(0),
    }
}

fn token_i32(value: &Value, key: &str) -> i32 {
    value.get(key).and_then(|v| v.as_i64()).unwrap_or(0) as i32
}

fn last_usage_delta(last: &Value) -> (i32, i32, i32) {
    let totals = read_token_totals(last);
    (
        totals.input.max(0),
        totals.cached.max(0),
        totals.output.max(0),
    )
}

fn fast_last_usage_delta(last: CodexFastTotals) -> (i32, i32, i32) {
    let totals = codex_totals_from_fast(last);
    (
        totals.input.max(0),
        totals.cached.max(0),
        totals.output.max(0),
    )
}

impl JsonlScanner {
    /// Get default Codex sessions root directory
    pub fn default_codex_sessions_root() -> Option<PathBuf> {
        // Check CODEX_HOME environment variable
        if let Ok(home) = std::env::var("CODEX_HOME") {
            let home = home.trim();
            if !home.is_empty() {
                return Some(PathBuf::from(home).join("sessions"));
            }
        }

        // Default to ~/.codex/sessions
        dirs::home_dir().map(|h| h.join(".codex").join("sessions"))
    }

    /// Get default Claude projects roots
    pub fn default_claude_projects_roots() -> Vec<PathBuf> {
        let mut roots = Vec::new();

        // Check CLAUDE_CONFIG_DIR
        if let Ok(config_dir) = std::env::var("CLAUDE_CONFIG_DIR") {
            let path = PathBuf::from(config_dir.trim()).join("projects");
            if path.exists() {
                roots.push(path);
            }
        }

        // Default locations
        if let Some(home) = dirs::home_dir() {
            let default_path = home.join(".claude").join("projects");
            if default_path.exists() && !roots.contains(&default_path) {
                roots.push(default_path);
            }
        }

        roots
    }

    /// List Codex session files in the given date range
    pub fn list_codex_session_files(
        root: &Path,
        scan_since_key: &str,
        scan_until_key: &str,
    ) -> Vec<PathBuf> {
        let mut files = Vec::new();

        let Some(mut date) = CostUsageDayRange::parse_day_key(scan_since_key) else {
            return files;
        };
        let Some(until_date) = CostUsageDayRange::parse_day_key(scan_until_key) else {
            return files;
        };

        while date <= until_date {
            let year = format!("{:04}", date.year());
            let month = format!("{:02}", date.month());
            let day = format!("{:02}", date.day());

            let day_dir = root.join(&year).join(&month).join(&day);

            if let Ok(entries) = fs::read_dir(&day_dir) {
                for entry in entries.flatten() {
                    let path = entry.path();
                    if path
                        .extension()
                        .is_some_and(|e| e.eq_ignore_ascii_case("jsonl"))
                    {
                        files.push(path);
                    }
                }
            }

            date += chrono::Duration::days(1);
        }

        files
    }

    /// Parse a Codex JSONL file
    pub fn parse_codex_file(
        file_path: &Path,
        range: &CostUsageDayRange,
        start_offset: i64,
        initial_model: Option<String>,
        initial_totals: Option<CodexTotals>,
    ) -> std::io::Result<CodexParseResult> {
        let file = File::open(file_path)?;
        let file_size = file.metadata()?.len() as i64;

        let mut reader = BufReader::new(file);
        if start_offset > 0 {
            reader.seek(SeekFrom::Start(start_offset as u64))?;
        }

        let mut parser = CodexParserState::new(initial_model, initial_totals);
        let mut parsed_bytes = start_offset;

        let mut line = String::new();
        while reader.read_line(&mut line)? > 0 {
            parsed_bytes += line.len() as i64;
            parser.process_line(&line, range);

            line.clear();
        }

        Ok(CodexParseResult {
            records: parser.records,
            parsed_bytes: file_size.max(parsed_bytes),
            last_model: parser.current_model,
            last_totals: parser.previous_totals,
        })
    }

    /// Load cache from disk
    pub fn load_cache(provider: ProviderId, cache_root: Option<&Path>) -> CostUsageCache {
        let cache_path = Self::cache_path(provider, cache_root);

        if let Ok(contents) = fs::read_to_string(&cache_path)
            && let Ok(cache) = serde_json::from_str(&contents)
        {
            return cache;
        }

        CostUsageCache::default()
    }

    /// Save cache to disk
    pub fn save_cache(provider: ProviderId, cache: &CostUsageCache, cache_root: Option<&Path>) {
        let cache_path = Self::cache_path(provider, cache_root);

        if let Some(parent) = cache_path.parent() {
            let _ = fs::create_dir_all(parent);
        }

        if let Ok(json) = serde_json::to_string_pretty(cache) {
            let _ = fs::write(&cache_path, json);
        }
    }

    fn cache_path(provider: ProviderId, cache_root: Option<&Path>) -> PathBuf {
        let root = cache_root
            .map(|p| p.to_path_buf())
            .or_else(|| dirs::cache_dir().map(|d| d.join("CodexBar")))
            .unwrap_or_else(|| PathBuf::from("."));

        root.join(format!("{}_cost_cache.json", provider.cli_name()))
    }
}

use chrono::Datelike;

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{TimeZone, Utc};
    use std::io::Write;

    #[test]
    fn test_day_range() {
        let since = NaiveDate::from_ymd_opt(2026, 1, 15).unwrap();
        let until = NaiveDate::from_ymd_opt(2026, 1, 20).unwrap();
        let range = CostUsageDayRange::new(since, until);

        assert_eq!(range.since_key, "2026-01-15");
        assert_eq!(range.until_key, "2026-01-20");
        assert_eq!(range.scan_since_key, "2026-01-14");
        assert_eq!(range.scan_until_key, "2026-01-21");
    }

    #[test]
    fn test_is_in_range() {
        assert!(CostUsageDayRange::is_in_range(
            "2026-01-15",
            "2026-01-10",
            "2026-01-20"
        ));
        assert!(!CostUsageDayRange::is_in_range(
            "2026-01-05",
            "2026-01-10",
            "2026-01-20"
        ));
        assert!(!CostUsageDayRange::is_in_range(
            "2026-01-25",
            "2026-01-10",
            "2026-01-20"
        ));
    }

    #[test]
    fn test_parse_day_key() {
        let date = CostUsageDayRange::parse_day_key("2026-01-15");
        assert!(date.is_some());
        let date = date.unwrap();
        assert_eq!(date.year(), 2026);
        assert_eq!(date.month(), 1);
        assert_eq!(date.day(), 15);
    }

    #[test]
    fn codex_timestamp_day_key_uses_local_calendar_day() {
        let today = Local::now().date_naive();
        let local_midnight = today.and_hms_opt(0, 30, 0).unwrap();
        let Some(local_time) = Local.from_local_datetime(&local_midnight).earliest() else {
            return;
        };
        let utc_timestamp = local_time.with_timezone(&chrono::Utc).to_rfc3339();
        let expected = today.format("%Y-%m-%d").to_string();

        assert_eq!(
            codex_timestamp_day_key(&utc_timestamp).as_deref(),
            Some(expected.as_str())
        );
    }

    #[test]
    fn test_fast_codex_parser_reads_last_usage_from_payload() {
        let range = CostUsageDayRange::new(
            NaiveDate::from_ymd_opt(2026, 5, 31).unwrap(),
            NaiveDate::from_ymd_opt(2026, 5, 31).unwrap(),
        );
        let mut parser = CodexParserState::new(None, None);

        parser.process_line(
            r#"{"timestamp":"2026-05-31T10:00:00.000Z","type":"turn_context","payload":{"info":{"model":"gpt-5.5"}}}"#,
            &range,
        );
        parser.process_line(
            r#"{"timestamp":"2026-05-31T06:00:02.000-04:00","type":"event_msg","payload":{"type":"token_count","info":{"last_token_usage":{"input_tokens":120,"cache_read_input_tokens":40,"output_tokens":9}}}}"#,
            &range,
        );

        assert_eq!(parser.records.len(), 1);
        let record = &parser.records[0];
        assert_eq!(record.day_key, "2026-05-31");
        assert_eq!(record.model, "gpt-5.5");
        assert_eq!((record.input, record.cached, record.output), (120, 40, 9));
        assert_eq!(
            record.timestamp,
            Some(Utc.with_ymd_and_hms(2026, 5, 31, 10, 0, 2).unwrap())
        );
        assert_eq!(parser.current_model.as_deref(), Some("gpt-5.5"));
    }

    #[test]
    fn serde_token_path_preserves_offset_timestamps_and_tolerates_malformed_values() {
        let range = CostUsageDayRange::new(
            NaiveDate::from_ymd_opt(2026, 5, 1).unwrap(),
            NaiveDate::from_ymd_opt(2026, 6, 30).unwrap(),
        );
        let mut parser = CodexParserState::new(Some("gpt-5".to_string()), None);
        let offset = serde_json::json!({
            "timestamp": "2026-05-31T06:00:02-04:00",
            "payload": {
                "type": "token_count",
                "info": {"last_token_usage": {"input_tokens": 4, "output_tokens": 2}}
            }
        });
        parser.record_token_count(&offset, &range);
        assert_eq!(parser.records.len(), 1);
        assert_eq!(
            parser.records[0].timestamp,
            Some(Utc.with_ymd_and_hms(2026, 5, 31, 10, 0, 2).unwrap())
        );

        // A malformed timestamp has no attributable day; it is dropped rather
        // than crashing the parser or being counted under a wrong day.
        let malformed = serde_json::json!({
            "timestamp": "not-a-timestamp",
            "payload": {
                "type": "token_count",
                "info": {"last_token_usage": {"input_tokens": 1, "output_tokens": 1}}
            }
        });
        parser.record_token_count(&malformed, &range);
        assert_eq!(parser.records.len(), 1);
    }

    #[test]
    fn test_fast_codex_parser_diffs_total_usage() {
        let range = CostUsageDayRange::new(
            NaiveDate::from_ymd_opt(2026, 5, 31).unwrap(),
            NaiveDate::from_ymd_opt(2026, 5, 31).unwrap(),
        );
        let mut parser = CodexParserState::new(Some("gpt-5".to_string()), None);

        parser.process_line(
            r#"{"timestamp":"2026-05-31T10:00:01.000Z","type":"event_msg","payload":{"type":"token_count","info":{"total_token_usage":{"input_tokens":1000,"cached_input_tokens":200,"output_tokens":50}}}}"#,
            &range,
        );
        parser.process_line(
            r#"{"timestamp":"2026-05-31T10:00:02.000Z","type":"event_msg","payload":{"type":"token_count","info":{"total_token_usage":{"input_tokens":1250,"cached_input_tokens":260,"output_tokens":90}}}}"#,
            &range,
        );

        assert_eq!(parser.records.len(), 2);
        assert_eq!(
            parser
                .records
                .iter()
                .map(|record| (record.input, record.cached, record.output))
                .collect::<Vec<_>>(),
            vec![(1_000, 200, 50), (250, 60, 40)]
        );
        let totals = parser.previous_totals.expect("last totals");
        assert_eq!(totals.input, 1250);
        assert_eq!(totals.cached, 260);
        assert_eq!(totals.output, 90);
    }

    #[test]
    fn turn_context_effort_attaches_to_following_token_records() {
        let range = CostUsageDayRange::new(
            NaiveDate::from_ymd_opt(2026, 5, 31).unwrap(),
            NaiveDate::from_ymd_opt(2026, 5, 31).unwrap(),
        );
        let mut parser = CodexParserState::new(Some("gpt-5".to_string()), None);

        // A turn at "high" effort, then usage.
        parser.process_line(
            r#"{"timestamp":"2026-05-31T10:00:00.000Z","type":"turn_context","payload":{"model":"gpt-5.6-sol","effort":"high"}}"#,
            &range,
        );
        parser.process_line(
            r#"{"timestamp":"2026-05-31T10:00:01.000Z","type":"event_msg","payload":{"type":"token_count","info":{"total_token_usage":{"input_tokens":1000,"cached_input_tokens":0,"output_tokens":50}}}}"#,
            &range,
        );
        // A later turn switches to "xhigh"; the next delta carries the new tier.
        parser.process_line(
            r#"{"timestamp":"2026-05-31T10:05:00.000Z","type":"turn_context","payload":{"model":"gpt-5.6-sol","effort":"xhigh"}}"#,
            &range,
        );
        parser.process_line(
            r#"{"timestamp":"2026-05-31T10:05:01.000Z","type":"event_msg","payload":{"type":"token_count","info":{"total_token_usage":{"input_tokens":1500,"cached_input_tokens":0,"output_tokens":90}}}}"#,
            &range,
        );
        // A turn that omits effort must NOT inherit "xhigh" — it resets to None.
        parser.process_line(
            r#"{"timestamp":"2026-05-31T10:10:00.000Z","type":"turn_context","payload":{"model":"gpt-5.6-sol"}}"#,
            &range,
        );
        parser.process_line(
            r#"{"timestamp":"2026-05-31T10:10:01.000Z","type":"event_msg","payload":{"type":"token_count","info":{"total_token_usage":{"input_tokens":1600,"cached_input_tokens":0,"output_tokens":95}}}}"#,
            &range,
        );
        // A whitespace-only effort is treated the same as missing.
        parser.process_line(
            r#"{"timestamp":"2026-05-31T10:15:00.000Z","type":"turn_context","payload":{"model":"gpt-5.6-sol","effort":"  "}}"#,
            &range,
        );
        parser.process_line(
            r#"{"timestamp":"2026-05-31T10:15:01.000Z","type":"event_msg","payload":{"type":"token_count","info":{"total_token_usage":{"input_tokens":1700,"cached_input_tokens":0,"output_tokens":99}}}}"#,
            &range,
        );

        let efforts: Vec<_> = parser
            .records
            .iter()
            .map(|record| record.effort.as_deref())
            .collect();
        assert_eq!(efforts, vec![Some("high"), Some("xhigh"), None, None]);
    }

    #[test]
    fn update_current_model_resets_effort_each_turn_on_json_path() {
        // The serde_json fallback path must also reset effort per turn_context.
        let mut parser = CodexParserState::new(Some("gpt-5".to_string()), None);
        parser.update_current_model(
            &serde_json::json!({"type":"turn_context","payload":{"model":"gpt-5.6-sol","effort":"high"}}),
        );
        assert_eq!(parser.current_effort.as_deref(), Some("high"));
        // A turn without effort clears the prior tier instead of inheriting it.
        parser.update_current_model(
            &serde_json::json!({"type":"turn_context","payload":{"model":"gpt-5.6-sol"}}),
        );
        assert_eq!(parser.current_effort, None);
        // Whitespace-only effort is treated as missing.
        parser.update_current_model(
            &serde_json::json!({"type":"turn_context","payload":{"model":"gpt-5.6-sol","effort":"  "}}),
        );
        assert_eq!(parser.current_effort, None);
    }

    #[test]
    fn session_meta_project_resets_when_cwd_is_absent() {
        let mut parser = CodexParserState::new(Some("gpt-5".to_string()), None);
        parser.update_current_project(
            &serde_json::json!({"type":"session_meta","payload":{"cwd":"C:\\projects\\ceiling"}}),
        );
        assert_eq!(parser.current_project.as_deref(), Some("ceiling"));
        // A later session (fork/child) with no cwd must not inherit "ceiling".
        parser.update_current_project(
            &serde_json::json!({"type":"session_meta","payload":{"originator":"x"}}),
        );
        assert_eq!(parser.current_project, None);
        // A filesystem root carries no project name.
        parser.update_current_project(
            &serde_json::json!({"type":"session_meta","payload":{"cwd":"C:\\"}}),
        );
        assert_eq!(parser.current_project, None);
    }

    #[test]
    fn test_fast_codex_parser_reads_legacy_event_msg_shape() {
        let range = CostUsageDayRange::new(
            NaiveDate::from_ymd_opt(2026, 5, 31).unwrap(),
            NaiveDate::from_ymd_opt(2026, 5, 31).unwrap(),
        );
        let mut parser = CodexParserState::new(Some("gpt-5".to_string()), None);

        parser.process_line(
            r#"{"timestamp":"2026-05-31T10:00:02.000Z","type":"event_msg","event_msg":{"type":"token_count","input_tokens":20,"cached_input_tokens":5,"output_tokens":3}}"#,
            &range,
        );

        assert_eq!(parser.records.len(), 1);
        let record = &parser.records[0];
        assert_eq!(record.model, "gpt-5");
        assert_eq!((record.input, record.cached, record.output), (20, 5, 3));
    }

    #[test]
    fn test_parse_codex_file_uses_fast_parser_for_current_logs() {
        let mut file = tempfile::NamedTempFile::new().expect("temp file");
        writeln!(
            file,
            r#"{{"timestamp":"2026-05-31T10:00:00.000Z","type":"turn_context","payload":{{"model":"gpt-5.5"}}}}"#
        )
        .unwrap();
        writeln!(
            file,
            r#"{{"timestamp":"2026-05-31T10:00:01.000Z","type":"event_msg","payload":{{"type":"token_count","info":{{"last_token_usage":{{"input_tokens":45,"cached_input_tokens":12,"output_tokens":8}}}}}}}}"#
        )
        .unwrap();

        let range = CostUsageDayRange::new(
            NaiveDate::from_ymd_opt(2026, 5, 31).unwrap(),
            NaiveDate::from_ymd_opt(2026, 5, 31).unwrap(),
        );
        let parsed =
            JsonlScanner::parse_codex_file(file.path(), &range, 0, None, None).expect("parse");

        assert_eq!(parsed.last_model.as_deref(), Some("gpt-5.5"));
        assert_eq!(parsed.records.len(), 1);
        let record = &parsed.records[0];
        assert_eq!(record.day_key, "2026-05-31");
        assert_eq!(record.model, "gpt-5.5");
        assert_eq!((record.input, record.cached, record.output), (45, 12, 8));
    }

    #[test]
    fn child_session_replayed_parent_history_is_suppressed() {
        // Regression for the OpenUsage v0.7.6 ~20x Codex inflation: a subagent
        // log replays the parent's cumulative totals before its own work. That
        // replayed history must seed the delta baseline without being emitted.
        let creation = DateTime::parse_from_rfc3339("2026-05-31T10:00:00Z")
            .unwrap()
            .timestamp();
        let replayed_task = creation - 3600; // a parent task-start predates the child
        let mut file = tempfile::NamedTempFile::new().expect("temp file");
        // Child session opens the replay gate.
        writeln!(
            file,
            r#"{{"timestamp":"2026-05-31T10:00:00.000Z","type":"session_meta","payload":{{"id":"child","timestamp":"2026-05-31T10:00:00.000Z","thread_source":"subagent"}}}}"#
        )
        .unwrap();
        // Replayed parent task_started (predates creation) must NOT clear it.
        writeln!(
            file,
            r#"{{"timestamp":"2026-05-31T09:00:00.000Z","type":"event_msg","payload":{{"type":"task_started","started_at":{replayed_task}}}}}"#
        )
        .unwrap();
        // Replayed parent cumulative usage — suppressed, seeds baseline at 5,000,000.
        writeln!(
            file,
            r#"{{"timestamp":"2026-05-31T09:00:01.000Z","type":"event_msg","payload":{{"type":"token_count","info":{{"total_token_usage":{{"input_tokens":5000000,"cached_input_tokens":0,"output_tokens":1000000}}}}}}}}"#
        )
        .unwrap();
        // The child's own live task_started clears the gate.
        writeln!(
            file,
            r#"{{"timestamp":"2026-05-31T10:00:02.000Z","type":"event_msg","payload":{{"type":"task_started","started_at":{creation}}}}}"#
        )
        .unwrap();
        // The child's own usage — a 100-input / 5-output delta over the baseline.
        writeln!(
            file,
            r#"{{"timestamp":"2026-05-31T10:00:03.000Z","type":"event_msg","payload":{{"type":"token_count","info":{{"total_token_usage":{{"input_tokens":5000100,"cached_input_tokens":0,"output_tokens":1000005}}}}}}}}"#
        )
        .unwrap();

        let range = CostUsageDayRange::new(
            NaiveDate::from_ymd_opt(2026, 5, 31).unwrap(),
            NaiveDate::from_ymd_opt(2026, 5, 31).unwrap(),
        );
        let parsed =
            JsonlScanner::parse_codex_file(file.path(), &range, 0, None, None).expect("parse");

        // Only the child's real delta is recorded — not the replayed 5M parent total.
        assert_eq!(parsed.records.len(), 1);
        assert_eq!(
            (parsed.records[0].input, parsed.records[0].output),
            (100, 5)
        );
    }

    #[test]
    fn child_replay_history_outside_scan_range_still_seeds_baseline() {
        // The replayed parent totals are dated well before the scan window. They
        // must still advance the baseline (while suppressed), or the child's
        // first in-range line re-emits the entire parent history.
        let creation = DateTime::parse_from_rfc3339("2026-05-31T10:00:00Z")
            .unwrap()
            .timestamp();
        let mut file = tempfile::NamedTempFile::new().expect("temp file");
        writeln!(
            file,
            r#"{{"timestamp":"2026-05-31T10:00:00.000Z","type":"session_meta","payload":{{"id":"child","timestamp":"2026-05-31T10:00:00.000Z","thread_source":"subagent"}}}}"#
        )
        .unwrap();
        // Replayed parent cumulative usage, dated a month before the scan range.
        writeln!(
            file,
            r#"{{"timestamp":"2026-04-01T09:00:00.000Z","type":"event_msg","payload":{{"type":"token_count","info":{{"total_token_usage":{{"input_tokens":5000000,"cached_input_tokens":0,"output_tokens":1000000}}}}}}}}"#
        )
        .unwrap();
        writeln!(
            file,
            r#"{{"timestamp":"2026-05-31T10:00:02.000Z","type":"event_msg","payload":{{"type":"task_started","started_at":{creation}}}}}"#
        )
        .unwrap();
        writeln!(
            file,
            r#"{{"timestamp":"2026-05-31T10:00:03.000Z","type":"event_msg","payload":{{"type":"token_count","info":{{"total_token_usage":{{"input_tokens":5000100,"cached_input_tokens":0,"output_tokens":1000005}}}}}}}}"#
        )
        .unwrap();

        let range = CostUsageDayRange::new(
            NaiveDate::from_ymd_opt(2026, 5, 31).unwrap(),
            NaiveDate::from_ymd_opt(2026, 5, 31).unwrap(),
        );
        let parsed =
            JsonlScanner::parse_codex_file(file.path(), &range, 0, None, None).expect("parse");

        assert_eq!(parsed.records.len(), 1);
        assert_eq!(
            (parsed.records[0].input, parsed.records[0].output),
            (100, 5)
        );
    }

    #[test]
    fn normal_user_session_is_not_gated() {
        // thread_source "user" with null fork/parent markers is a normal
        // session; its usage must be counted as before.
        let mut file = tempfile::NamedTempFile::new().expect("temp file");
        writeln!(
            file,
            r#"{{"timestamp":"2026-05-31T10:00:00.000Z","type":"session_meta","payload":{{"id":"root","timestamp":"2026-05-31T10:00:00.000Z","thread_source":"user","forked_from_id":null,"parent_thread_id":null}}}}"#
        )
        .unwrap();
        writeln!(
            file,
            r#"{{"timestamp":"2026-05-31T10:00:01.000Z","type":"event_msg","payload":{{"type":"token_count","info":{{"last_token_usage":{{"input_tokens":45,"cached_input_tokens":12,"output_tokens":8}}}}}}}}"#
        )
        .unwrap();

        let range = CostUsageDayRange::new(
            NaiveDate::from_ymd_opt(2026, 5, 31).unwrap(),
            NaiveDate::from_ymd_opt(2026, 5, 31).unwrap(),
        );
        let parsed =
            JsonlScanner::parse_codex_file(file.path(), &range, 0, None, None).expect("parse");

        assert_eq!(parsed.records.len(), 1);
        assert_eq!((parsed.records[0].input, parsed.records[0].output), (45, 8));
    }
}
