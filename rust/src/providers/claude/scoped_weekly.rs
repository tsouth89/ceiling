use chrono::{DateTime, Utc};
use serde::Deserialize;
use std::collections::HashSet;

use crate::core::{NamedRateWindow, RateWindow};

use super::cli_reset::slug_claude_model;

#[derive(Debug, Deserialize)]
pub(super) struct ScopedWeeklyLimit {
    kind: Option<String>,
    group: Option<String>,
    percent: Option<f64>,
    #[serde(alias = "resetsAt")]
    resets_at: Option<String>,
    scope: Option<ScopedWeeklyScope>,
}

#[derive(Debug, Deserialize)]
struct ScopedWeeklyScope {
    model: Option<ScopedWeeklyModel>,
}

#[derive(Debug, Deserialize)]
struct ScopedWeeklyModel {
    id: Option<String>,
    #[serde(alias = "displayName")]
    display_name: Option<String>,
}

pub(super) fn scoped_weekly_windows(limits: &[ScopedWeeklyLimit]) -> Vec<NamedRateWindow> {
    let mut seen = HashSet::new();
    limits
        .iter()
        .filter_map(|limit| {
            if limit.kind.as_deref() != Some("weekly_scoped")
                || limit.group.as_deref() != Some("weekly")
            {
                return None;
            }
            let percent = limit.percent.filter(|value| value.is_finite())?;
            let model = limit.scope.as_ref()?.model.as_ref()?;
            let title = model.display_name.as_deref()?.trim();
            if title.is_empty() {
                return None;
            }
            let identity = model
                .id
                .as_deref()
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .unwrap_or(title);
            let slug = slug_claude_model(identity);
            if slug.is_empty() || !seen.insert(slug.clone()) {
                return None;
            }
            let resets_at = limit
                .resets_at
                .as_deref()
                .and_then(|value| DateTime::parse_from_rfc3339(value).ok())
                .map(|value| value.with_timezone(&Utc));
            Some(NamedRateWindow::new(
                format!("claude-weekly-scoped-{slug}"),
                format!("{title} only"),
                RateWindow::with_details(percent, Some(7 * 24 * 60), resets_at, None),
            ))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_valid_limits_and_deduplicates_stable_model_ids() {
        let limits: Vec<ScopedWeeklyLimit> = serde_json::from_str(
            r#"[
                {"kind":"weekly_scoped","group":"weekly","percent":7,"scope":{"model":{"id":"claude/fable.5:promo","display_name":"Fable"}}},
                {"kind":"weekly_scoped","group":"weekly","percent":8,"scope":{"model":{"id":"claude/fable.5:promo","display_name":"Renamed"}}}
            ]"#,
        )
        .unwrap();

        let windows = scoped_weekly_windows(&limits);
        assert_eq!(windows.len(), 1);
        assert_eq!(windows[0].id, "claude-weekly-scoped-claude-fable-5-promo");
        assert_eq!(windows[0].title, "Fable only");
    }

    #[test]
    fn ignores_unrelated_malformed_and_unnamed_limits() {
        let limits: Vec<ScopedWeeklyLimit> = serde_json::from_str(
            r#"[
                {"kind":"session","group":"weekly","percent":7,"scope":{"model":{"display_name":"Fable"}}},
                {"kind":"weekly_scoped","group":"monthly","percent":7,"scope":{"model":{"display_name":"Fable"}}},
                {"kind":"weekly_scoped","group":"weekly","percent":7,"scope":{"model":null}},
                {"kind":"weekly_scoped","group":"weekly","percent":7,"scope":{"model":{"display_name":" "}}}
            ]"#,
        )
        .unwrap();

        assert!(scoped_weekly_windows(&limits).is_empty());
    }
}
