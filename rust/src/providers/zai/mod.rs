//! z.ai provider implementation
//!
//! Fetches usage data from z.ai's quota API
//! Uses API token stored in Windows Credential Manager

pub mod mcp_details;

// Re-exports for MCP details menu
#[allow(unused_imports)]
pub use mcp_details::{
    McpDetailsMenu, ZaiLimitEntry, ZaiLimitType, ZaiLimitUnit, ZaiUsageDetail, ZaiUsageSnapshot,
};

use async_trait::async_trait;
use serde::Deserialize;

use crate::core::{
    FetchContext, Provider, ProviderError, ProviderFetchResult, ProviderId, ProviderMetadata,
    RateWindow, SourceMode, UsageSnapshot,
};

/// z.ai API endpoint for quota/usage
const ZAI_API_URL: &str = "https://api.z.ai/api/monitor/usage/quota/limit";

/// Windows Credential Manager target for z.ai API token
const ZAI_CREDENTIAL_TARGET: &str = "codexbar-zai";

/// z.ai quota response structure
#[derive(Debug, Deserialize)]
struct ZaiQuotaResponse {
    #[serde(default)]
    code: Option<i32>,
    #[serde(default)]
    data: Option<ZaiQuotaData>,
    /// Legacy flat limits array (backwards compat)
    #[serde(default)]
    limits: Vec<ZaiLimit>,
}

#[derive(Debug, Deserialize)]
struct ZaiQuotaData {
    #[serde(default)]
    limits: Vec<ZaiLimit>,
    #[serde(rename = "planName")]
    plan_name: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ZaiLimit {
    /// Limit type: "TOKENS_LIMIT" or "TIME_LIMIT" (upstream) or "tokens"/"mcp" (legacy)
    #[serde(rename = "type")]
    limit_type: Option<String>,
    /// Used amount
    used: Option<f64>,
    /// Current value (alternative to used)
    #[serde(rename = "currentValue")]
    current_value: Option<f64>,
    /// Total limit
    limit: Option<f64>,
    /// Remaining amount
    remaining: Option<f64>,
    /// Time unit enum: 1=days, 3=hours, 5=minutes, 6=weeks
    unit: Option<i32>,
    /// Number of time units in the window
    number: Option<i32>,
    /// Reset time (ISO 8601)
    #[serde(rename = "resetAt")]
    reset_at: Option<String>,
}

/// z.ai provider
pub struct ZaiProvider {
    metadata: ProviderMetadata,
}

impl ZaiProvider {
    pub fn new() -> Self {
        Self {
            metadata: ProviderMetadata {
                id: ProviderId::Zai,
                display_name: "z.ai",
                session_label: "Tokens",
                weekly_label: "MCP",
                supports_opus: false,
                supports_credits: true,
                default_enabled: false,
                is_primary: false,
                dashboard_url: Some("https://z.ai/dashboard"),
                status_page_url: None,
            },
        }
    }

    /// Get API token from ctx, Windows Credential Manager, or env
    fn get_api_token(api_key: Option<&str>) -> Result<String, ProviderError> {
        // Check ctx.api_key first (from settings)
        if let Some(key) = api_key
            && !key.is_empty()
        {
            return Ok(key.to_string());
        }

        // Try Windows Credential Manager
        match keyring::Entry::new(ZAI_CREDENTIAL_TARGET, "api_token") {
            Ok(entry) => match entry.get_password() {
                Ok(token) => Ok(token),
                Err(_) => {
                    // Try environment variable as fallback
                    std::env::var("ZAI_API_TOKEN").map_err(|_| {
                        ProviderError::NotInstalled(
                            "z.ai API token not found. Set in Preferences → Providers or ZAI_API_TOKEN environment variable.".to_string()
                        )
                    })
                }
            },
            Err(_) => {
                // Try environment variable as fallback
                std::env::var("ZAI_API_TOKEN").map_err(|_| {
                    ProviderError::NotInstalled(
                        "z.ai API token not found. Set in Preferences → Providers or ZAI_API_TOKEN environment variable.".to_string()
                    )
                })
            }
        }
    }

    /// Fetch usage from z.ai API
    async fn fetch_usage_api(&self, ctx: &FetchContext) -> Result<UsageSnapshot, ProviderError> {
        let api_token = Self::get_api_token(ctx.api_key.as_deref())?;

        let client = crate::core::credentialed_http_client_builder()
            .timeout(std::time::Duration::from_secs(30))
            .build()
            .map_err(|e| ProviderError::Other(e.to_string()))?;

        let resp = client
            .get(ZAI_API_URL)
            .header("Authorization", format!("Bearer {}", api_token))
            .header("Accept", "application/json")
            .send()
            .await?;

        if resp.status() == reqwest::StatusCode::UNAUTHORIZED {
            return Err(ProviderError::AuthRequired);
        }

        if !resp.status().is_success() {
            return Err(ProviderError::Other(format!(
                "z.ai API returned status {}",
                resp.status()
            )));
        }

        let resp_bytes = resp
            .bytes()
            .await
            .map_err(|e| ProviderError::Other(e.to_string()))?;

        // Handle empty response body (can happen with wrong region/endpoint)
        if resp_bytes.is_empty() {
            return Err(ProviderError::Parse(
                "Empty response body from z.ai API. Check API region and token.".to_string(),
            ));
        }

        let quota: ZaiQuotaResponse =
            serde_json::from_slice(&resp_bytes).map_err(|e| ProviderError::Parse(e.to_string()))?;

        self.parse_quota_response(&quota)
    }

    fn parse_quota_response(
        &self,
        quota: &ZaiQuotaResponse,
    ) -> Result<UsageSnapshot, ProviderError> {
        // Get limits from data.limits (upstream) or flat limits (legacy)
        let limits = if let Some(data) = &quota.data {
            &data.limits
        } else {
            &quota.limits
        };
        let plan_name = quota
            .data
            .as_ref()
            .and_then(|d| d.plan_name.as_deref())
            .unwrap_or("z.ai");

        // Collect TOKENS_LIMIT entries (upstream uses "TOKENS_LIMIT", legacy uses "tokens")
        let mut token_limits: Vec<&ZaiLimit> = limits
            .iter()
            .filter(|l| {
                matches!(
                    l.limit_type.as_deref(),
                    Some("TOKENS_LIMIT") | Some("tokens")
                )
            })
            .collect();

        // Find TIME_LIMIT entry (or legacy "mcp")
        let time_limit = limits
            .iter()
            .find(|l| matches!(l.limit_type.as_deref(), Some("TIME_LIMIT") | Some("mcp")));

        // Sort token limits by window_minutes: shortest first
        token_limits.sort_by_key(|l| Self::window_minutes(l));

        // Compute used percent for a limit entry
        fn compute_percent(l: &ZaiLimit) -> f64 {
            let limit = l.limit.unwrap_or(0.0);
            if limit <= 0.0 {
                return if l.used.unwrap_or(0.0) > 0.0 || l.current_value.unwrap_or(0.0) > 0.0 {
                    100.0
                } else {
                    0.0
                };
            }
            let used = {
                let from_remaining = l.remaining.map(|r| limit - r);
                let from_current = l.current_value;
                let from_used = l.used;
                // Use max of available signals
                let candidates = [from_remaining, from_current, from_used];
                candidates.iter().filter_map(|&v| v).fold(0.0_f64, f64::max)
            };
            ((used / limit) * 100.0).clamp(0.0, 100.0)
        }

        fn make_window(l: &ZaiLimit, window_mins: Option<u32>) -> RateWindow {
            RateWindow::with_details(compute_percent(l), window_mins, None, l.reset_at.clone())
        }

        // Build windows based on upstream layout:
        // If 2+ TOKENS_LIMIT: shortest → session (5-hour), longest → weekly (primary)
        // TIME_LIMIT → secondary
        let (primary, secondary, tertiary) = match token_limits.len() {
            0 => {
                // No token limits; use time_limit as primary if available
                let p = time_limit
                    .map(|l| make_window(l, Self::window_minutes(l)))
                    .unwrap_or_else(|| RateWindow::new(0.0));
                (p, None, None)
            }
            1 => {
                let p = make_window(token_limits[0], Self::window_minutes(token_limits[0]));
                let s = time_limit.map(|l| make_window(l, Self::window_minutes(l)));
                (p, s, None)
            }
            _ => {
                // 2+ token limits: longest → primary (weekly), shortest → tertiary (5-hour)
                let weekly = token_limits.last().unwrap();
                let session = token_limits.first().unwrap();
                let p = make_window(weekly, Self::window_minutes(weekly));
                let s = time_limit.map(|l| make_window(l, Self::window_minutes(l)));
                let t = Some(make_window(session, Self::window_minutes(session)));
                (p, s, t)
            }
        };

        let mut usage = UsageSnapshot::new(primary).with_login_method(plan_name);
        if let Some(sec) = secondary {
            usage = usage.with_secondary(sec);
        }
        if let Some(ter) = tertiary {
            usage = usage.with_model_specific(ter);
        }

        Ok(usage)
    }

    /// Compute window_minutes from a limit's unit + number fields
    fn window_minutes(l: &ZaiLimit) -> Option<u32> {
        let unit = l.unit?;
        let number = l.number.unwrap_or(1) as u32;
        let minutes_per_unit = match unit {
            1 => 1440,  // days
            3 => 60,    // hours
            5 => 1,     // minutes
            6 => 10080, // weeks
            _ => return None,
        };
        Some(number * minutes_per_unit)
    }
}

impl Default for ZaiProvider {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Provider for ZaiProvider {
    fn id(&self) -> ProviderId {
        ProviderId::Zai
    }

    fn metadata(&self) -> &ProviderMetadata {
        &self.metadata
    }

    async fn fetch_usage(&self, ctx: &FetchContext) -> Result<ProviderFetchResult, ProviderError> {
        tracing::debug!("Fetching z.ai usage");

        // z.ai only supports OAuth/API token - no CLI or web cookie fallback
        match ctx.source_mode {
            SourceMode::Auto | SourceMode::OAuth => {
                let usage = self.fetch_usage_api(ctx).await?;
                Ok(ProviderFetchResult::new(usage, "oauth"))
            }
            SourceMode::Web | SourceMode::Cli => {
                // z.ai doesn't support web cookies or CLI
                Err(ProviderError::UnsupportedSource(ctx.source_mode))
            }
        }
    }

    fn available_sources(&self) -> Vec<SourceMode> {
        vec![SourceMode::Auto, SourceMode::OAuth]
    }

    fn supports_web(&self) -> bool {
        false
    }

    fn supports_cli(&self) -> bool {
        false
    }
}
