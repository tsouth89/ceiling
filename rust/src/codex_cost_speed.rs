//! Codex API-equivalent cost speed tier (standard vs fast/priority).
//!
//! Matches ccusage's `--speed` behavior:
//! - `standard` uses list rates
//! - `fast` multiplies those rates by 2.0 (OpenAI priority / fast tier)
//! - `auto` reads `service_tier` from `~/.codex/config.toml` (or `$CODEX_HOME`)
//!
//! When `service_tier = "priority"`, ccusage auto mode prices at the fast tier.
//! Ceiling used to always price standard, so totals looked ~half of default
//! `npx ccusage codex` on priority machines.

use std::fs;
use std::path::PathBuf;

/// Cost speed tier for Codex local-log pricing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum CodexCostSpeed {
    /// List / standard API rates (ccusage `--speed standard`).
    #[default]
    Standard,
    /// Priority / fast tier (ccusage `--speed fast`): 2× standard rates.
    Fast,
}

impl CodexCostSpeed {
    /// Label used in CLI/JSON (`standard` / `fast`).
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Standard => "standard",
            Self::Fast => "fast",
        }
    }

    /// Multiplier applied to standard list-rate dollars.
    pub fn multiplier(self) -> f64 {
        match self {
            Self::Standard => 1.0,
            Self::Fast => 2.0,
        }
    }

    /// Parse an explicit CLI/UI override (`standard`, `fast`, `priority`).
    ///
    /// Returns `None` for `auto` or unrecognized values so callers can fall
    /// back to config discovery.
    pub fn parse_override(raw: &str) -> Option<Self> {
        match raw.trim().to_ascii_lowercase().as_str() {
            "standard" | "default" | "flex" => Some(Self::Standard),
            "fast" | "priority" => Some(Self::Fast),
            "auto" | "" => None,
            _ => None,
        }
    }

    /// Map a Codex `service_tier` config value to a cost speed.
    pub fn from_service_tier(tier: &str) -> Self {
        match tier.trim().to_ascii_lowercase().as_str() {
            "priority" | "fast" => Self::Fast,
            _ => Self::Standard,
        }
    }

    /// Resolve like ccusage: explicit override, else config `service_tier`, else standard.
    pub fn resolve(speed_override: Option<&str>) -> Self {
        if let Some(raw) = speed_override
            && let Some(parsed) = Self::parse_override(raw)
        {
            return parsed;
        }
        Self::from_config_file()
    }

    /// Read `service_tier` from the active Codex config and map it.
    pub fn from_config_file() -> Self {
        match read_codex_service_tier() {
            Some(tier) => Self::from_service_tier(&tier),
            None => Self::Standard,
        }
    }
}

/// Raw `service_tier` string from config, when present.
pub fn read_codex_service_tier() -> Option<String> {
    let path = codex_config_path()?;
    let content = fs::read_to_string(path).ok()?;
    parse_service_tier_from_toml(&content)
}

fn codex_config_path() -> Option<PathBuf> {
    if let Ok(home) = std::env::var("CODEX_HOME") {
        let trimmed = home.trim();
        if !trimmed.is_empty() {
            return Some(PathBuf::from(trimmed).join("config.toml"));
        }
    }
    dirs::home_dir().map(|home| home.join(".codex").join("config.toml"))
}

/// Extract top-level `service_tier = "..."` from Codex config.toml content.
fn parse_service_tier_from_toml(content: &str) -> Option<String> {
    // Prefer full TOML parse; fall back to a line scan if the file has
    // partial/invalid sections we still want to read past.
    if let Ok(value) = content.parse::<toml::Value>()
        && let Some(tier) = value.get("service_tier").and_then(|v| v.as_str())
    {
        let trimmed = tier.trim();
        if !trimmed.is_empty() {
            return Some(trimmed.to_string());
        }
    }

    for line in content.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with('#') {
            continue;
        }
        // Stop at first table header so we only read root-level keys.
        if trimmed.starts_with('[') {
            break;
        }
        let Some(rest) = trimmed.strip_prefix("service_tier") else {
            continue;
        };
        let rest = rest.trim_start();
        let Some(rest) = rest.strip_prefix('=') else {
            continue;
        };
        let rest = rest.trim();
        let value = rest
            .strip_prefix('"')
            .and_then(|s| s.strip_suffix('"'))
            .or_else(|| rest.strip_prefix('\'').and_then(|s| s.strip_suffix('\'')))
            .unwrap_or(rest)
            .trim();
        if !value.is_empty() {
            return Some(value.to_string());
        }
    }
    None
}

/// Apply the speed multiplier to every dollar field on a cost summary.
pub fn apply_speed_to_summary(
    summary: &mut crate::cost_scanner::CostSummary,
    speed: CodexCostSpeed,
) {
    summary.codex_cost_speed = Some(speed.as_str().to_string());
    if let Some(tier) = read_codex_service_tier() {
        summary.codex_service_tier = Some(tier);
    }

    let mult = speed.multiplier();
    if (mult - 1.0).abs() < f64::EPSILON {
        return;
    }

    summary.total_cost_usd *= mult;
    for cost in summary.by_model.values_mut() {
        *cost *= mult;
    }
    for cost in summary.by_effort.values_mut() {
        *cost *= mult;
    }
    for cost in summary.by_project.values_mut() {
        *cost *= mult;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_override_accepts_ccusage_names() {
        assert_eq!(
            CodexCostSpeed::parse_override("standard"),
            Some(CodexCostSpeed::Standard)
        );
        assert_eq!(
            CodexCostSpeed::parse_override("fast"),
            Some(CodexCostSpeed::Fast)
        );
        assert_eq!(
            CodexCostSpeed::parse_override("priority"),
            Some(CodexCostSpeed::Fast)
        );
        assert_eq!(CodexCostSpeed::parse_override("auto"), None);
    }

    #[test]
    fn service_tier_priority_maps_to_fast() {
        assert_eq!(
            CodexCostSpeed::from_service_tier("priority"),
            CodexCostSpeed::Fast
        );
        assert_eq!(
            CodexCostSpeed::from_service_tier("standard"),
            CodexCostSpeed::Standard
        );
        assert_eq!(
            CodexCostSpeed::from_service_tier("flex"),
            CodexCostSpeed::Standard
        );
    }

    #[test]
    fn fast_multiplier_is_two() {
        assert!((CodexCostSpeed::Fast.multiplier() - 2.0).abs() < 1e-12);
        assert!((CodexCostSpeed::Standard.multiplier() - 1.0).abs() < 1e-12);
    }

    #[test]
    fn parses_service_tier_from_toml() {
        let content = r#"
model = "gpt-5.6-sol"
model_reasoning_effort = "medium"
service_tier = "priority"

[tui]
something = true
"#;
        assert_eq!(
            parse_service_tier_from_toml(content).as_deref(),
            Some("priority")
        );
    }

    #[test]
    fn ignores_service_tier_inside_tables() {
        let content = r#"
model = "gpt-5.6-sol"

[experimental]
service_tier = "priority"
"#;
        // Full TOML parse still sees nested keys; only root-level is intended.
        // Our primary parse reads value.get("service_tier") at root — nested is ignored.
        assert_eq!(parse_service_tier_from_toml(content), None);
    }

    #[test]
    fn line_scan_stops_at_table() {
        // Invalid TOML that still has a clear root assignment.
        let content = "service_tier = \"fast\"\n[broken\n";
        assert_eq!(
            parse_service_tier_from_toml(content).as_deref(),
            Some("fast")
        );
    }

    #[test]
    fn resolve_prefers_explicit_override() {
        assert_eq!(
            CodexCostSpeed::resolve(Some("standard")),
            CodexCostSpeed::Standard
        );
        assert_eq!(CodexCostSpeed::resolve(Some("fast")), CodexCostSpeed::Fast);
    }

    #[test]
    fn apply_speed_doubles_dollar_fields() {
        use crate::cost_scanner::CostSummary;
        use std::collections::HashMap;

        let mut summary = CostSummary {
            total_cost_usd: 10.0,
            by_model: HashMap::from([("gpt-5.6-sol".into(), 8.0)]),
            by_effort: HashMap::from([("medium".into(), 10.0)]),
            by_project: HashMap::from([("ceiling".into(), 10.0)]),
            ..CostSummary::default()
        };
        apply_speed_to_summary(&mut summary, CodexCostSpeed::Fast);
        assert!((summary.total_cost_usd - 20.0).abs() < 1e-9);
        assert!((summary.by_model["gpt-5.6-sol"] - 16.0).abs() < 1e-9);
        assert_eq!(summary.codex_cost_speed.as_deref(), Some("fast"));
    }
}
