//! Local Grok Build session usage scanner.
//!
//! Reads `~/.grok/sessions/<project>/<session-id>/updates.jsonl` turn_completed
//! records (plus sibling `summary.json` for project + reasoning effort).
//!
//! When present, `usage.costUsdTicks` is Grok's API-equivalent dollar estimate
//! (same figure `/usage` shows). Scale: USD = ticks / 10^10. SuperGrok weekly
//! pool % is still a separate subscription meter — these dollars are not cash
//! billed against the pool. Rows without ticks stay unpriced.

use chrono::{DateTime, TimeZone, Utc};
use serde::Deserialize;
use serde_json::Value;
use std::collections::HashSet;
use std::fs::{self, File};
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};

/// Grok logs store USD cost as integer ticks: `usd = ticks / COST_USD_TICKS_PER_DOLLAR`.
pub const COST_USD_TICKS_PER_DOLLAR: u64 = 10_000_000_000;

/// Convert Grok `costUsdTicks` to USD. Returns `None` when ticks are zero/absent.
pub fn cost_usd_from_ticks(ticks: Option<u64>) -> Option<f64> {
    let ticks = ticks.filter(|t| *t > 0)?;
    Some(ticks as f64 / COST_USD_TICKS_PER_DOLLAR as f64)
}

/// One turn-level usage row from a Grok session log.
#[derive(Debug, Clone)]
pub struct GrokUsageRecord {
    pub timestamp: Option<DateTime<Utc>>,
    pub model: String,
    pub effort: Option<String>,
    pub project: Option<String>,
    pub input: u64,
    pub output: u64,
    pub cache_read: u64,
    pub reasoning: u64,
    /// API-equivalent USD from `costUsdTicks` when the log provides it.
    pub cost_usd: Option<f64>,
    /// Provider `modelCalls` for this row (0 when unknown / partial).
    pub model_calls: u64,
    pub dedup_key: Option<String>,
    /// True when tokens came from a fallback signal (e.g. subagent_finished)
    /// because turn_completed omitted the full usage block.
    pub partial: bool,
}

#[derive(Debug, Clone, Default)]
pub(crate) struct SessionMeta {
    pub project: Option<String>,
    pub effort: Option<String>,
    pub model: Option<String>,
}

/// Resolve Grok home (`GROK_HOME` or `~/.grok`).
pub fn grok_home() -> Option<PathBuf> {
    if let Ok(home) = std::env::var("GROK_HOME") {
        let trimmed = home.trim();
        if !trimmed.is_empty() {
            return Some(PathBuf::from(trimmed));
        }
    }
    dirs::home_dir().map(|home| home.join(".grok"))
}

/// Root of encoded project session trees.
pub fn grok_sessions_dir(home: Option<&Path>) -> Option<PathBuf> {
    home.map(|h| h.join("sessions"))
        .or_else(|| grok_home().map(|h| h.join("sessions")))
}

/// Session directories that contain an `updates.jsonl` (or may after filtering).
pub fn discover_grok_session_dirs(sessions_root: &Path) -> Vec<PathBuf> {
    let mut sessions = Vec::new();
    let Ok(project_entries) = fs::read_dir(sessions_root) else {
        return sessions;
    };
    for project in project_entries.flatten() {
        let project_path = project.path();
        if !project_path.is_dir() {
            continue;
        }
        // Skip the search index and any non-project files at the root.
        let name = project.file_name();
        if name == "session_search.sqlite" || name.to_string_lossy().ends_with(".sqlite") {
            continue;
        }
        let Ok(session_entries) = fs::read_dir(&project_path) else {
            continue;
        };
        for session in session_entries.flatten() {
            let session_path = session.path();
            if !session_path.is_dir() {
                continue;
            }
            if session_path.join("updates.jsonl").is_file() {
                sessions.push(session_path);
            }
        }
    }
    sessions.sort();
    sessions
}

pub fn load_session_meta(session_dir: &Path) -> SessionMeta {
    let summary_path = session_dir.join("summary.json");
    let Ok(raw) = fs::read_to_string(&summary_path) else {
        return SessionMeta {
            project: project_from_session_path(session_dir),
            ..SessionMeta::default()
        };
    };
    let Ok(value) = serde_json::from_str::<Value>(&raw) else {
        return SessionMeta {
            project: project_from_session_path(session_dir),
            ..SessionMeta::default()
        };
    };
    let cwd = value
        .pointer("/info/cwd")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty());
    let project = cwd
        .map(|path| {
            Path::new(path)
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or(path)
                .to_string()
        })
        .or_else(|| project_from_session_path(session_dir));
    let effort = value
        .get("reasoning_effort")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(|s| s.to_ascii_lowercase());
    let model = value
        .get("current_model_id")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string);
    SessionMeta {
        project,
        effort,
        model,
    }
}

/// Project name from the URL-encoded parent folder Grok uses under `sessions/`.
fn project_from_session_path(session_dir: &Path) -> Option<String> {
    let encoded = session_dir.parent()?.file_name()?.to_string_lossy();
    let decoded = percent_decode(&encoded);
    let name = Path::new(&decoded)
        .file_name()
        .and_then(|n| n.to_str())
        .filter(|s| !s.is_empty())?;
    Some(name.to_string())
}

fn percent_decode(input: &str) -> String {
    let bytes = input.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%'
            && i + 2 < bytes.len()
            && let (Some(hi), Some(lo)) = (from_hex(bytes[i + 1]), from_hex(bytes[i + 2]))
        {
            out.push((hi << 4) | lo);
            i += 3;
            continue;
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn from_hex(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        b'A'..=b'F' => Some(b - b'A' + 10),
        _ => None,
    }
}

/// Parse one session's updates.jsonl into usage rows.
///
/// Prefer full `turn_completed.usage` blocks. Some Grok sessions (observed on
/// multi-project workloads) emit turn_completed without usage at all; for those
/// we fall back to weaker signals so the project still appears on charts:
/// - `subagent_finished.tokens_used` (total only, no cache/reasoning split)
/// - bare turn_completed counts as activity with zero tokens when nothing else
///   is available (keeps the project visible rather than silently dropped).
pub fn parse_grok_updates_file(
    path: &Path,
    meta: &SessionMeta,
    cutoff: DateTime<Utc>,
) -> Vec<GrokUsageRecord> {
    let Ok(file) = File::open(path) else {
        return Vec::new();
    };
    let reader = BufReader::new(file);
    let mut usage_records = Vec::new();
    let mut bare_turns: Vec<(Option<DateTime<Utc>>, Option<String>)> = Vec::new();
    let mut subagent_tokens: u64 = 0;
    let mut subagent_last_ts: Option<DateTime<Utc>> = None;
    let mut subagent_keys: Vec<String> = Vec::new();

    for line in reader.lines().map_while(Result::ok) {
        // Cheap prefilter: most lines are tool chatter.
        if !line.contains("turn_completed")
            && !line.contains("subagent_finished")
            && !line.contains("inputTokens")
        {
            continue;
        }
        let Ok(event) = serde_json::from_str::<GrokUpdateEvent>(&line) else {
            continue;
        };
        let Some(update) = event.params.update.as_ref() else {
            continue;
        };
        let kind = update.session_update.as_deref().unwrap_or("");
        let timestamp = parse_timestamp(event.timestamp, event.params.meta.as_ref());
        if timestamp.is_some_and(|ts| ts < cutoff) {
            continue;
        }

        match kind {
            "turn_completed" => {
                let from_usage = records_from_turn_completed(&event, update, meta);
                if from_usage.is_empty() {
                    let dedup = event
                        .params
                        .meta
                        .as_ref()
                        .and_then(|m| m.event_id.clone())
                        .or_else(|| {
                            update.prompt_id.as_ref().map(|pid| {
                                format!(
                                    "bare-turn:{}:{}",
                                    event.params.session_id.as_deref().unwrap_or(""),
                                    pid
                                )
                            })
                        });
                    bare_turns.push((timestamp, dedup));
                } else {
                    usage_records.extend(from_usage);
                }
            }
            "subagent_finished" => {
                if let Some(tokens) = update.tokens_used.filter(|t| *t > 0) {
                    subagent_tokens = subagent_tokens.saturating_add(tokens);
                    if timestamp.is_some_and(|ts| subagent_last_ts.is_none_or(|prev| ts > prev)) {
                        subagent_last_ts = timestamp;
                    }
                    if let Some(id) = event
                        .params
                        .meta
                        .as_ref()
                        .and_then(|m| m.event_id.clone())
                        .or_else(|| update.subagent_id.clone())
                    {
                        subagent_keys.push(format!("subagent:{id}"));
                    }
                }
            }
            _ => {}
        }
    }

    if !usage_records.is_empty() {
        return usage_records;
    }

    // No full usage telemetry in this session. Prefer subagent totals, else a
    // zero-token activity row so the project still shows on Charts.
    let model = meta.model.clone().unwrap_or_else(|| "grok".to_string());
    let project = meta.project.clone();
    let effort = meta.effort.clone();

    if subagent_tokens > 0 {
        let dedup = if subagent_keys.is_empty() {
            Some(format!("subagent-sum:{}:{subagent_tokens}", path.display()))
        } else {
            Some(subagent_keys.join("+"))
        };
        return vec![GrokUsageRecord {
            timestamp: subagent_last_ts.or_else(|| bare_turns.last().and_then(|(ts, _)| *ts)),
            model,
            effort,
            project,
            input: subagent_tokens,
            output: 0,
            cache_read: 0,
            reasoning: 0,
            cost_usd: None,
            model_calls: 0,
            dedup_key: dedup,
            partial: true,
        }];
    }

    if bare_turns.is_empty() {
        return Vec::new();
    }

    // Activity-only: one row per bare turn so session counts and project lists
    // stay honest when Grok omits usage (token totals stay 0).
    bare_turns
        .into_iter()
        .map(|(timestamp, dedup_key)| GrokUsageRecord {
            timestamp,
            model: model.clone(),
            effort: effort.clone(),
            project: project.clone(),
            input: 0,
            output: 0,
            cache_read: 0,
            reasoning: 0,
            cost_usd: None,
            model_calls: 0,
            dedup_key,
            partial: true,
        })
        .collect()
}

fn records_from_turn_completed(
    event: &GrokUpdateEvent,
    update: &GrokSessionUpdate,
    meta: &SessionMeta,
) -> Vec<GrokUsageRecord> {
    let usage = match &update.usage {
        Some(u) => u,
        None => return Vec::new(),
    };

    let timestamp = parse_timestamp(event.timestamp, event.params.meta.as_ref());
    let dedup_key = event
        .params
        .meta
        .as_ref()
        .and_then(|m| m.event_id.clone())
        .or_else(|| {
            update.prompt_id.as_ref().map(|pid| {
                format!(
                    "{}:{}",
                    event.params.session_id.as_deref().unwrap_or(""),
                    pid
                )
            })
        });
    let project = meta.project.clone();
    let effort = meta.effort.clone();
    let fallback_model = update
        .meta
        .as_ref()
        .and_then(|m| m.model_id.clone())
        .or_else(|| meta.model.clone())
        .unwrap_or_else(|| "grok".to_string());

    if let Some(model_usage) = usage.model_usage.as_ref()
        && !model_usage.is_empty()
    {
        let single_model = model_usage.len() == 1;
        let top_level_cost = cost_usd_from_ticks(usage.cost_usd_ticks);
        let top_level_calls = usage.model_calls.unwrap_or(0);
        return model_usage
            .iter()
            .map(|(model, counts)| {
                let cost_usd = cost_usd_from_ticks(counts.cost_usd_ticks).or_else(|| {
                    // Live logs put ticks on both levels for a single model;
                    // fall back to the top-level total when the model row omits it.
                    if single_model { top_level_cost } else { None }
                });
                let model_calls = counts
                    .model_calls
                    .or(if single_model {
                        usage.model_calls
                    } else {
                        None
                    })
                    .unwrap_or(0);
                GrokUsageRecord {
                    timestamp,
                    model: model.clone(),
                    effort: effort.clone(),
                    project: project.clone(),
                    input: counts.input_tokens.unwrap_or(0),
                    output: counts.output_tokens.unwrap_or(0),
                    cache_read: counts.cached_read_tokens.unwrap_or(0),
                    reasoning: counts.reasoning_tokens.unwrap_or(0),
                    cost_usd,
                    model_calls: if model_calls > 0 {
                        model_calls
                    } else if single_model {
                        top_level_calls
                    } else {
                        0
                    },
                    dedup_key: dedup_key.as_ref().map(|key| format!("{key}:{model}")),
                    partial: false,
                }
            })
            .filter(|r| {
                r.input > 0
                    || r.output > 0
                    || r.cache_read > 0
                    || r.reasoning > 0
                    || r.cost_usd.is_some()
            })
            .collect();
    }

    let record = GrokUsageRecord {
        timestamp,
        model: fallback_model,
        effort,
        project,
        input: usage.input_tokens.unwrap_or(0),
        output: usage.output_tokens.unwrap_or(0),
        cache_read: usage.cached_read_tokens.unwrap_or(0),
        reasoning: usage.reasoning_tokens.unwrap_or(0),
        cost_usd: cost_usd_from_ticks(usage.cost_usd_ticks),
        model_calls: usage.model_calls.unwrap_or(0),
        dedup_key,
        partial: false,
    };
    if record.input > 0
        || record.output > 0
        || record.cache_read > 0
        || record.reasoning > 0
        || record.cost_usd.is_some()
    {
        vec![record]
    } else {
        Vec::new()
    }
}

fn parse_timestamp(
    root_timestamp: Option<f64>,
    meta: Option<&GrokParamsMeta>,
) -> Option<DateTime<Utc>> {
    if let Some(ms) = meta.and_then(|m| m.agent_timestamp_ms) {
        let secs = ms / 1000;
        let nsecs = ((ms % 1000) * 1_000_000) as u32;
        return Utc.timestamp_opt(secs, nsecs).single();
    }
    let ts = root_timestamp?;
    if !ts.is_finite() || ts <= 0.0 {
        return None;
    }
    // Seconds if small; milliseconds if clearly past year ~2001 in ms scale.
    if ts > 1_000_000_000_000.0 {
        let ms = ts as i64;
        Utc.timestamp_opt(ms / 1000, ((ms % 1000) * 1_000_000) as u32)
            .single()
    } else {
        Utc.timestamp_opt(ts as i64, 0).single()
    }
}

pub fn should_count_grok_record(
    record: &GrokUsageRecord,
    cutoff: DateTime<Utc>,
    seen: &mut HashSet<String>,
) -> bool {
    if record.timestamp.is_some_and(|ts| ts < cutoff) {
        return false;
    }
    if let Some(key) = record.dedup_key.as_ref()
        && !seen.insert(key.clone())
    {
        return false;
    }
    true
}

#[derive(Debug, Deserialize)]
struct GrokUpdateEvent {
    #[serde(default)]
    timestamp: Option<f64>,
    #[serde(default)]
    params: GrokUpdateParams,
}

#[derive(Debug, Default, Deserialize)]
struct GrokUpdateParams {
    #[serde(default, rename = "sessionId")]
    session_id: Option<String>,
    #[serde(default, rename = "_meta")]
    meta: Option<GrokParamsMeta>,
    #[serde(default)]
    update: Option<GrokSessionUpdate>,
}

#[derive(Debug, Deserialize)]
struct GrokParamsMeta {
    #[serde(default, rename = "eventId")]
    event_id: Option<String>,
    #[serde(default, rename = "agentTimestampMs")]
    agent_timestamp_ms: Option<i64>,
}

#[derive(Debug, Deserialize)]
struct GrokSessionUpdate {
    #[serde(default, rename = "sessionUpdate")]
    session_update: Option<String>,
    #[serde(default, rename = "prompt_id")]
    prompt_id: Option<String>,
    #[serde(default, rename = "_meta")]
    meta: Option<GrokUpdateMeta>,
    #[serde(default)]
    usage: Option<GrokUsageBlock>,
    /// Present on `subagent_finished` (total tokens only).
    #[serde(default)]
    tokens_used: Option<u64>,
    #[serde(default)]
    subagent_id: Option<String>,
}

#[derive(Debug, Deserialize)]
struct GrokUpdateMeta {
    #[serde(default, rename = "modelId")]
    model_id: Option<String>,
}

#[derive(Debug, Deserialize)]
struct GrokUsageBlock {
    #[serde(default, rename = "inputTokens")]
    input_tokens: Option<u64>,
    #[serde(default, rename = "outputTokens")]
    output_tokens: Option<u64>,
    #[serde(default, rename = "cachedReadTokens")]
    cached_read_tokens: Option<u64>,
    #[serde(default, rename = "reasoningTokens")]
    reasoning_tokens: Option<u64>,
    /// API-equivalent USD × 10^10 (same units `/usage` displays as Cost).
    #[serde(default, rename = "costUsdTicks")]
    cost_usd_ticks: Option<u64>,
    #[serde(default, rename = "modelCalls")]
    model_calls: Option<u64>,
    #[serde(default, rename = "modelUsage")]
    model_usage: Option<std::collections::HashMap<String, GrokModelUsageCounts>>,
}

#[derive(Debug, Deserialize)]
struct GrokModelUsageCounts {
    #[serde(default, rename = "inputTokens")]
    input_tokens: Option<u64>,
    #[serde(default, rename = "outputTokens")]
    output_tokens: Option<u64>,
    #[serde(default, rename = "cachedReadTokens")]
    cached_read_tokens: Option<u64>,
    #[serde(default, rename = "reasoningTokens")]
    reasoning_tokens: Option<u64>,
    #[serde(default, rename = "costUsdTicks")]
    cost_usd_ticks: Option<u64>,
    #[serde(default, rename = "modelCalls")]
    model_calls: Option<u64>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use tempfile::tempdir;

    fn write_session(dir: &Path, updates: &str, summary: &str) {
        fs::create_dir_all(dir).unwrap();
        fs::write(dir.join("summary.json"), summary).unwrap();
        let mut f = File::create(dir.join("updates.jsonl")).unwrap();
        f.write_all(updates.as_bytes()).unwrap();
    }

    #[test]
    fn parses_turn_completed_with_cache_and_reasoning() {
        let dir = tempdir().unwrap();
        let session = dir.path().join("sess");
        let ts = Utc
            .with_ymd_and_hms(2026, 7, 20, 12, 0, 0)
            .unwrap()
            .timestamp() as f64;
        let ms = (ts * 1000.0) as i64;
        // 591284544000 ticks = $59.1284544 (matches live /usage scale).
        let updates = format!(
            r#"{{"timestamp":{ts},"method":"_x.ai/session/update","params":{{"sessionId":"s1","_meta":{{"eventId":"e1","agentTimestampMs":{ms}}},"update":{{"sessionUpdate":"turn_completed","prompt_id":"p1","usage":{{"inputTokens":1000,"outputTokens":100,"totalTokens":1100,"cachedReadTokens":800,"reasoningTokens":40,"modelCalls":17,"costUsdTicks":5912850000,"modelUsage":{{"grok-4.5-build":{{"inputTokens":1000,"outputTokens":100,"cachedReadTokens":800,"reasoningTokens":40,"modelCalls":17,"costUsdTicks":5912850000}}}}}}}}}}}}"#
        );
        let summary = r#"{
          "info": {"id": "s1", "cwd": "C:\\projects\\personal\\ceiling"},
          "current_model_id": "grok-4.5",
          "reasoning_effort": "high"
        }"#;
        write_session(&session, &updates, summary);

        let meta = load_session_meta(&session);
        assert_eq!(meta.project.as_deref(), Some("ceiling"));
        assert_eq!(meta.effort.as_deref(), Some("high"));

        let cutoff = Utc.with_ymd_and_hms(2026, 1, 1, 0, 0, 0).unwrap();
        let records = parse_grok_updates_file(&session.join("updates.jsonl"), &meta, cutoff);
        assert_eq!(records.len(), 1);
        let r = &records[0];
        assert_eq!(r.model, "grok-4.5-build");
        assert_eq!(r.input, 1000);
        assert_eq!(r.output, 100);
        assert_eq!(r.cache_read, 800);
        assert_eq!(r.reasoning, 40);
        assert_eq!(r.model_calls, 17);
        assert_eq!(r.effort.as_deref(), Some("high"));
        assert_eq!(r.project.as_deref(), Some("ceiling"));
        assert!(!r.partial);
        let cost = r.cost_usd.expect("costUsdTicks should price the row");
        assert!((cost - 0.591285).abs() < 1e-9);
    }

    #[test]
    fn cost_usd_from_ticks_matches_usage_scale() {
        // Live toolport session: sum(costUsdTicks) / 1e10 == $59.1285
        assert_eq!(cost_usd_from_ticks(None), None);
        assert_eq!(cost_usd_from_ticks(Some(0)), None);
        let cost = cost_usd_from_ticks(Some(591_285_440_000)).unwrap();
        assert!((cost - 59.128544).abs() < 1e-9);
    }

    #[test]
    fn dedups_on_event_id() {
        let mut seen = HashSet::new();
        let record = GrokUsageRecord {
            timestamp: Some(Utc::now()),
            model: "grok-4.5-build".into(),
            effort: Some("high".into()),
            project: Some("ceiling".into()),
            input: 10,
            output: 1,
            cache_read: 0,
            reasoning: 0,
            cost_usd: Some(0.01),
            model_calls: 1,
            dedup_key: Some("e1:grok-4.5-build".into()),
            partial: false,
        };
        let cutoff = Utc::now() - chrono::Duration::days(30);
        assert!(should_count_grok_record(&record, cutoff, &mut seen));
        assert!(!should_count_grok_record(&record, cutoff, &mut seen));
    }

    #[test]
    fn discovers_nested_session_dirs() {
        let dir = tempdir().unwrap();
        let sessions = dir.path().join("sessions");
        let nested = sessions
            .join("C%3A%5Cprojects%5Cpersonal%5Cceiling")
            .join("019f-session");
        write_session(
            &nested,
            "{\"timestamp\":1}\n",
            r#"{"info":{"cwd":"C:\\projects\\personal\\ceiling"}}"#,
        );
        fs::write(sessions.join("session_search.sqlite"), b"").unwrap();
        let found = discover_grok_session_dirs(&sessions);
        assert_eq!(found, vec![nested]);
    }

    #[test]
    fn bare_turn_completed_without_usage_still_attributes_project() {
        // Live toolport logs: turn_completed with only stop_reason, no usage.
        let dir = tempdir().unwrap();
        let session = dir
            .path()
            .join("C%3A%5Cprojects%5Cpersonal%5Ctoolport")
            .join("sess");
        let ts = Utc
            .with_ymd_and_hms(2026, 7, 24, 12, 0, 0)
            .unwrap()
            .timestamp() as f64;
        let ms = (ts * 1000.0) as i64;
        let updates = format!(
            r#"{{"timestamp":{ts},"method":"_x.ai/session/update","params":{{"sessionId":"s1","_meta":{{"eventId":"bare1","agentTimestampMs":{ms}}},"update":{{"sessionUpdate":"turn_completed","prompt_id":"p1","stop_reason":"end_turn"}}}}}}
{{"timestamp":{ts},"method":"_x.ai/session/update","params":{{"sessionId":"s1","_meta":{{"eventId":"bare2","agentTimestampMs":{ms}}},"update":{{"sessionUpdate":"turn_completed","prompt_id":"p2","stop_reason":"end_turn"}}}}}}
"#
        );
        let summary = r#"{
          "info": {"id": "s1", "cwd": "C:\\projects\\personal\\toolport"},
          "current_model_id": "grok-4.5",
          "reasoning_effort": "high"
        }"#;
        write_session(&session, &updates, summary);
        let meta = load_session_meta(&session);
        assert_eq!(meta.project.as_deref(), Some("toolport"));
        let cutoff = Utc.with_ymd_and_hms(2026, 1, 1, 0, 0, 0).unwrap();
        let records = parse_grok_updates_file(&session.join("updates.jsonl"), &meta, cutoff);
        assert_eq!(records.len(), 2);
        assert!(records.iter().all(|r| r.partial));
        assert!(
            records
                .iter()
                .all(|r| r.project.as_deref() == Some("toolport"))
        );
        assert_eq!(records.iter().map(|r| r.input + r.output).sum::<u64>(), 0);
    }

    #[test]
    fn subagent_tokens_used_when_usage_block_missing() {
        let dir = tempdir().unwrap();
        let session = dir.path().join("sess");
        let ts = Utc
            .with_ymd_and_hms(2026, 7, 24, 12, 0, 0)
            .unwrap()
            .timestamp() as f64;
        let ms = (ts * 1000.0) as i64;
        let updates = format!(
            r#"{{"timestamp":{ts},"method":"_x.ai/session/update","params":{{"sessionId":"s1","_meta":{{"eventId":"bare1","agentTimestampMs":{ms}}},"update":{{"sessionUpdate":"turn_completed","prompt_id":"p1","stop_reason":"end_turn"}}}}}}
{{"timestamp":{ts},"method":"_x.ai/session/update","params":{{"sessionId":"s1","_meta":{{"eventId":"sa1","agentTimestampMs":{ms}}},"update":{{"sessionUpdate":"subagent_finished","subagent_id":"child-1","tokens_used":50000,"status":"ok"}}}}}}
{{"timestamp":{ts},"method":"_x.ai/session/update","params":{{"sessionId":"s1","_meta":{{"eventId":"sa2","agentTimestampMs":{ms}}},"update":{{"sessionUpdate":"subagent_finished","subagent_id":"child-2","tokens_used":25000,"status":"ok"}}}}}}
"#
        );
        let summary = r#"{
          "info": {"cwd": "C:\\projects\\personal\\toolport"},
          "current_model_id": "grok-4.5",
          "reasoning_effort": "high"
        }"#;
        write_session(&session, &updates, summary);
        let meta = load_session_meta(&session);
        let cutoff = Utc.with_ymd_and_hms(2026, 1, 1, 0, 0, 0).unwrap();
        let records = parse_grok_updates_file(&session.join("updates.jsonl"), &meta, cutoff);
        assert_eq!(records.len(), 1);
        assert!(records[0].partial);
        assert_eq!(records[0].input, 75_000);
        assert_eq!(records[0].project.as_deref(), Some("toolport"));
    }

    #[test]
    fn project_name_from_encoded_session_path_when_summary_missing() {
        let dir = tempdir().unwrap();
        let session = dir
            .path()
            .join("C%3A%5Cprojects%5Cpersonal%5Ctoolport")
            .join("sess");
        fs::create_dir_all(&session).unwrap();
        let meta = load_session_meta(&session);
        assert_eq!(meta.project.as_deref(), Some("toolport"));
    }
}
