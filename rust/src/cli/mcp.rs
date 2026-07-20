//! `codexbar mcp` — local-first MCP server for usage and spend.
//!
//! Speaks Model Context Protocol over stdio so Claude Code / Codex can query
//! remaining quota and estimated spend mid-conversation. Quota comes from the
//! desktop widget snapshot (cache-only, no network). Spend comes from local
//! Codex/Claude JSONL logs via [`crate::cost_scanner`].

use clap::Args;
use rmcp::{
    ErrorData as McpError, ServerHandler, ServiceExt,
    handler::server::wrapper::Parameters,
    model::{
        CallToolResult, ContentBlock, Implementation, ProtocolVersion, ServerCapabilities,
        ServerInfo,
    },
    schemars, tool, tool_handler, tool_router,
    transport::stdio,
};
use serde::Deserialize;
use serde_json::json;

use crate::core::{
    ProviderId, RateWindow, WidgetProviderEntry, WidgetSnapshot, WidgetSnapshotStore,
};
use crate::cost_scanner::{CostSummary, get_cost_usage_report};

const COST_SUPPORTED: &[&str] = &["codex", "claude"];

#[derive(Args, Debug, Clone, Default)]
pub struct McpArgs {}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
struct ProviderFilter {
    /// Optional provider id (`claude`, `codex`, `cursor`, …). Omit for all.
    #[serde(default)]
    provider: Option<String>,
}

#[derive(Clone)]
struct CeilingMcp {
    tool_router: rmcp::handler::server::router::tool::ToolRouter<CeilingMcp>,
}

#[tool_router]
impl CeilingMcp {
    fn new() -> Self {
        Self {
            tool_router: Self::tool_router(),
        }
    }

    #[tool(
        description = "List providers with cached quota and whether local spend scanning is supported (Codex/Claude)."
    )]
    fn list_providers(&self) -> Result<CallToolResult, McpError> {
        Ok(json_tool_result(list_providers_payload(
            WidgetSnapshotStore::load().as_ref(),
        )))
    }

    #[tool(
        description = "Get remaining quota windows from the desktop widget snapshot (cache-only, no network). Requires Ceiling desktop to have refreshed recently."
    )]
    fn get_usage(
        &self,
        Parameters(ProviderFilter { provider }): Parameters<ProviderFilter>,
    ) -> Result<CallToolResult, McpError> {
        Ok(json_tool_result(usage_payload(
            WidgetSnapshotStore::load().as_ref(),
            provider.as_deref(),
        )))
    }

    #[tool(
        description = "Get estimated API-value spend from local Codex/Claude logs for today, 7 days, and 30 days. Local-only; not a bill."
    )]
    fn get_spend(
        &self,
        Parameters(ProviderFilter { provider }): Parameters<ProviderFilter>,
    ) -> Result<CallToolResult, McpError> {
        Ok(json_tool_result(spend_payload(provider.as_deref())))
    }

    #[tool(
        description = "Compact status: remaining quota + today's estimated spend for one provider (or the best available). Good for 'am I about to hit my cap?' checks."
    )]
    fn get_status(
        &self,
        Parameters(ProviderFilter { provider }): Parameters<ProviderFilter>,
    ) -> Result<CallToolResult, McpError> {
        Ok(json_tool_result(status_payload(
            WidgetSnapshotStore::load().as_ref(),
            provider.as_deref(),
        )))
    }
}

#[tool_handler]
impl ServerHandler for CeilingMcp {
    fn get_info(&self) -> ServerInfo {
        ServerInfo::new(ServerCapabilities::builder().enable_tools().build())
            .with_server_info(Implementation::new("ceiling", env!("CARGO_PKG_VERSION")))
            .with_protocol_version(ProtocolVersion::V_2024_11_05)
            .with_instructions(
                "Ceiling local usage/spend tools. get_usage reads the desktop widget snapshot \
(cache-only). get_spend scans local Codex/Claude logs. Prefer get_status for a quick \
remaining-quota + today-$ check before starting a large job. Values are estimated API \
value, never a billed invoice."
                    .to_string(),
            )
    }
}

pub async fn run(_args: McpArgs) -> anyhow::Result<()> {
    tracing::info!("Starting Ceiling MCP server (stdio)");
    let service = CeilingMcp::new().serve(stdio()).await?;
    service.waiting().await?;
    Ok(())
}

fn json_tool_result(value: serde_json::Value) -> CallToolResult {
    CallToolResult::success(vec![ContentBlock::text(value.to_string())])
}

fn list_providers_payload(snapshot: Option<&WidgetSnapshot>) -> serde_json::Value {
    let snapshot_providers: Vec<String> = snapshot
        .map(|s| {
            s.entries
                .iter()
                .map(|e| e.provider.cli_name().to_string())
                .collect()
        })
        .unwrap_or_default();

    let mut providers = Vec::new();
    for id in ProviderId::all() {
        let cli = id.cli_name();
        let in_snapshot = snapshot_providers.iter().any(|p| p == cli);
        providers.push(json!({
            "id": cli,
            "display_name": id.display_name(),
            "has_quota_snapshot": in_snapshot,
            "local_spend_supported": COST_SUPPORTED.contains(&cli),
        }));
    }

    json!({
        "snapshot_present": snapshot.is_some(),
        "snapshot_generated_at": snapshot.map(|s| s.generated_at.to_rfc3339()),
        "providers": providers,
    })
}

fn usage_payload(snapshot: Option<&WidgetSnapshot>, provider: Option<&str>) -> serde_json::Value {
    let Some(snapshot) = snapshot else {
        return json!({
            "ok": false,
            "error": "No widget snapshot found. Open Ceiling desktop so it can refresh and persist quota.",
            "providers": []
        });
    };

    let entries: Vec<&WidgetProviderEntry> = match provider {
        Some(name) => match ProviderId::from_cli_name(name) {
            Some(id) => snapshot.entry_for(id).into_iter().collect(),
            None => {
                return json!({
                    "ok": false,
                    "error": format!("Unknown provider '{name}'"),
                    "providers": []
                });
            }
        },
        None => snapshot.entries.iter().collect(),
    };

    let providers: Vec<_> = entries.iter().map(|e| entry_usage_json(e)).collect();
    json!({
        "ok": true,
        "source": "widget-snapshot",
        "generated_at": snapshot.generated_at.to_rfc3339(),
        "providers": providers,
    })
}

fn entry_usage_json(entry: &WidgetProviderEntry) -> serde_json::Value {
    json!({
        "provider": entry.provider.cli_name(),
        "display_name": entry.provider.display_name(),
        "updated_at": entry.updated_at.to_rfc3339(),
        "account_email": entry.account_email,
        "login_method": entry.login_method,
        "credits_remaining": entry.credits_remaining,
        "primary": window_json(entry.primary.as_ref()),
        "secondary": window_json(entry.secondary.as_ref()),
        "tertiary": window_json(entry.tertiary.as_ref()),
        "session_cost_usd": entry.token_usage.as_ref().and_then(|t| t.session_cost_usd),
    })
}

fn window_json(window: Option<&RateWindow>) -> serde_json::Value {
    let Some(window) = window else {
        return serde_json::Value::Null;
    };
    json!({
        "used_percent": window.used_percent,
        "remaining_percent": window.remaining_percent(),
        "window_minutes": window.window_minutes,
        "resets_at": window.resets_at.map(|dt| dt.to_rfc3339()),
        "reset_countdown": window.format_countdown(),
        "is_exhausted": window.is_exhausted(),
    })
}

fn spend_payload(provider: Option<&str>) -> serde_json::Value {
    let targets: Vec<&str> = match provider {
        Some(name) => {
            let cli = ProviderId::from_cli_name(name)
                .map(|id| id.cli_name())
                .unwrap_or(name);
            if !COST_SUPPORTED.contains(&cli) {
                return json!({
                    "ok": false,
                    "error": format!(
                        "Local spend scanning is only available for Codex and Claude (got '{cli}')"
                    ),
                    "providers": []
                });
            }
            vec![cli]
        }
        None => COST_SUPPORTED.to_vec(),
    };

    let mut providers = Vec::new();
    for cli in targets {
        match get_cost_usage_report(cli, 30) {
            Some(report) => providers.push(json!({
                "provider": cli,
                "supported": true,
                "note": "Estimated API-rate value from local logs, not a bill.",
                "today": summary_json(&report.today),
                "seven_days": summary_json(&report.seven_days),
                "thirty_days": summary_json(&report.thirty_days),
            })),
            None => providers.push(json!({
                "provider": cli,
                "supported": false,
                "error": "Local cost scanning not available for this provider",
            })),
        }
    }

    json!({
        "ok": true,
        "source": "local-logs",
        "providers": providers,
    })
}

fn summary_json(summary: &CostSummary) -> serde_json::Value {
    let total_tokens = summary.input_tokens
        + summary.output_tokens
        + summary.cache_read_tokens
        + summary.cache_write_tokens;
    json!({
        "total_usd": summary.total_cost_usd,
        "input_tokens": summary.input_tokens,
        "output_tokens": summary.output_tokens,
        "cache_read_tokens": summary.cache_read_tokens,
        "cache_write_tokens": summary.cache_write_tokens,
        "total_tokens": total_tokens,
        "sessions_count": summary.sessions_count,
        "by_model": summary.by_model,
        "unknown_models": summary.unknown_models.iter().cloned().collect::<Vec<_>>(),
        "has_data": summary.sessions_count > 0
            || summary.total_cost_usd > 0.0
            || total_tokens > 0,
    })
}

fn status_payload(snapshot: Option<&WidgetSnapshot>, provider: Option<&str>) -> serde_json::Value {
    let chosen = choose_status_provider(snapshot, provider);
    let usage = match (&chosen, snapshot) {
        (Some(id), Some(snap)) => snap.entry_for(*id).map(entry_usage_json),
        _ => None,
    };

    let spend = chosen.and_then(|id| {
        let cli = id.cli_name();
        if !COST_SUPPORTED.contains(&cli) {
            return None;
        }
        get_cost_usage_report(cli, 30).map(|report| summary_json(&report.today))
    });

    let remaining = usage
        .as_ref()
        .and_then(|u| u.get("primary"))
        .and_then(|p| p.get("remaining_percent"))
        .cloned();

    json!({
        "ok": usage.is_some() || spend.is_some(),
        "provider": chosen.map(|id| id.cli_name()),
        "remaining_percent": remaining,
        "usage": usage,
        "today_spend": spend,
        "hint": if snapshot.is_none() {
            Some("Open Ceiling desktop to refresh quota snapshot.")
        } else {
            None
        },
    })
}

fn choose_status_provider(
    snapshot: Option<&WidgetSnapshot>,
    provider: Option<&str>,
) -> Option<ProviderId> {
    if let Some(name) = provider {
        return ProviderId::from_cli_name(name);
    }
    let snapshot = snapshot?;
    for preferred in [ProviderId::Claude, ProviderId::Codex] {
        if snapshot.entry_for(preferred).is_some() {
            return Some(preferred);
        }
    }
    snapshot.entries.first().map(|e| e.provider)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::RateWindow;
    use chrono::Utc;

    fn sample_snapshot() -> WidgetSnapshot {
        let entry = WidgetProviderEntry::new(ProviderId::Claude, Utc::now())
            .with_primary(RateWindow::with_details(
                42.0,
                Some(300),
                Some(Utc::now() + chrono::Duration::hours(3)),
                Some("in 3h".into()),
            ))
            .with_login_method("Claude Pro");
        WidgetSnapshot::new(vec![entry], Utc::now())
    }

    #[test]
    fn list_providers_marks_snapshot_and_spend_support() {
        let payload = list_providers_payload(Some(&sample_snapshot()));
        assert_eq!(payload["snapshot_present"], true);
        let providers = payload["providers"].as_array().unwrap();
        let claude = providers
            .iter()
            .find(|p| p["id"] == "claude")
            .expect("claude");
        assert_eq!(claude["has_quota_snapshot"], true);
        assert_eq!(claude["local_spend_supported"], true);
        let cursor = providers
            .iter()
            .find(|p| p["id"] == "cursor")
            .expect("cursor");
        assert_eq!(cursor["local_spend_supported"], false);
    }

    #[test]
    fn usage_requires_snapshot() {
        let payload = usage_payload(None, None);
        assert_eq!(payload["ok"], false);
    }

    #[test]
    fn usage_filters_provider() {
        let snap = sample_snapshot();
        let payload = usage_payload(Some(&snap), Some("claude"));
        assert_eq!(payload["ok"], true);
        assert_eq!(payload["providers"].as_array().unwrap().len(), 1);
        assert_eq!(payload["providers"][0]["primary"]["used_percent"], 42.0);
        assert_eq!(
            payload["providers"][0]["primary"]["remaining_percent"],
            58.0
        );
    }

    #[test]
    fn spend_rejects_unsupported_provider() {
        let payload = spend_payload(Some("cursor"));
        assert_eq!(payload["ok"], false);
    }

    #[test]
    fn status_prefers_claude_when_present() {
        let snap = sample_snapshot();
        let payload = status_payload(Some(&snap), None);
        assert_eq!(payload["provider"], "claude");
        assert_eq!(payload["remaining_percent"], 58.0);
    }
}
