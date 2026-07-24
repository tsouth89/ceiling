//! Local Grok Build session usage scanner.
//!
//! Reads `~/.grok/sessions/<project>/<session-id>/updates.jsonl` turn_completed
//! records (plus sibling `summary.json` for project + reasoning effort). Grok
//! does not publish per-token API rates for SuperGrok pool usage, so costs stay
//! unpriced; token / cache / effort / project rollups still feed charts.

use chrono::{DateTime, TimeZone, Utc};
use serde::Deserialize;
use serde_json::Value;
use std::collections::HashSet;
use std::fs::{self, File};
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};

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
    pub dedup_key: Option<String>,
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
        return SessionMeta::default();
    };
    let Ok(value) = serde_json::from_str::<Value>(&raw) else {
        return SessionMeta::default();
    };
    let cwd = value
        .pointer("/info/cwd")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty());
    let project = cwd.map(|path| {
        Path::new(path)
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or(path)
            .to_string()
    });
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

/// Parse turn_completed usage rows from one session's updates.jsonl.
pub fn parse_grok_updates_file(
    path: &Path,
    meta: &SessionMeta,
    cutoff: DateTime<Utc>,
) -> Vec<GrokUsageRecord> {
    let Ok(file) = File::open(path) else {
        return Vec::new();
    };
    let reader = BufReader::new(file);
    let mut records = Vec::new();
    for line in reader.lines().map_while(Result::ok) {
        if !line.contains("turn_completed") {
            continue;
        }
        let Ok(event) = serde_json::from_str::<GrokUpdateEvent>(&line) else {
            continue;
        };
        for record in records_from_event(&event, meta) {
            if record.timestamp.is_some_and(|ts| ts < cutoff) {
                continue;
            }
            records.push(record);
        }
    }
    records
}

fn records_from_event(event: &GrokUpdateEvent, meta: &SessionMeta) -> Vec<GrokUsageRecord> {
    let update = match &event.params.update {
        Some(u) if u.session_update.as_deref() == Some("turn_completed") => u,
        _ => return Vec::new(),
    };
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
        return model_usage
            .iter()
            .map(|(model, counts)| GrokUsageRecord {
                timestamp,
                model: model.clone(),
                effort: effort.clone(),
                project: project.clone(),
                input: counts.input_tokens.unwrap_or(0),
                output: counts.output_tokens.unwrap_or(0),
                cache_read: counts.cached_read_tokens.unwrap_or(0),
                reasoning: counts.reasoning_tokens.unwrap_or(0),
                dedup_key: dedup_key.as_ref().map(|key| format!("{key}:{model}")),
            })
            .filter(|r| r.input > 0 || r.output > 0 || r.cache_read > 0 || r.reasoning > 0)
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
        dedup_key,
    };
    if record.input > 0 || record.output > 0 || record.cache_read > 0 || record.reasoning > 0 {
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
        let updates = format!(
            r#"{{"timestamp":{ts},"method":"_x.ai/session/update","params":{{"sessionId":"s1","_meta":{{"eventId":"e1","agentTimestampMs":{ms}}},"update":{{"sessionUpdate":"turn_completed","prompt_id":"p1","usage":{{"inputTokens":1000,"outputTokens":100,"totalTokens":1100,"cachedReadTokens":800,"reasoningTokens":40,"modelUsage":{{"grok-4.5-build":{{"inputTokens":1000,"outputTokens":100,"cachedReadTokens":800,"reasoningTokens":40}}}}}}}}}}}}"#
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
        assert_eq!(r.effort.as_deref(), Some("high"));
        assert_eq!(r.project.as_deref(), Some("ceiling"));
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
            dedup_key: Some("e1:grok-4.5-build".into()),
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
}
