//! Codex local-log cost aggregation helpers.

#[cfg(test)]
use chrono::Local;
use chrono::{Duration, NaiveDate};
use std::path::Path;

use crate::core::{CodexUsageRecord, CostUsageDayRange, CostUsagePricing, JsonlScanner};
use crate::cost_scanner::{CostSummary, ModelTokenCounts};

pub(crate) fn codex_period_start(today: NaiveDate, days: u32) -> NaiveDate {
    today - Duration::days(days.saturating_sub(1) as i64)
}

pub(crate) fn codex_scan_dates(range: &CostUsageDayRange) -> Vec<NaiveDate> {
    let Some(mut date) = CostUsageDayRange::parse_day_key(&range.scan_since_key) else {
        return Vec::new();
    };
    let Some(until) = CostUsageDayRange::parse_day_key(&range.scan_until_key) else {
        return Vec::new();
    };
    let mut dates = Vec::new();
    while date <= until {
        dates.push(date);
        date += Duration::days(1);
    }
    dates
}

pub(crate) fn add_codex_records_to_summary(
    summary: &mut CostSummary,
    records: &[CodexUsageRecord],
    range: &CostUsageDayRange,
) -> (f64, bool) {
    let mut total_cost = 0.0;
    let mut has_tokens = false;

    for record in records.iter().filter(|record| {
        CostUsageDayRange::is_in_range(&record.day_key, &range.since_key, &range.until_key)
    }) {
        let tokens = CodexTokenCounts::from_values(record.input, record.cached, record.output);
        if let Some(cost) =
            add_codex_tokens_to_summary(summary, &record.model, record.effort.as_deref(), tokens)
        {
            total_cost += cost;
            has_tokens = true;
        }
    }

    (total_cost, has_tokens)
}

/// Add one already-parsed Codex usage record to an aggregate.
///
/// The chart scanner uses this while building daily, weekly, and session
/// buckets from a single JSONL pass. Keeping pricing here avoids duplicating
/// the canonical Codex token semantics in the scanner.
pub(crate) fn add_codex_record_to_summary(
    summary: &mut CostSummary,
    record: &CodexUsageRecord,
) -> Option<f64> {
    let tokens = CodexTokenCounts::from_values(record.input, record.cached, record.output);
    add_codex_tokens_to_summary(summary, &record.model, record.effort.as_deref(), tokens)
}

pub(crate) fn scan_codex_file_cost_for_range(path: &Path, range: &CostUsageDayRange) -> f64 {
    let parse_result = match JsonlScanner::parse_codex_file(path, range, 0, None, None) {
        Ok(result) => result,
        Err(_) => return 0.0,
    };

    codex_records_cost(&parse_result.records, range)
}

#[cfg(test)]
pub(crate) fn scan_codex_file_cost(path: &Path) -> f64 {
    let today = Local::now().date_naive();
    let range = CostUsageDayRange::new(codex_period_start(today, 30), today);
    scan_codex_file_cost_for_range(path, &range)
}

#[derive(Clone, Copy)]
struct CodexTokenCounts {
    input: u64,
    cached: u64,
    output: u64,
}

impl CodexTokenCounts {
    fn from_values(input: i32, cached: i32, output: i32) -> Self {
        let input = input.max(0) as u64;
        Self {
            input,
            cached: (cached.max(0) as u64).min(input),
            output: output.max(0) as u64,
        }
    }

    fn is_empty(self) -> bool {
        self.input == 0 && self.cached == 0 && self.output == 0
    }
}

fn add_tokens(summary: &mut ModelTokenCounts, tokens: CodexTokenCounts) {
    summary.input_tokens += tokens.input;
    summary.output_tokens += tokens.output;
    summary.cached_tokens += tokens.cached;
}

/// Add one Codex token delta to the summary.
///
/// Returns `None` for empty tokens, `Some(0.0)` for usage of an unknown model
/// (tokens are still recorded, but no dollars are fabricated), and `Some(cost)`
/// when the model has canonical pricing. Unknown-model dollars are never added
/// to the totals — a period of only-unpriced usage must not read as `$0.00`.
fn add_codex_tokens_to_summary(
    summary: &mut CostSummary,
    model: &str,
    effort: Option<&str>,
    tokens: CodexTokenCounts,
) -> Option<f64> {
    if tokens.is_empty() {
        return None;
    }

    // Always preserve tokens and the model name so pricing coverage is visible.
    summary.input_tokens += tokens.input;
    summary.cached_tokens += tokens.cached;
    summary.cache_read_tokens += tokens.cached;
    summary.output_tokens += tokens.output;
    let effort_bucket = codex_effort_bucket(effort);
    add_tokens(
        summary
            .by_model_tokens
            .entry(model.to_string())
            .or_default(),
        tokens,
    );
    add_tokens(
        summary
            .by_effort_tokens
            .entry(effort_bucket.to_string())
            .or_default(),
        tokens,
    );

    // Unknown models stay unpriced: never fall back to a guessed rate.
    let Some(cost) =
        CostUsagePricing::codex_cost_usd(model, tokens.input, tokens.cached, tokens.output)
    else {
        summary.unknown_models.insert(model.to_string());
        return Some(0.0);
    };

    *summary.by_model.entry(model.to_string()).or_insert(0.0) += cost;
    *summary
        .by_effort
        .entry(effort_bucket.to_string())
        .or_insert(0.0) += cost;
    Some(cost)
}

fn codex_records_cost(records: &[CodexUsageRecord], range: &CostUsageDayRange) -> f64 {
    let mut total_cost = 0.0;

    for record in records.iter().filter(|record| {
        CostUsageDayRange::is_in_range(&record.day_key, &range.since_key, &range.until_key)
    }) {
        let tokens = CodexTokenCounts::from_values(record.input, record.cached, record.output);
        if tokens.is_empty() {
            continue;
        }
        // Unknown models contribute no dollars (no fabricated fallback rate).
        if let Some(cost) = CostUsagePricing::codex_cost_usd(
            &record.model,
            tokens.input,
            tokens.cached,
            tokens.output,
        ) {
            total_cost += cost;
        }
    }

    total_cost
}

/// Map a rollout's reasoning-effort string to a stable bucket key. Codex logs
/// declare effort in `turn_context` (e.g. "medium"/"high"/"xhigh"); usage
/// without a declared effort is bucketed as "unknown" rather than guessed.
fn codex_effort_bucket(effort: Option<&str>) -> String {
    match effort.map(str::trim).filter(|effort| !effort.is_empty()) {
        Some(effort) => effort.to_ascii_lowercase(),
        None => "unknown".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::CodexUsageRecord;

    #[test]
    fn summary_prices_known_model_and_leaves_unknown_model_unpriced() {
        let target = NaiveDate::from_ymd_opt(2026, 5, 31).unwrap();
        let range = CostUsageDayRange::new(target, target);
        let records = vec![
            // Canonical model: priced normally.
            CodexUsageRecord {
                day_key: "2026-05-31".to_string(),
                timestamp: None,
                model: "gpt-5.6-sol".to_string(),
                effort: Some("high".to_string()),
                input: 200_000,
                cached: 0,
                output: 0,
            },
            // Unknown model: tokens kept, but no fabricated dollars.
            CodexUsageRecord {
                day_key: "2026-05-31".to_string(),
                timestamp: None,
                model: "gpt-4o".to_string(),
                effort: Some("medium".to_string()),
                input: 1_000_000,
                cached: 0,
                output: 1_000_000,
            },
        ];
        let mut summary = CostSummary::default();

        let (cost, has_tokens) = add_codex_records_to_summary(&mut summary, &records, &range);

        assert!(has_tokens);
        // Only the known model contributes dollars; the unknown one does not.
        assert!(cost > 0.0);
        assert!(summary.by_model.contains_key("gpt-5.6-sol"));
        assert!(!summary.by_model.contains_key("gpt-4o"));
        assert!(summary.unknown_models.contains("gpt-4o"));
        // Both models' tokens are preserved for coverage.
        assert_eq!(summary.input_tokens, 1_200_000);
        // Effort tiers: only the priced record adds dollars, but both tiers
        // record tokens.
        assert!(summary.by_effort.contains_key("high"));
        assert!(!summary.by_effort.contains_key("medium"));
        assert!(summary.by_effort_tokens.contains_key("high"));
        assert!(summary.by_effort_tokens.contains_key("medium"));
    }

    #[test]
    fn codex_summary_prices_gpt56_usage_records_individually() {
        let target = NaiveDate::from_ymd_opt(2026, 5, 31).unwrap();
        let range = CostUsageDayRange::new(target, target);
        let records = vec![
            CodexUsageRecord {
                day_key: "2026-05-31".to_string(),
                timestamp: None,
                model: "gpt-5.6-sol".to_string(),
                effort: Some("high".to_string()),
                input: 200_000,
                cached: 0,
                output: 0,
            },
            CodexUsageRecord {
                day_key: "2026-05-31".to_string(),
                timestamp: None,
                model: "gpt-5.6-sol".to_string(),
                effort: Some("high".to_string()),
                input: 200_000,
                cached: 0,
                output: 0,
            },
            CodexUsageRecord {
                day_key: "2026-05-30".to_string(),
                timestamp: None,
                model: "gpt-5.6-sol".to_string(),
                effort: Some("high".to_string()),
                input: 200_000,
                cached: 0,
                output: 0,
            },
        ];
        let mut summary = CostSummary::default();

        let (cost, has_tokens) = add_codex_records_to_summary(&mut summary, &records, &range);

        assert!(has_tokens);
        assert_eq!(summary.input_tokens, 400_000);
        assert!((cost - 2.0).abs() < f64::EPSILON);
    }

    #[test]
    fn only_unpriced_usage_yields_no_dollars_but_keeps_tokens() {
        // A period containing only unknown-model usage must report tokens and
        // the unknown model, but zero dollars — never a fabricated estimate.
        let target = NaiveDate::from_ymd_opt(2026, 5, 31).unwrap();
        let range = CostUsageDayRange::new(target, target);
        let records = vec![CodexUsageRecord {
            day_key: "2026-05-31".to_string(),
            timestamp: None,
            model: "gpt-mystery".to_string(),
            effort: None,
            input: 1_000_000,
            cached: 0,
            output: 1_000_000,
        }];
        let mut summary = CostSummary::default();

        let (cost, has_tokens) = add_codex_records_to_summary(&mut summary, &records, &range);

        assert!(has_tokens);
        assert_eq!(cost, 0.0);
        assert!(summary.by_model.is_empty());
        assert!(summary.unknown_models.contains("gpt-mystery"));
        assert_eq!(summary.input_tokens, 1_000_000);
    }

    #[test]
    fn test_codex_effort_bucket() {
        // Declared effort is lowercased and trimmed to a stable key.
        assert_eq!(codex_effort_bucket(Some("high")), "high");
        assert_eq!(codex_effort_bucket(Some("  XHigh ")), "xhigh");
        // Missing or blank effort buckets as "unknown", never guessed.
        assert_eq!(codex_effort_bucket(None), "unknown");
        assert_eq!(codex_effort_bucket(Some("   ")), "unknown");
    }

    #[test]
    fn unpriced_usage_still_records_effort_tokens() {
        let target = NaiveDate::from_ymd_opt(2026, 5, 31).unwrap();
        let range = CostUsageDayRange::new(target, target);
        let records = vec![CodexUsageRecord {
            day_key: "2026-05-31".to_string(),
            timestamp: None,
            model: "gpt-mystery".to_string(),
            effort: None,
            input: 1_000_000,
            cached: 0,
            output: 0,
        }];
        let mut summary = CostSummary::default();

        add_codex_records_to_summary(&mut summary, &records, &range);

        // No dollars for the unknown model, but the effort tier keeps tokens.
        assert!(summary.by_effort.is_empty());
        assert!(summary.by_effort_tokens.contains_key("unknown"));
    }
}
