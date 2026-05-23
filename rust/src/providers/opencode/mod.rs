//! OpenCode provider implementation
//!
//! Fetches usage data from OpenCode (opencode.ai)
//! Uses browser cookies for authentication

pub mod scraper;

// Re-exports for advanced scraping
#[allow(unused_imports)]
pub use scraper::{OpenCodeError, OpenCodeUsageFetcher, OpenCodeUsageSnapshot};

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use reqwest::Client;
use serde_json::Value;
use uuid::Uuid;

use crate::core::{
    FetchContext, Provider, ProviderError, ProviderFetchResult, ProviderId, ProviderMetadata,
    RateWindow, SourceMode, UsageSnapshot,
};

const BASE_URL: &str = "https://opencode.ai";
const SERVER_URL: &str = "https://opencode.ai/_server";
const WORKSPACES_SERVER_ID: &str =
    "def39973159c7f0483d8793a822b8dbb10d067e12c65455fcb4608459ba0234f";
const SUBSCRIPTION_SERVER_ID: &str =
    "7abeebee372f304e050aaaf92be863f4a86490e382f8c79db68fd94040d691b4";

/// OpenCode provider
pub struct OpenCodeProvider {
    metadata: ProviderMetadata,
    client: Client,
}

impl OpenCodeProvider {
    pub fn new() -> Self {
        Self {
            metadata: ProviderMetadata {
                id: ProviderId::OpenCode,
                display_name: "OpenCode",
                session_label: "5-hour",
                weekly_label: "Weekly",
                supports_opus: false,
                supports_credits: false,
                default_enabled: false,
                is_primary: false,
                dashboard_url: Some("https://opencode.ai"),
                status_page_url: None,
            },
            client: Client::builder()
                .timeout(std::time::Duration::from_secs(30))
                .build()
                .unwrap_or_else(|_| Client::new()),
        }
    }

    /// Fetch usage with cookie header
    async fn fetch_with_cookies(
        &self,
        cookie_header: &str,
    ) -> Result<UsageSnapshot, ProviderError> {
        // First get workspace ID
        let workspace_id = self.fetch_workspace_id(cookie_header).await?;

        // Then fetch subscription info
        let subscription = self
            .fetch_subscription(&workspace_id, cookie_header)
            .await?;

        // Parse the response
        self.parse_subscription(&subscription)
    }

    /// Fetch workspace ID from server
    async fn fetch_workspace_id(&self, cookie_header: &str) -> Result<String, ProviderError> {
        let url = format!("{}?id={}", SERVER_URL, WORKSPACES_SERVER_ID);

        let response = self
            .client
            .get(&url)
            .header("Cookie", cookie_header)
            .header("X-Server-Id", WORKSPACES_SERVER_ID)
            .header("X-Server-Instance", format!("server-fn:{}", Uuid::new_v4()))
            .header(
                "User-Agent",
                "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36",
            )
            .header("Origin", BASE_URL)
            .header("Referer", BASE_URL)
            .header(
                "Accept",
                "text/javascript, application/json;q=0.9, */*;q=0.8",
            )
            .send()
            .await?;

        if !response.status().is_success() {
            if response.status().as_u16() == 401 || response.status().as_u16() == 403 {
                return Err(ProviderError::AuthRequired);
            }
            return Err(ProviderError::Other(format!(
                "OpenCode API returned {}",
                response.status()
            )));
        }

        let text = response.text().await?;

        // Check for sign-out indicators
        if self.looks_signed_out(&text) {
            return Err(ProviderError::AuthRequired);
        }

        // Parse workspace IDs
        let ids = self.parse_workspace_ids(&text);
        if ids.is_empty() {
            return Err(ProviderError::Parse("No workspace ID found".to_string()));
        }

        Ok(ids[0].clone())
    }

    /// Fetch subscription info for a workspace
    async fn fetch_subscription(
        &self,
        workspace_id: &str,
        cookie_header: &str,
    ) -> Result<String, ProviderError> {
        let referer = format!("https://opencode.ai/workspace/{}/billing", workspace_id);
        let args = serde_json::json!([workspace_id]);
        let encoded_args = Self::url_encode(&args.to_string());
        let url = format!(
            "{}?id={}&args={}",
            SERVER_URL, SUBSCRIPTION_SERVER_ID, encoded_args
        );

        let response = self
            .client
            .get(&url)
            .header("Cookie", cookie_header)
            .header("X-Server-Id", SUBSCRIPTION_SERVER_ID)
            .header("X-Server-Instance", format!("server-fn:{}", Uuid::new_v4()))
            .header(
                "User-Agent",
                "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36",
            )
            .header("Origin", BASE_URL)
            .header("Referer", referer)
            .header(
                "Accept",
                "text/javascript, application/json;q=0.9, */*;q=0.8",
            )
            .send()
            .await?;

        if !response.status().is_success() {
            if response.status().as_u16() == 401 || response.status().as_u16() == 403 {
                return Err(ProviderError::AuthRequired);
            }
            return Err(ProviderError::Other(format!(
                "OpenCode subscription API returned {}",
                response.status()
            )));
        }

        let text = response.text().await?;

        if self.looks_signed_out(&text) {
            return Err(ProviderError::AuthRequired);
        }

        Ok(text)
    }

    /// Parse subscription response into UsageSnapshot
    fn parse_subscription(&self, text: &str) -> Result<UsageSnapshot, ProviderError> {
        let now = Utc::now();

        // Try to parse as JSON
        if let Ok(json) = serde_json::from_str::<Value>(text)
            && let Some(snapshot) = self.parse_usage_json(&json, now)
        {
            return Ok(snapshot);
        }

        // Fall back to regex-based parsing
        let rolling = self.extract_usage_regex(text, "rollingUsage")?;
        let weekly = self.extract_usage_regex(text, "weeklyUsage")?;

        let primary = RateWindow::with_details(
            rolling.0,
            Some(300), // 5 hours
            Some(now + chrono::Duration::seconds(rolling.1)),
            None,
        );

        let secondary = RateWindow::with_details(
            weekly.0,
            Some(10080), // 7 days
            Some(now + chrono::Duration::seconds(weekly.1)),
            None,
        );

        let usage = UsageSnapshot::new(primary)
            .with_secondary(secondary)
            .with_login_method("OpenCode");

        Ok(usage)
    }

    /// Parse usage from JSON response
    fn parse_usage_json(&self, json: &Value, now: DateTime<Utc>) -> Option<UsageSnapshot> {
        // Look for rollingUsage and weeklyUsage
        let rolling =
            self.find_usage_window(json, &["rollingUsage", "rolling", "rolling_usage"])?;
        let weekly = self.find_usage_window(json, &["weeklyUsage", "weekly", "weekly_usage"])?;

        let primary = RateWindow::with_details(
            rolling.0,
            Some(300),
            Some(now + chrono::Duration::seconds(rolling.1)),
            None,
        );

        let secondary = RateWindow::with_details(
            weekly.0,
            Some(10080),
            Some(now + chrono::Duration::seconds(weekly.1)),
            None,
        );

        let usage = UsageSnapshot::new(primary)
            .with_secondary(secondary)
            .with_login_method("OpenCode");

        Some(usage)
    }

    /// Find usage window in JSON by keys
    fn find_usage_window(&self, json: &Value, keys: &[&str]) -> Option<(f64, i64)> {
        for key in keys {
            if let Some(obj) = json.get(key)
                && let Some(window) = self.parse_window(obj)
            {
                return Some(window);
            }
        }

        // Try nested search
        if let Some(obj) = json.as_object() {
            for (_, value) in obj {
                if let Some(window) = self.find_usage_window(value, keys) {
                    return Some(window);
                }
            }
        }

        None
    }

    /// Parse a usage window object
    fn parse_window(&self, obj: &Value) -> Option<(f64, i64)> {
        let percent = Self::window_percent(obj)?;
        let reset_sec = Self::window_reset_seconds(obj).unwrap_or(0);
        Some((percent.clamp(0.0, 100.0), reset_sec.max(0)))
    }

    fn window_percent(obj: &Value) -> Option<f64> {
        let percent_keys = [
            "usagePercent",
            "usedPercent",
            "percentUsed",
            "percent",
            "usage_percent",
            "used_percent",
            "utilization",
            "utilizationPercent",
            "utilization_percent",
            "usage",
        ];

        Self::first_f64(obj, &percent_keys)
            .map(|val| if val <= 1.0 { val * 100.0 } else { val })
            .or_else(|| Self::percent_from_used_limit(obj))
    }

    fn percent_from_used_limit(obj: &Value) -> Option<f64> {
        let used = obj
            .get("used")
            .or(obj.get("usage"))
            .and_then(|v| v.as_f64());
        let limit = obj
            .get("limit")
            .or(obj.get("total"))
            .and_then(|v| v.as_f64());
        match (used, limit) {
            (Some(used), Some(limit)) if limit > 0.0 => Some((used / limit) * 100.0),
            _ => None,
        }
    }

    fn window_reset_seconds(obj: &Value) -> Option<i64> {
        let reset_in_keys = [
            "resetInSec",
            "resetInSeconds",
            "resetSeconds",
            "reset_sec",
            "reset_in_sec",
            "resetsInSec",
            "resetsInSeconds",
            "resetIn",
            "resetSec",
        ];
        let reset_at_keys = [
            "resetAt",
            "resetsAt",
            "reset_at",
            "resets_at",
            "nextReset",
            "next_reset",
            "renewAt",
            "renew_at",
        ];

        Self::first_i64(obj, &reset_in_keys)
            .or_else(|| Self::reset_at_to_seconds(obj, &reset_at_keys))
    }

    fn reset_at_to_seconds(obj: &Value, keys: &[&str]) -> Option<i64> {
        let reset_at = Self::first_i64(obj, keys)?;
        let now = chrono::Utc::now().timestamp();
        Some((reset_at - now).max(0))
    }

    fn first_f64(obj: &Value, keys: &[&str]) -> Option<f64> {
        keys.iter().find_map(|key| obj.get(*key)?.as_f64())
    }

    fn first_i64(obj: &Value, keys: &[&str]) -> Option<i64> {
        keys.iter().find_map(|key| obj.get(*key)?.as_i64())
    }

    /// Extract usage via regex patterns
    fn extract_usage_regex(&self, text: &str, prefix: &str) -> Result<(f64, i64), ProviderError> {
        let percent_pattern = format!(r"{}[^}}]*?usagePercent\s*:\s*([0-9]+(?:\.[0-9]+)?)", prefix);
        let reset_pattern = format!(r"{}[^}}]*?resetInSec\s*:\s*([0-9]+)", prefix);

        let percent = self
            .extract_number(&percent_pattern, text)
            .ok_or_else(|| ProviderError::Parse(format!("Missing {} percent", prefix)))?;

        let reset = self
            .extract_number(&reset_pattern, text)
            .map(|n| n as i64)
            .unwrap_or(0);

        Ok((percent, reset))
    }

    /// Extract a number using regex
    fn extract_number(&self, pattern: &str, text: &str) -> Option<f64> {
        let re = regex_lite::Regex::new(pattern).ok()?;
        let caps = re.captures(text)?;
        caps.get(1)?.as_str().parse().ok()
    }

    /// Parse workspace IDs from response
    fn parse_workspace_ids(&self, text: &str) -> Vec<String> {
        let pattern = r#"id\s*:\s*"(wrk_[^"]+)""#;
        let re = match regex_lite::Regex::new(pattern) {
            Ok(r) => r,
            Err(_) => return vec![],
        };

        re.captures_iter(text)
            .filter_map(|caps| caps.get(1).map(|m| m.as_str().to_string()))
            .collect()
    }

    /// Check if response indicates user is signed out
    fn looks_signed_out(&self, text: &str) -> bool {
        let lower = text.to_lowercase();
        lower.contains("login") || lower.contains("sign in") || lower.contains("auth/authorize")
    }

    /// URL encode a string for query parameters
    fn url_encode(s: &str) -> String {
        let mut result = String::with_capacity(s.len() * 3);
        for c in s.chars() {
            match c {
                'a'..='z' | 'A'..='Z' | '0'..='9' | '-' | '_' | '.' | '~' => {
                    result.push(c);
                }
                _ => {
                    for b in c.to_string().as_bytes() {
                        result.push_str(&format!("%{:02X}", b));
                    }
                }
            }
        }
        result
    }
}

impl Default for OpenCodeProvider {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Provider for OpenCodeProvider {
    fn id(&self) -> ProviderId {
        ProviderId::OpenCode
    }

    fn metadata(&self) -> &ProviderMetadata {
        &self.metadata
    }

    async fn fetch_usage(&self, ctx: &FetchContext) -> Result<ProviderFetchResult, ProviderError> {
        tracing::debug!("Fetching OpenCode usage");

        match ctx.source_mode {
            SourceMode::Auto | SourceMode::Web => {
                // Check for manual cookie header first
                if let Some(ref cookie_header) = ctx.manual_cookie_header {
                    let usage = self.fetch_with_cookies(cookie_header).await?;
                    return Ok(ProviderFetchResult::new(usage, "web"));
                }

                // Try to get cookies from browser
                #[cfg(windows)]
                {
                    use crate::browser::cookies::{Cookie, CookieExtractor};
                    use crate::browser::detection::BrowserDetector;

                    let browsers = BrowserDetector::detect_all();

                    for browser in browsers {
                        if let Ok(cookies) =
                            CookieExtractor::extract_for_domain(&browser, "opencode.ai")
                        {
                            // Build cookie header
                            let cookie_header: String = cookies
                                .iter()
                                .map(|c: &Cookie| format!("{}={}", c.name, c.value))
                                .collect::<Vec<_>>()
                                .join("; ");

                            if !cookie_header.is_empty() {
                                match self.fetch_with_cookies(&cookie_header).await {
                                    Ok(usage) => return Ok(ProviderFetchResult::new(usage, "web")),
                                    Err(ProviderError::AuthRequired) => continue,
                                    Err(e) => return Err(e),
                                }
                            }
                        }
                    }
                }

                Err(ProviderError::AuthRequired)
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
