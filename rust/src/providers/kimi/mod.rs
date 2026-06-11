//! Kimi AI provider implementation
//!
//! Fetches usage data from Kimi (Moonshot AI)
//! Uses JWT from kimi-auth cookie for authentication
//! Tracks weekly quota + 5-hour rate limit

use async_trait::async_trait;

use crate::browser::cookies::get_cookie_header;
use crate::core::{
    FetchContext, Provider, ProviderError, ProviderFetchResult, ProviderId, ProviderMetadata,
    RateWindow, SourceMode, UsageSnapshot,
};

const KIMI_API_BASE: &str = "https://kimi.moonshot.cn";
const KIMI_COOKIE_DOMAIN: &str = "kimi.moonshot.cn";

/// Kimi AI provider
pub struct KimiProvider {
    metadata: ProviderMetadata,
}

impl KimiProvider {
    pub fn new() -> Self {
        Self {
            metadata: ProviderMetadata {
                id: ProviderId::Kimi,
                display_name: "Kimi",
                session_label: "Weekly",
                weekly_label: "Rate Limit",
                supports_opus: false,
                supports_credits: false,
                default_enabled: false,
                is_primary: false,
                dashboard_url: Some("https://kimi.moonshot.cn"),
                status_page_url: None,
            },
        }
    }

    /// Extract JWT token from kimi-auth cookie
    fn get_auth_token(&self) -> Result<String, ProviderError> {
        // Try to get cookies from browser
        let cookies = get_cookie_header(KIMI_COOKIE_DOMAIN)
            .map_err(|e| ProviderError::Other(format!("Failed to get cookies: {}", e)))?;

        if cookies.is_empty() {
            return Err(ProviderError::AuthRequired);
        }

        // Look for the kimi-auth or authorization cookie
        for cookie in cookies.split(';') {
            let cookie = cookie.trim();
            if cookie.starts_with("kimi-auth=") || cookie.starts_with("authorization=") {
                let token = cookie.split('=').nth(1).unwrap_or("");
                if !token.is_empty() {
                    return Ok(token.to_string());
                }
            }
        }

        // Also check for access_token cookie
        for cookie in cookies.split(';') {
            let cookie = cookie.trim();
            if cookie.starts_with("access_token=") {
                let token = cookie.split('=').nth(1).unwrap_or("");
                if !token.is_empty() {
                    return Ok(token.to_string());
                }
            }
        }

        Err(ProviderError::AuthRequired)
    }

    /// Fetch usage via Kimi web API
    async fn fetch_via_web(&self) -> Result<UsageSnapshot, ProviderError> {
        let token = self.get_auth_token()?;

        let client = crate::core::credentialed_http_client_builder()
            .timeout(std::time::Duration::from_secs(30))
            .build()
            .map_err(|e| ProviderError::Other(e.to_string()))?;

        // Fetch user profile/quota info
        let resp = client
            .get(format!("{}/api/user", KIMI_API_BASE))
            .header("Authorization", format!("Bearer {}", token))
            .header("Cookie", format!("kimi-auth={}", token))
            .header("Accept", "application/json")
            .header(
                "User-Agent",
                "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36",
            )
            .send()
            .await?;

        if !resp.status().is_success() {
            let status = resp.status();
            if status.as_u16() == 401 || status.as_u16() == 403 {
                return Err(ProviderError::AuthRequired);
            }
            return Err(ProviderError::Other(format!("API error: {}", status)));
        }

        let json: serde_json::Value = resp
            .json()
            .await
            .map_err(|e| ProviderError::Parse(e.to_string()))?;

        self.parse_usage_response(&json)
    }

    /// Parse Kimi usage response
    fn parse_usage_response(
        &self,
        json: &serde_json::Value,
    ) -> Result<UsageSnapshot, ProviderError> {
        // Extract quota information
        // Kimi typically has: daily/weekly limits and 5-hour rate limits

        let quota = json.get("quota").or_else(|| json.get("usage"));

        // 5-hour rate limit (session-like)
        let five_hour_used = quota
            .and_then(|q| q.get("rate_limit_used").or_else(|| q.get("five_hour_used")))
            .and_then(|v| v.as_f64())
            .unwrap_or(0.0);

        let five_hour_limit = quota
            .and_then(|q| {
                q.get("rate_limit_total")
                    .or_else(|| q.get("five_hour_limit"))
            })
            .and_then(|v| v.as_f64())
            .unwrap_or(100.0);

        let five_hour_percent = if five_hour_limit > 0.0 {
            (five_hour_used / five_hour_limit) * 100.0
        } else {
            0.0
        };

        // Weekly quota
        let weekly_used = quota
            .and_then(|q| q.get("weekly_used").or_else(|| q.get("week_used")))
            .and_then(|v| v.as_f64())
            .unwrap_or(0.0);

        let weekly_limit = quota
            .and_then(|q| q.get("weekly_limit").or_else(|| q.get("week_limit")))
            .and_then(|v| v.as_f64())
            .unwrap_or(1000.0);

        let weekly_percent = if weekly_limit > 0.0 {
            (weekly_used / weekly_limit) * 100.0
        } else {
            0.0
        };

        // Get user info
        let nickname = json
            .get("nickname")
            .or_else(|| json.get("name"))
            .and_then(|v| v.as_str());

        let plan = json
            .get("vip_type")
            .or_else(|| json.get("plan"))
            .and_then(|v| v.as_str())
            .unwrap_or("Kimi");

        // Create primary rate window (weekly quota - more important for planning)
        let primary = RateWindow::new(weekly_percent);

        // Create secondary rate window (5-hour rate limit)
        let mut rate_limit = RateWindow::new(five_hour_percent);

        // Try to parse resetTime / reset_time from the response; fall back to 5h from now.
        let resets_at = quota
            .and_then(|q| q.get("resetTime").or_else(|| q.get("reset_time")))
            .and_then(|v| {
                if let Some(s) = v.as_str() {
                    chrono::DateTime::parse_from_rfc3339(s)
                        .map(|dt| dt.with_timezone(&chrono::Utc))
                        .ok()
                } else {
                    v.as_i64().map(|ts| {
                        chrono::DateTime::from_timestamp(ts, 0)
                            .unwrap_or_else(|| chrono::Utc::now() + chrono::Duration::hours(5))
                    })
                }
            })
            .unwrap_or_else(|| chrono::Utc::now() + chrono::Duration::hours(5));

        rate_limit.resets_at = Some(resets_at);

        // Try to parse windowMinutes / window_minutes; fall back to 300 (5 hours).
        let window_minutes = quota
            .and_then(|q| q.get("windowMinutes").or_else(|| q.get("window_minutes")))
            .and_then(|v| v.as_i64())
            .unwrap_or(300);

        rate_limit.window_minutes = Some(window_minutes as u32);

        let mut usage = UsageSnapshot::new(primary).with_login_method(plan);

        // Only add rate limit as secondary if we actually have rate limit data
        if five_hour_limit > 0.0 {
            usage = usage.with_secondary(rate_limit);
        }

        if let Some(name) = nickname {
            usage = usage.with_email(name.to_string());
        }

        Ok(usage)
    }
}

impl Default for KimiProvider {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Provider for KimiProvider {
    fn id(&self) -> ProviderId {
        ProviderId::Kimi
    }

    fn metadata(&self) -> &ProviderMetadata {
        &self.metadata
    }

    async fn fetch_usage(&self, ctx: &FetchContext) -> Result<ProviderFetchResult, ProviderError> {
        tracing::debug!("Fetching Kimi usage");

        match ctx.source_mode {
            SourceMode::Auto | SourceMode::Web => {
                let usage = self.fetch_via_web().await?;
                Ok(ProviderFetchResult::new(usage, "web"))
            }
            SourceMode::Cli => Err(ProviderError::UnsupportedSource(SourceMode::Cli)),
            SourceMode::OAuth => Err(ProviderError::UnsupportedSource(SourceMode::OAuth)),
        }
    }

    fn available_sources(&self) -> Vec<SourceMode> {
        vec![SourceMode::Auto, SourceMode::Web]
    }

    fn supports_web(&self) -> bool {
        true
    }

    fn supports_cli(&self) -> bool {
        false
    }
}
