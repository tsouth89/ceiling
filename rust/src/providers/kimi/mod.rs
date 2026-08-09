//! Kimi AI provider implementation
//!
//! Fetches usage data from Kimi (Moonshot AI)
//! Uses JWT from kimi-auth cookie for authentication
//! Tracks weekly quota + 5-hour rate limit

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use reqwest::{Client, Url};
use serde::Deserialize;

use crate::core::{
    FetchContext, Provider, ProviderError, ProviderFetchResult, ProviderId, ProviderMetadata,
    RateWindow, SourceMode, UsageSnapshot,
};

const KIMI_WEB_USAGE_URL: &str =
    "https://www.kimi.com/apiv2/kimi.gateway.billing.v1.BillingService/GetUsages";
const KIMI_SUBSCRIPTION_STATS_URL: &str =
    "https://www.kimi.com/apiv2/kimi.gateway.membership.v2.MembershipService/GetSubscriptionStats";
const KIMI_CODE_API_BASE: &str = "https://api.kimi.com";
const KIMI_CODE_API_KEY_ENV: &str = "KIMI_CODE_API_KEY";
const KIMI_CODE_BASE_URL_ENV: &str = "KIMI_CODE_BASE_URL";

#[derive(Debug, Deserialize)]
struct KimiCodeApiUsageResponse {
    usage: KimiUsageDetail,
    #[serde(default)]
    limits: Option<Vec<KimiRateLimit>>,
}

#[derive(Debug, Deserialize)]
struct KimiWebUsageResponse {
    usages: Vec<KimiUsage>,
}

#[derive(Debug, Deserialize)]
struct KimiUsage {
    scope: String,
    detail: KimiUsageDetail,
    #[serde(default)]
    limits: Option<Vec<KimiRateLimit>>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct KimiSubscriptionStatsResponse {
    subscription_balance: Option<KimiSubscriptionBalance>,
    ratelimit_code7d: Option<KimiSubscriptionRateLimit>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct KimiSubscriptionBalance {
    amount_used_ratio: Option<serde_json::Value>,
    expire_time: Option<serde_json::Value>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct KimiSubscriptionRateLimit {
    ratio: Option<serde_json::Value>,
    enabled: Option<bool>,
    reset_time: Option<serde_json::Value>,
}

#[derive(Debug, Deserialize)]
struct KimiUsageDetail {
    #[serde(default)]
    limit: Option<serde_json::Value>,
    #[serde(default)]
    used: Option<serde_json::Value>,
    #[serde(default)]
    remaining: Option<serde_json::Value>,
    #[serde(
        default,
        rename = "resetTime",
        alias = "resetAt",
        alias = "reset_time",
        alias = "reset_at"
    )]
    reset_time: Option<serde_json::Value>,
}

#[derive(Debug, Deserialize)]
struct KimiRateLimit {
    window: Option<KimiWindow>,
    detail: KimiUsageDetail,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct KimiWindow {
    duration: u32,
    time_unit: String,
}

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

    fn auth_token_from_cookie_headers(
        headers: impl IntoIterator<Item = impl AsRef<str>>,
    ) -> Result<String, ProviderError> {
        for header in headers {
            if let Ok(token) = Self::auth_token_from_cookie_header(header.as_ref()) {
                return Ok(token);
            }
        }
        Err(ProviderError::AuthRequired)
    }

    fn auth_token_from_cookie_header(cookie_header: &str) -> Result<String, ProviderError> {
        for cookie in cookie_header.split(';') {
            let cookie = cookie.trim();
            if cookie.starts_with("kimi-auth=")
                || cookie.starts_with("authorization=")
                || cookie.starts_with("access_token=")
            {
                let token = cookie.split('=').nth(1).unwrap_or("").trim();
                if !token.is_empty() {
                    return Ok(token.to_string());
                }
            }
        }
        Err(ProviderError::AuthRequired)
    }

    /// Fetch usage via Kimi web API
    async fn fetch_via_web(
        &self,
        cookie_header: Option<&str>,
    ) -> Result<UsageSnapshot, ProviderError> {
        let token = match cookie_header {
            Some(header) if !header.trim().is_empty() => {
                Self::auth_token_from_cookie_header(header)
            }
            _ => Err(ProviderError::AuthRequired),
        }?;

        let client = crate::core::credentialed_http_client_builder()
            .timeout(std::time::Duration::from_secs(30))
            .build()
            .map_err(|e| ProviderError::Other(e.to_string()))?;

        let resp = kimi_web_post(
            &client,
            KIMI_WEB_USAGE_URL,
            &token,
            serde_json::json!({ "scope": ["FEATURE_CODING"] }),
        )
        .await?;

        if !resp.status().is_success() {
            let status = resp.status();
            if status.as_u16() == 401 || status.as_u16() == 403 {
                return Err(ProviderError::AuthRequired);
            }
            return Err(ProviderError::Other(format!("API error: {}", status)));
        }

        let usage: KimiWebUsageResponse = resp
            .json()
            .await
            .map_err(|e| ProviderError::Parse(e.to_string()))?;

        let subscription = match kimi_web_post(
            &client,
            KIMI_SUBSCRIPTION_STATS_URL,
            &token,
            serde_json::json!({}),
        )
        .await
        {
            Ok(response) if response.status().is_success() => response.json().await.ok(),
            _ => None,
        };

        Self::snapshot_from_web_usage_response(usage, subscription)
    }

    async fn fetch_via_code_api(
        &self,
        api_key: Option<&str>,
    ) -> Result<UsageSnapshot, ProviderError> {
        let api_key = Self::code_api_key(api_key)?;
        let base_url = Self::code_api_base_url()?;
        let endpoint = Self::code_api_usage_endpoint(&base_url)?;
        let client = crate::core::credentialed_http_client_builder()
            .timeout(std::time::Duration::from_secs(30))
            .build()
            .map_err(|e| ProviderError::Other(e.to_string()))?;

        let resp = client
            .get(endpoint)
            .header("Authorization", format!("Bearer {api_key}"))
            .header("Accept", "application/json")
            .send()
            .await?;

        if resp.status() == reqwest::StatusCode::UNAUTHORIZED {
            return Err(ProviderError::AuthRequired);
        }
        if !resp.status().is_success() {
            return Err(ProviderError::Other(format!(
                "Kimi Code API returned status {}",
                resp.status()
            )));
        }

        let json: KimiCodeApiUsageResponse = resp.json().await.map_err(|e| {
            ProviderError::Parse(format!("Failed to parse Kimi Code API response: {e}"))
        })?;
        Self::snapshot_from_code_api_response(json)
    }

    fn code_api_key(explicit: Option<&str>) -> Result<String, ProviderError> {
        if let Some(key) = explicit.map(str::trim).filter(|key| !key.is_empty()) {
            return Ok(key.to_string());
        }
        std::env::var(KIMI_CODE_API_KEY_ENV)
            .map(|key| key.trim().to_string())
            .ok()
            .filter(|key| !key.is_empty())
            .ok_or(ProviderError::AuthRequired)
    }

    fn code_api_base_url() -> Result<Url, ProviderError> {
        let raw = std::env::var(KIMI_CODE_BASE_URL_ENV)
            .ok()
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| KIMI_CODE_API_BASE.to_string());
        crate::providers::validated_https_url(&raw, "Kimi Code API base")
    }

    fn code_api_usage_endpoint(base_url: &Url) -> Result<Url, ProviderError> {
        let base = base_url.as_str().trim_end_matches('/');
        let path = base_url.path().trim_matches('/');
        let endpoint = if path == "coding/v1" || path.ends_with("/coding/v1") {
            format!("{base}/usages")
        } else if path == "coding" || path.ends_with("/coding") {
            format!("{base}/v1/usages")
        } else {
            format!("{base}/coding/v1/usages")
        };
        Url::parse(&endpoint)
            .map_err(|_| ProviderError::Other("Kimi Code API usage endpoint is invalid".into()))
    }

    fn snapshot_from_code_api_response(
        response: KimiCodeApiUsageResponse,
    ) -> Result<UsageSnapshot, ProviderError> {
        let primary = Self::rate_window_from_usage_detail(&response.usage, None)?;
        let mut usage = UsageSnapshot::new(primary).with_login_method("Code API");

        if let Some(limit) = response.limits.unwrap_or_default().into_iter().next() {
            let window_minutes = limit.window.as_ref().and_then(kimi_window_minutes);
            let rate_limit = Self::rate_window_from_usage_detail(&limit.detail, window_minutes)?;
            usage = usage.with_secondary(rate_limit);
        }

        Ok(usage)
    }

    fn snapshot_from_web_usage_response(
        response: KimiWebUsageResponse,
        subscription: Option<KimiSubscriptionStatsResponse>,
    ) -> Result<UsageSnapshot, ProviderError> {
        let coding = response
            .usages
            .into_iter()
            .find(|usage| usage.scope == "FEATURE_CODING")
            .ok_or_else(|| ProviderError::Parse("Kimi FEATURE_CODING usage missing".into()))?;
        let primary = Self::rate_window_from_usage_detail(&coding.detail, Some(10080))?;
        let mut usage = UsageSnapshot::new(primary).with_login_method("Kimi");

        if let Some(limit) = coding.limits.unwrap_or_default().into_iter().next() {
            let window_minutes = limit.window.as_ref().and_then(kimi_window_minutes);
            let rate_limit = Self::rate_window_from_usage_detail(&limit.detail, window_minutes)?;
            usage = usage.with_secondary(rate_limit);
        }

        if let Some(subscription) = subscription {
            if let Some(balance) = subscription.subscription_balance
                && let Some(ratio) =
                    value_as_f64(balance.amount_used_ratio.as_ref()).filter(|v| v.is_finite())
            {
                usage = usage.with_extra_rate_window(
                    "kimi-monthly",
                    "Monthly",
                    RateWindow::with_details(
                        ratio * 100.0,
                        None,
                        balance.expire_time.as_ref().and_then(parse_kimi_timestamp),
                        None,
                    ),
                );
            }

            if let Some(limit) = subscription.ratelimit_code7d
                && limit.enabled.unwrap_or(true)
                && let Some(ratio) = value_as_f64(limit.ratio.as_ref()).filter(|v| v.is_finite())
            {
                usage = usage.with_extra_rate_window(
                    "kimi-code-7d",
                    "Code 7-day",
                    RateWindow::with_details(
                        ratio * 100.0,
                        Some(10080),
                        limit.reset_time.as_ref().and_then(parse_kimi_timestamp),
                        None,
                    ),
                );
            }
        }

        Ok(usage)
    }

    fn rate_window_from_usage_detail(
        detail: &KimiUsageDetail,
        window_minutes: Option<u32>,
    ) -> Result<RateWindow, ProviderError> {
        let limit = value_as_f64(detail.limit.as_ref())
            .filter(|limit| *limit > 0.0)
            .ok_or_else(|| ProviderError::Parse("Kimi usage limit missing".into()))?;
        let used = match (
            value_as_f64(detail.used.as_ref()),
            value_as_f64(detail.remaining.as_ref()),
        ) {
            (Some(used), _) => used,
            (None, Some(remaining)) => (limit - remaining).max(0.0),
            (None, None) => {
                return Err(ProviderError::Parse(
                    "Kimi usage used/remaining value missing".into(),
                ));
            }
        };
        let reset_at = detail.reset_time.as_ref().and_then(parse_kimi_timestamp);
        let description = Some(format!(
            "{}/{} credits",
            format_usage_amount(used),
            format_usage_amount(limit)
        ));

        Ok(RateWindow::with_details(
            (used / limit) * 100.0,
            window_minutes,
            reset_at,
            description,
        ))
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
            SourceMode::Auto => {
                if Self::code_api_key(ctx.api_key.as_deref()).is_ok() {
                    let usage = self.fetch_via_code_api(ctx.api_key.as_deref()).await?;
                    Ok(ProviderFetchResult::new(usage, "code-api"))
                } else {
                    let usage = self
                        .fetch_via_web(ctx.manual_cookie_header.as_deref())
                        .await?;
                    Ok(ProviderFetchResult::new(usage, "web"))
                }
            }
            SourceMode::OAuth => {
                let usage = self.fetch_via_code_api(ctx.api_key.as_deref()).await?;
                Ok(ProviderFetchResult::new(usage, "code-api"))
            }
            SourceMode::Web => {
                let usage = self
                    .fetch_via_web(ctx.manual_cookie_header.as_deref())
                    .await?;
                Ok(ProviderFetchResult::new(usage, "web"))
            }
            SourceMode::Cli => Err(ProviderError::UnsupportedSource(SourceMode::Cli)),
        }
    }

    fn available_sources(&self) -> Vec<SourceMode> {
        vec![SourceMode::Auto, SourceMode::Web, SourceMode::OAuth]
    }

    fn supports_web(&self) -> bool {
        true
    }

    fn supports_cli(&self) -> bool {
        false
    }

    fn supports_oauth(&self) -> bool {
        true
    }
}

fn kimi_window_minutes(window: &KimiWindow) -> Option<u32> {
    let unit = window
        .time_unit
        .trim()
        .trim_start_matches("TIME_UNIT_")
        .to_ascii_lowercase();
    match unit.as_str() {
        "second" | "seconds" => Some((window.duration / 60).max(1)),
        "minute" | "minutes" => Some(window.duration),
        "hour" | "hours" => Some(window.duration.saturating_mul(60)),
        "day" | "days" => Some(window.duration.saturating_mul(24 * 60)),
        _ => None,
    }
}

async fn kimi_web_post(
    client: &Client,
    url: &str,
    token: &str,
    body: serde_json::Value,
) -> Result<reqwest::Response, ProviderError> {
    client
        .post(url)
        .bearer_auth(token)
        .header("Cookie", format!("kimi-auth={token}"))
        .header("Accept", "application/json")
        .header("Content-Type", "application/json")
        .header(
            "User-Agent",
            "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36",
        )
        .json(&body)
        .send()
        .await
        .map_err(ProviderError::from)
}

fn value_as_f64(value: Option<&serde_json::Value>) -> Option<f64> {
    match value? {
        serde_json::Value::Number(number) => number.as_f64(),
        serde_json::Value::String(text) => text.trim().replace(',', "").parse().ok(),
        _ => None,
    }
}

fn parse_kimi_timestamp(value: &serde_json::Value) -> Option<DateTime<Utc>> {
    match value {
        serde_json::Value::String(text) => parse_kimi_timestamp_str(text),
        serde_json::Value::Number(number) => number.as_i64().and_then(timestamp_from_number),
        _ => None,
    }
}

fn parse_kimi_timestamp_str(text: &str) -> Option<DateTime<Utc>> {
    let text = text.trim();
    if text.is_empty() {
        return None;
    }
    if let Ok(dt) = DateTime::parse_from_rfc3339(text) {
        return Some(dt.with_timezone(&Utc));
    }
    text.parse::<i64>().ok().and_then(timestamp_from_number)
}

fn timestamp_from_number(raw: i64) -> Option<DateTime<Utc>> {
    let seconds = if raw > 10_000_000_000 {
        raw / 1000
    } else {
        raw
    };
    DateTime::from_timestamp(seconds, 0)
}

fn format_usage_amount(value: f64) -> String {
    if (value.fract()).abs() < f64::EPSILON {
        format!("{}", value as i64)
    } else {
        format!("{value:.2}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn code_api_usage_endpoint_normalizes_base_paths() {
        let root = Url::parse("https://api.kimi.com").unwrap();
        assert_eq!(
            KimiProvider::code_api_usage_endpoint(&root)
                .unwrap()
                .as_str(),
            "https://api.kimi.com/coding/v1/usages"
        );
        let coding = Url::parse("https://proxy.example/kimi/coding").unwrap();
        assert_eq!(
            KimiProvider::code_api_usage_endpoint(&coding)
                .unwrap()
                .as_str(),
            "https://proxy.example/kimi/coding/v1/usages"
        );
        let versioned = Url::parse("https://proxy.example/kimi/coding/v1").unwrap();
        assert_eq!(
            KimiProvider::code_api_usage_endpoint(&versioned)
                .unwrap()
                .as_str(),
            "https://proxy.example/kimi/coding/v1/usages"
        );
    }

    #[test]
    fn auth_token_search_skips_unrelated_cookie_headers() {
        let token = KimiProvider::auth_token_from_cookie_headers([
            "locale=en-US; device_id=abc",
            "kimi-auth=valid-token",
        ])
        .unwrap();

        assert_eq!(token, "valid-token");
    }

    #[test]
    fn parses_code_api_usage_with_string_numbers() {
        let response: KimiCodeApiUsageResponse = serde_json::from_value(json!({
            "usage": {
                "limit": "1000",
                "used": "250",
                "remaining": "750",
                "reset_time": "1767225600"
            },
            "limits": [{
                "window": { "duration": 300, "timeUnit": "TIME_UNIT_MINUTE" },
                "detail": {
                    "limit": "100",
                    "remaining": "80",
                    "resetAt": "2026-01-01T00:00:00Z"
                }
            }]
        }))
        .unwrap();

        let snapshot = KimiProvider::snapshot_from_code_api_response(response).unwrap();
        assert_eq!(snapshot.login_method.as_deref(), Some("Code API"));
        assert!((snapshot.primary.used_percent - 25.0).abs() < f64::EPSILON);
        let secondary = snapshot.secondary.unwrap();
        assert_eq!(secondary.window_minutes, Some(300));
        assert!((secondary.used_percent - 20.0).abs() < f64::EPSILON);
    }

    #[test]
    fn parses_code_api_usage_with_null_limits() {
        let response: KimiCodeApiUsageResponse = serde_json::from_value(json!({
            "usage": {
                "limit": "1000",
                "used": "125"
            },
            "limits": null
        }))
        .unwrap();

        let snapshot = KimiProvider::snapshot_from_code_api_response(response).unwrap();
        assert!((snapshot.primary.used_percent - 12.5).abs() < f64::EPSILON);
        assert!(snapshot.secondary.is_none());
    }

    #[test]
    fn parses_web_usage_with_subscription_windows() {
        let usage: KimiWebUsageResponse = serde_json::from_value(json!({
            "usages": [{
                "scope": "FEATURE_CODING",
                "detail": { "limit": "2048", "used": "375", "resetTime": "2026-01-09T15:23:13Z" },
                "limits": [{
                    "window": { "duration": 300, "timeUnit": "TIME_UNIT_MINUTE" },
                    "detail": { "limit": "100", "used": "25" }
                }]
            }]
        }))
        .unwrap();
        let subscription: KimiSubscriptionStatsResponse = serde_json::from_value(json!({
            "subscriptionBalance": {
                "amountUsedRatio": 0.7716,
                "expireTime": "2026-07-23T00:00:00Z"
            },
            "ratelimitCode7d": {
                "ratio": 0.0946,
                "enabled": true,
                "resetTime": "2026-07-13T15:28:00Z"
            }
        }))
        .unwrap();

        let snapshot =
            KimiProvider::snapshot_from_web_usage_response(usage, Some(subscription)).unwrap();

        assert!((snapshot.primary.used_percent - 18.310546875).abs() < f64::EPSILON);
        assert_eq!(
            snapshot.secondary.as_ref().unwrap().window_minutes,
            Some(300)
        );
        let monthly = snapshot
            .extra_rate_windows
            .iter()
            .find(|window| window.id == "kimi-monthly")
            .unwrap();
        assert_eq!(monthly.title, "Monthly");
        assert!((monthly.window.used_percent - 77.16).abs() < 0.0001);
        let code_7d = snapshot
            .extra_rate_windows
            .iter()
            .find(|window| window.id == "kimi-code-7d")
            .unwrap();
        assert_eq!(code_7d.title, "Code 7-day");
        assert_eq!(code_7d.window.window_minutes, Some(10080));
        assert!((code_7d.window.used_percent - 9.46).abs() < 0.0001);
    }
}
