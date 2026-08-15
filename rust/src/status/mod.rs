//! Status page polling for AI providers
//!
//! Fetches operational status from provider status pages

#![allow(dead_code)]
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Status level for a provider
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum StatusLevel {
    /// All systems operational
    Operational,
    /// Degraded performance
    Degraded,
    /// Partial outage
    Partial,
    /// Major outage
    Major,
    /// Unknown status
    #[default]
    Unknown,
}

impl StatusLevel {
    /// Create from a string indicator
    pub fn from_indicator(s: &str) -> Self {
        match s.to_lowercase().as_str() {
            "operational" | "none" | "green" | "ok" => StatusLevel::Operational,
            // Statuspage v2 `status.indicator` is none|minor|major|critical.
            // `minor` is the most common non-ok value and had no arm, so every
            // small incident on OpenAI, Claude, Cursor, and GitHub resolved to
            // Unknown and produced no badge at all.
            "degraded" | "degraded_performance" | "minor" | "yellow" => StatusLevel::Degraded,
            "partial" | "partial_outage" | "orange" => StatusLevel::Partial,
            "major" | "major_outage" | "critical" | "red" => StatusLevel::Major,
            // Planned work is not an outage, and the badge exists to tell an
            // outage apart from a spent cap. Operational is the explicit
            // decision: it neither badges nor poisons a component sweep into
            // Unknown, which the badge reads as a page it could not parse.
            "under_maintenance" | "maintenance" => StatusLevel::Operational,
            _ => StatusLevel::Unknown,
        }
    }

    /// How bad this level is, for picking the worst of several components.
    ///
    /// Not the declaration order: `Unknown` is declared last so it can be the
    /// `Default`, and comparing the discriminants let a single unreadable
    /// component outrank a real major outage on the same page.
    fn severity_rank(self) -> u8 {
        match self {
            StatusLevel::Operational => 0,
            StatusLevel::Unknown => 1,
            StatusLevel::Degraded => 2,
            StatusLevel::Partial => 3,
            StatusLevel::Major => 4,
        }
    }

    /// Get a human-readable description
    pub fn description(&self) -> &'static str {
        match self {
            StatusLevel::Operational => "All Systems Operational",
            StatusLevel::Degraded => "Degraded Performance",
            StatusLevel::Partial => "Partial Outage",
            StatusLevel::Major => "Major Outage",
            StatusLevel::Unknown => "Status Unknown",
        }
    }
}

/// Provider status information
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ProviderStatus {
    pub level: StatusLevel,
    pub description: String,
    pub last_updated: Option<String>,
    pub components: Vec<ComponentStatus>,
}

/// Individual component status
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ComponentStatus {
    pub name: String,
    pub status: StatusLevel,
}

/// Status page URLs for known providers
pub fn get_status_page_url(provider: &str) -> Option<&'static str> {
    match provider.to_lowercase().as_str() {
        "claude" | "anthropic" => Some("https://status.anthropic.com"),
        "codex" | "openai" => Some("https://status.openai.com"),
        "gemini" | "google" => Some("https://status.cloud.google.com"),
        "copilot" | "github" => Some("https://www.githubstatus.com"),
        "cursor" => Some("https://status.cursor.com"),
        "factory" | "droid" => None, // Factory.ai doesn't have a public status page
        "zai" | "z.ai" => None,      // z.ai doesn't have a public status page
        _ => None,
    }
}

/// Fetch status from a Statuspage.io-based status page
pub async fn fetch_statuspage_io(url: &str) -> Result<ProviderStatus, String> {
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(10))
        .build()
        .map_err(|e| e.to_string())?;

    // Statuspage.io API endpoint
    let api_url = format!("{}/api/v2/status.json", url.trim_end_matches('/'));

    let resp = client
        .get(&api_url)
        .header("Accept", "application/json")
        .send()
        .await
        .map_err(|e| e.to_string())?;

    if !resp.status().is_success() {
        return Err(format!("HTTP {}", resp.status()));
    }

    let json: serde_json::Value = resp.json().await.map_err(|e| e.to_string())?;

    // Parse Statuspage.io format
    let status = json
        .get("status")
        .and_then(|s| s.get("indicator"))
        .and_then(|i| i.as_str())
        .map(StatusLevel::from_indicator)
        .unwrap_or(StatusLevel::Unknown);

    let description = json
        .get("status")
        .and_then(|s| s.get("description"))
        .and_then(|d| d.as_str())
        .unwrap_or("Unknown")
        .to_string();

    let last_updated = json
        .get("page")
        .and_then(|p| p.get("updated_at"))
        .and_then(|u| u.as_str())
        .map(|s| s.to_string());

    Ok(ProviderStatus {
        level: status,
        description,
        last_updated,
        components: Vec::new(),
    })
}

/// Fetch status with components from a Statuspage.io-based status page
pub async fn fetch_statuspage_io_components(url: &str) -> Result<ProviderStatus, String> {
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(10))
        .build()
        .map_err(|e| e.to_string())?;

    // Statuspage.io components endpoint
    let api_url = format!("{}/api/v2/components.json", url.trim_end_matches('/'));

    let resp = client
        .get(&api_url)
        .header("Accept", "application/json")
        .send()
        .await
        .map_err(|e| e.to_string())?;

    if !resp.status().is_success() {
        return Err(format!("HTTP {}", resp.status()));
    }

    let json: serde_json::Value = resp.json().await.map_err(|e| e.to_string())?;

    let mut components = Vec::new();
    let mut overall_status = StatusLevel::Operational;

    if let Some(comps) = json.get("components").and_then(|c| c.as_array()) {
        for comp in comps {
            let name = comp
                .get("name")
                .and_then(|n| n.as_str())
                .unwrap_or("Unknown");
            let status_str = comp
                .get("status")
                .and_then(|s| s.as_str())
                .unwrap_or("unknown");
            let status = StatusLevel::from_indicator(status_str);

            // Update overall status to worst component
            if status.severity_rank() > overall_status.severity_rank() {
                overall_status = status;
            }

            components.push(ComponentStatus {
                name: name.to_string(),
                status,
            });
        }
    }

    Ok(ProviderStatus {
        level: overall_status,
        description: overall_status.description().to_string(),
        last_updated: None,
        components,
    })
}

/// Fetch status for a specific provider
pub async fn fetch_provider_status(provider: &str) -> Option<ProviderStatus> {
    let url = get_status_page_url(provider)?;

    // Try the simple status endpoint first
    match fetch_statuspage_io(url).await {
        Ok(status) => Some(status),
        Err(_) => {
            // Fall back to components endpoint
            fetch_statuspage_io_components(url).await.ok()
        }
    }
}

/// Fetch status for all providers in parallel
pub async fn fetch_all_statuses(providers: &[&str]) -> HashMap<String, ProviderStatus> {
    let futures: Vec<_> = providers
        .iter()
        .map(|&p| async move {
            let status = fetch_provider_status(p).await;
            (p.to_string(), status)
        })
        .collect();

    let results = futures::future::join_all(futures).await;

    results
        .into_iter()
        .filter_map(|(provider, status)| status.map(|s| (provider, s)))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Statuspage v2 `status.indicator` is `none|minor|major|critical`, and
    /// `minor` is what a small incident actually reports. It had no arm, so it
    /// fell through to `Unknown` — which the badge treats as an unreadable
    /// page, meaning the most common incident state produced nothing at all.
    #[test]
    fn every_statuspage_indicator_maps_to_a_real_level() {
        assert_eq!(
            StatusLevel::from_indicator("none"),
            StatusLevel::Operational
        );
        assert_eq!(StatusLevel::from_indicator("minor"), StatusLevel::Degraded);
        assert_eq!(StatusLevel::from_indicator("major"), StatusLevel::Major);
        assert_eq!(StatusLevel::from_indicator("critical"), StatusLevel::Major);
        for indicator in ["none", "minor", "major", "critical"] {
            assert_ne!(
                StatusLevel::from_indicator(indicator),
                StatusLevel::Unknown,
                "{indicator} is a documented Statuspage indicator"
            );
        }
    }

    /// Component status values are a different vocabulary from the top-level
    /// indicator, and `fetch_statuspage_io_components` runs them through the
    /// same mapper.
    #[test]
    fn component_status_values_map_too() {
        assert_eq!(
            StatusLevel::from_indicator("operational"),
            StatusLevel::Operational
        );
        assert_eq!(
            StatusLevel::from_indicator("degraded_performance"),
            StatusLevel::Degraded
        );
        assert_eq!(
            StatusLevel::from_indicator("partial_outage"),
            StatusLevel::Partial
        );
        assert_eq!(
            StatusLevel::from_indicator("major_outage"),
            StatusLevel::Major
        );
    }

    /// `under_maintenance` is the fifth documented component status. It fell
    /// through to `Unknown`, which the badge reads as an unreadable page, so a
    /// single component in planned maintenance made the whole provider look
    /// unreadable and retried it every five minutes.
    #[test]
    fn planned_maintenance_is_not_an_incident() {
        assert_eq!(
            StatusLevel::from_indicator("under_maintenance"),
            StatusLevel::Operational
        );
        assert_eq!(
            StatusLevel::from_indicator("maintenance"),
            StatusLevel::Operational
        );
    }

    /// `Unknown` is declared last so it can be the `Default`, so comparing the
    /// enum discriminants made one unreadable component outrank a real outage
    /// on the same page and hid the incident behind a "could not read" answer.
    #[test]
    fn a_real_outage_outranks_an_unreadable_component() {
        assert!(StatusLevel::Major.severity_rank() > StatusLevel::Unknown.severity_rank());
        assert!(StatusLevel::Degraded.severity_rank() > StatusLevel::Unknown.severity_rank());
        assert!(StatusLevel::Unknown.severity_rank() > StatusLevel::Operational.severity_rank());
        assert!(StatusLevel::Major.severity_rank() > StatusLevel::Partial.severity_rank());
        assert!(StatusLevel::Partial.severity_rank() > StatusLevel::Degraded.severity_rank());
    }

    #[test]
    fn an_unrecognized_value_is_still_unknown() {
        assert_eq!(StatusLevel::from_indicator("wat"), StatusLevel::Unknown);
        assert_eq!(StatusLevel::from_indicator(""), StatusLevel::Unknown);
    }
}
