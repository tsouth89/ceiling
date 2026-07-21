//! Claude Web API fetcher - uses browser cookies to fetch usage from claude.ai

use chrono::{DateTime, Utc};
use reqwest::{Client, header};
use serde::Deserialize;
use std::path::PathBuf;

use super::UtilizationScale;
use crate::browser::cookies::{get_cookie_header, get_cookie_header_from_browser};
use crate::browser::detection::{BrowserProfile, BrowserType, DetectedBrowser};
use crate::core::{
    CostSnapshot, NamedRateWindow, PromoSignal, ProviderError, ProviderFetchResult, RateWindow,
    UsageSnapshot,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClaudeDesktopSessionStatus {
    Ready,
    Locked,
    SignedOut,
    Unavailable,
}

/// Locate Claude Desktop's Chromium profile without reading any credential
/// values. Windows Store builds keep Electron data under the package's
/// redirected Roaming directory; older standalone builds use `%APPDATA%`.
fn claude_desktop_data_dirs() -> Vec<PathBuf> {
    let mut candidates = std::env::var_os("CLAUDE_DESKTOP_DATA_DIR")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .into_iter()
        .collect::<Vec<_>>();
    for candidate in claude_desktop_data_dirs_from(dirs::data_local_dir(), dirs::data_dir()) {
        if !candidates.contains(&candidate) {
            candidates.push(candidate);
        }
    }
    candidates
}

fn claude_desktop_data_dirs_from(
    local_app_data: Option<PathBuf>,
    roaming_app_data: Option<PathBuf>,
) -> Vec<PathBuf> {
    let mut candidates = Vec::new();
    if let Some(local) = local_app_data {
        candidates.push(
            local
                .join("Packages")
                .join("Claude_pzs8sxrjxfjjc")
                .join("LocalCache")
                .join("Roaming")
                .join("Claude"),
        );
    }
    if let Some(roaming) = roaming_app_data {
        candidates.push(roaming.join("Claude"));
    }
    candidates.dedup();
    candidates
}

fn claude_desktop_session() -> Result<String, ClaudeDesktopSessionStatus> {
    let mut found_profile = false;
    for data_dir in claude_desktop_data_dirs() {
        if !data_dir.join("Network").join("Cookies").is_file()
            || !data_dir.join("Local State").is_file()
        {
            continue;
        }
        found_profile = true;

        let desktop = DetectedBrowser {
            browser_type: BrowserType::Chromium,
            user_data_dir: data_dir.clone(),
            profiles: vec![BrowserProfile {
                name: "Claude Desktop".to_string(),
                path: data_dir,
                is_default: true,
            }],
        };

        match get_cookie_header_from_browser("claude.ai", &desktop) {
            Ok(header) if cookie_value(&header, "sessionKey").is_some() => {
                tracing::debug!("Using Claude Desktop session for usage fetch");
                return Ok(header);
            }
            Ok(_) => tracing::debug!("Claude Desktop is present but has no active session"),
            Err(error) => {
                tracing::debug!("Claude Desktop session unavailable: {error}");
                let message = error.to_string().to_ascii_lowercase();
                if message.contains("os error 32")
                    || message.contains("being used by another process")
                    || message.contains("sharing violation")
                {
                    return Err(ClaudeDesktopSessionStatus::Locked);
                }
            }
        }
    }
    Err(if found_profile {
        ClaudeDesktopSessionStatus::SignedOut
    } else {
        ClaudeDesktopSessionStatus::Unavailable
    })
}

pub fn claude_desktop_session_status() -> ClaudeDesktopSessionStatus {
    match claude_desktop_session() {
        Ok(_) => ClaudeDesktopSessionStatus::Ready,
        Err(status) => status,
    }
}

fn claude_desktop_cookie_header() -> Option<String> {
    claude_desktop_session().ok()
}

/// Read the response body as text, then deserialize as JSON. On failure, include
/// non-sensitive shape metadata so auth redirects, error envelopes, and schema
/// changes are distinguishable without exposing account data in UI/log output.
async fn parse_json_with_body<T: serde::de::DeserializeOwned>(
    response: reqwest::Response,
    label: &str,
) -> Result<T, ProviderError> {
    let content_type = response
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .map(str::to_owned);
    let body = response
        .text()
        .await
        .map_err(|e| ProviderError::Parse(format!("Failed to read {label} response body: {e}")))?;

    serde_json::from_str::<T>(&body).map_err(|e| {
        ProviderError::Parse(format!(
            "Failed to parse {label}: {e} ({})",
            describe_json_body_shape(&body, content_type.as_deref())
        ))
    })
}

fn describe_json_body_shape(body: &str, content_type: Option<&str>) -> String {
    let content_type = content_type.unwrap_or("unknown");
    let body_len = body.len();

    match serde_json::from_str::<serde_json::Value>(body) {
        Ok(serde_json::Value::Object(map)) => {
            let mut keys: Vec<&str> = map.keys().map(String::as_str).collect();
            keys.sort_unstable();
            let suffix = if keys.len() > 12 { ", ..." } else { "" };
            let keys = keys.into_iter().take(12).collect::<Vec<_>>().join(", ");
            format!("content_type={content_type}, body_len={body_len}, json_keys=[{keys}{suffix}]")
        }
        Ok(value) => format!(
            "content_type={content_type}, body_len={body_len}, json_type={}",
            json_value_kind(&value)
        ),
        Err(_) => format!("content_type={content_type}, body_len={body_len}, body_kind=non-json"),
    }
}

fn json_value_kind(value: &serde_json::Value) -> &'static str {
    match value {
        serde_json::Value::Null => "null",
        serde_json::Value::Bool(_) => "bool",
        serde_json::Value::Number(_) => "number",
        serde_json::Value::String(_) => "string",
        serde_json::Value::Array(_) => "array",
        serde_json::Value::Object(_) => "object",
    }
}

/// Claude Web API fetcher
pub struct ClaudeWebApiFetcher {
    client: Client,
}

/// Organization info from Claude API
#[derive(Debug, Deserialize)]
struct Organization {
    uuid: String,
    #[allow(dead_code)]
    name: Option<String>,
}

/// Usage response from Claude API.
///
/// Anthropic ships overlapping field names for the design and routines
/// windows (e.g. both `seven_day_design` and `seven_day_omelette` may appear
/// in the same payload). Serde aliases can't accept that — it errors with
/// "duplicate field" if more than one alias is present. We deserialize into
/// a generic map and pick the first alias that yields a non-null value.
#[derive(Debug)]
struct UsageResponse {
    five_hour: Option<UsageWindow>,
    seven_day: Option<UsageWindow>,
    seven_day_opus: Option<UsageWindow>,
    seven_day_sonnet: Option<UsageWindow>,
    seven_day_oauth_apps: Option<UsageWindow>,
    seven_day_design: Option<UsageWindow>,
    /// Temporary promotional weekly pool when Anthropic reports omelette fields.
    seven_day_promotional: Option<UsageWindow>,
    seven_day_routines: Option<UsageWindow>,
    extra_usage: Option<ExtraUsageResponse>,
    limits: Vec<super::scoped_weekly::ScopedWeeklyLimit>,
}

impl<'de> Deserialize<'de> for UsageResponse {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let mut map: std::collections::HashMap<String, serde_json::Value> =
            std::collections::HashMap::deserialize(deserializer)?;

        let take = |map: &mut std::collections::HashMap<String, serde_json::Value>,
                    keys: &[&str]|
         -> Result<Option<UsageWindow>, D::Error> {
            for key in keys {
                if let Some(value) = map.remove(*key) {
                    if value.is_null() {
                        continue;
                    }
                    let window: UsageWindow =
                        serde_json::from_value(value).map_err(serde::de::Error::custom)?;
                    return Ok(Some(window));
                }
            }
            Ok(None)
        };

        Ok(UsageResponse {
            five_hour: take(&mut map, &["five_hour"])?,
            seven_day: take(&mut map, &["seven_day"])?,
            seven_day_opus: take(&mut map, &["seven_day_opus"])?,
            seven_day_sonnet: take(&mut map, &["seven_day_sonnet"])?,
            seven_day_oauth_apps: take(
                &mut map,
                &[
                    "seven_day_oauth_apps",
                    "seven_day_claude_oauth_apps",
                    "oauth_apps",
                    "oauth",
                ],
            )?,
            seven_day_design: take(
                &mut map,
                &[
                    "seven_day_design",
                    "seven_day_claude_design",
                    "claude_design",
                    "design",
                ],
            )?,
            seven_day_promotional: take(
                &mut map,
                &["omelette_promotional", "omelette", "seven_day_omelette"],
            )?,
            seven_day_routines: take(
                &mut map,
                &[
                    "seven_day_routines",
                    "seven_day_claude_routines",
                    "claude_routines",
                    "routines",
                    "routine",
                    "seven_day_cowork",
                    "cowork",
                ],
            )?,
            limits: map
                .get("limits")
                .filter(|value| !value.is_null())
                .cloned()
                .map(serde_json::from_value)
                .transpose()
                .map_err(serde::de::Error::custom)?
                .unwrap_or_default(),
            extra_usage: map
                .remove("extra_usage")
                .filter(|value| !value.is_null())
                .map(serde_json::from_value)
                .transpose()
                .map_err(serde::de::Error::custom)?,
        })
    }
}

/// A usage window from the API
#[derive(Debug, Deserialize)]
struct UsageWindow {
    utilization: Option<f64>,

    #[serde(rename = "resets_at")]
    resets_at: Option<String>,
}

/// Extra usage (credits) response
#[derive(Debug, Clone, Deserialize)]
struct ExtraUsageResponse {
    #[serde(rename = "monthly_credit_limit")]
    monthly_credit_limit: Option<f64>,

    #[serde(rename = "used_credits")]
    used_credits: Option<f64>,

    currency: Option<String>,

    #[serde(rename = "is_enabled")]
    is_enabled: Option<bool>,
}

/// Account info response
#[derive(Debug, Deserialize)]
struct AccountResponse {
    email_address: Option<String>,

    #[serde(rename = "rate_limit_tier")]
    rate_limit_tier: Option<String>,

    #[serde(default)]
    memberships: Vec<AccountMembership>,
}

#[derive(Debug, Deserialize)]
struct AccountMembership {
    uuid: Option<String>,
    organization: Option<AccountOrganization>,
}

#[derive(Debug, Deserialize)]
struct AccountOrganization {
    uuid: Option<String>,
}

impl AccountResponse {
    fn first_membership_org_id(&self) -> Option<String> {
        self.memberships.iter().find_map(|membership| {
            membership
                .organization
                .as_ref()
                .and_then(|organization| organization.uuid.as_deref())
                .or(membership.uuid.as_deref())
                .map(str::trim)
                .filter(|uuid| !uuid.is_empty())
                .map(ToString::to_string)
        })
    }
}

impl ClaudeWebApiFetcher {
    const BASE_URL: &'static str = "https://claude.ai/api";

    /// Create a new fetcher
    pub fn new() -> Self {
        Self {
            client: crate::core::credentialed_http_client_builder()
                .timeout(std::time::Duration::from_secs(30))
                .build()
                .expect("Failed to create HTTP client"),
        }
    }

    /// Fetch usage using browser cookies or env-var session key
    pub async fn fetch_with_cookies(&self) -> Result<ProviderFetchResult, ProviderError> {
        if let Some(session_key) = Self::resolve_session_key_from_env() {
            tracing::debug!("Using Claude session key from environment variable");
            let cookie_header = format!("sessionKey={session_key}");
            return self.fetch_with_cookie_header(&cookie_header).await;
        }

        // Reuse the signed-in Claude Desktop session before probing browsers.
        // This keeps Automatic genuinely zero-setup for desktop users and does
        // not persist or log the extracted cookie value in Ceiling.
        if let Some(cookie_header) = claude_desktop_cookie_header() {
            let mut result = self.fetch_with_cookie_header(&cookie_header).await?;
            result.source_label = "desktop".to_string();
            return Ok(result);
        }

        // Try multiple domains - Claude uses different domains for different services
        let domains = [
            "claude.ai",
            "claude.com",
            "console.anthropic.com",
            "anthropic.com",
        ];

        for domain in domains {
            match get_cookie_header(domain) {
                Ok(cookie_header) if !cookie_header.is_empty() => {
                    tracing::debug!("Found cookies for {}", domain);
                    return self.fetch_with_cookie_header(&cookie_header).await;
                }
                Ok(_) => {
                    tracing::debug!("No cookies found for {}", domain);
                }
                Err(e) => {
                    tracing::debug!("Failed to get cookies for {}: {}", domain, e);
                }
            }
        }

        Err(ProviderError::NoCookies)
    }

    /// Fetch usage with a provided cookie header
    pub async fn fetch_with_cookie_header(
        &self,
        cookie_header: &str,
    ) -> Result<ProviderFetchResult, ProviderError> {
        tracing::debug!("Fetching Claude usage via web API");

        let headers = Self::build_headers(cookie_header);

        // Step 1: Get organization ID
        let org_id = self.get_organization_id(cookie_header, &headers).await?;
        tracing::debug!("Got organization ID: {}", org_id);

        // Step 2: Fetch usage data
        let usage = self.get_usage(&org_id, &headers).await?;

        // Step 3: Fetch extra usage (credits) - optional
        let extra_usage = self
            .get_extra_usage(&org_id, &headers)
            .await
            .ok()
            .or_else(|| usage.extra_usage.clone());

        // Step 4: Fetch account info - optional
        let account = self.get_account_info(&headers).await.ok();

        // Build the result. Anthropic mixes fractions and percentages between
        // payloads, so settle the unit once from the whole response first.
        let scale = usage.utilization_scale();

        let primary = usage
            .five_hour
            .as_ref()
            .map(|w| self.to_rate_window(w, Some(300), scale)) // 5 hours = 300 minutes
            .unwrap_or_else(|| RateWindow::new(0.0));

        let secondary = usage
            .seven_day
            .as_ref()
            .map(|w| self.to_rate_window(w, Some(10080), scale)); // 7 days = 10080 minutes

        let model_specific = usage
            .seven_day_opus
            .as_ref()
            .map(|w| self.to_rate_window(w, Some(10080), scale));

        let mut snapshot = UsageSnapshot::new(primary);

        if let Some(s) = secondary {
            snapshot = snapshot.with_secondary(s);
        }

        if let Some(m) = model_specific {
            snapshot = snapshot.with_model_specific(m);
        }

        for (id, title, window) in [
            (
                "claude-oauth-apps",
                "OAuth apps",
                usage
                    .seven_day_oauth_apps
                    .as_ref()
                    .map(|w| self.to_rate_window(w, Some(10080), scale)),
            ),
            (
                "claude-routines",
                "Daily Routines",
                usage
                    .seven_day_routines
                    .as_ref()
                    .map(|w| self.to_rate_window(w, Some(10080), scale)),
            ),
            (
                "claude-design",
                "Design",
                usage
                    .seven_day_design
                    .as_ref()
                    .map(|w| self.to_rate_window(w, Some(10080), scale)),
            ),
            (
                "claude-weekly-promo",
                "Weekly promo",
                usage
                    .seven_day_promotional
                    .as_ref()
                    .map(|w| self.to_rate_window(w, Some(10080), scale)),
            ),
        ] {
            if let Some(window) = window {
                snapshot
                    .extra_rate_windows
                    .push(NamedRateWindow::new(id, title, window));
            }
        }
        snapshot
            .extra_rate_windows
            .extend(super::scoped_weekly::scoped_weekly_windows(&usage.limits));

        if let Some(promo) = usage.seven_day_promotional.as_ref() {
            let ends_at = promo
                .resets_at
                .as_deref()
                .and_then(ClaudeWebApiFetcher::parse_iso8601);
            snapshot = snapshot.with_promo_signal(PromoSignal::boost(
                "claude-weekly-promo",
                "Weekly promo",
                "Temporary promotional weekly capacity reported by Claude",
                Some("claude-weekly-promo".to_string()),
                ends_at,
            ));
        }

        if let Some(ref acc) = account {
            if let Some(ref email) = acc.email_address {
                snapshot = snapshot.with_email(email.clone());
            }
            if let Some(ref tier) = acc.rate_limit_tier {
                snapshot = snapshot.with_login_method(super::claude_plan_label(tier));
            }
        }

        let mut result = ProviderFetchResult::new(snapshot, "web");

        // Add cost info if available
        if let Some(extra) = extra_usage
            && extra.is_enabled.unwrap_or(false)
        {
            let used_cents = extra.used_credits.unwrap_or(0.0);
            let limit_cents = extra.monthly_credit_limit;
            let currency = extra.currency.unwrap_or_else(|| "USD".to_string());

            let mut cost = CostSnapshot::new(
                used_cents / 100.0, // Convert cents to dollars
                currency,
                "Monthly",
            );

            if let Some(limit) = limit_cents {
                cost = cost.with_limit(limit / 100.0);
            }

            result = result.with_cost(cost);
        }

        Ok(result)
    }

    fn build_headers(cookie_header: &str) -> reqwest::header::HeaderMap {
        use reqwest::header::HeaderValue;

        let mut headers = reqwest::header::HeaderMap::new();
        if let Ok(cookie) = HeaderValue::from_str(cookie_header) {
            headers.insert(header::COOKIE, cookie);
        }
        headers.insert(header::ACCEPT, HeaderValue::from_static("application/json"));
        headers.insert(
            header::ORIGIN,
            HeaderValue::from_static("https://claude.ai"),
        );
        headers.insert(
            header::REFERER,
            HeaderValue::from_static("https://claude.ai/settings/usage"),
        );
        headers.insert(
            header::USER_AGENT,
            HeaderValue::from_static(
                "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 \
                 (KHTML, like Gecko) Chrome/144.0.0.0 Safari/537.36",
            ),
        );
        headers.insert(
            reqwest::header::HeaderName::from_static("anthropic-client-platform"),
            HeaderValue::from_static("web_claude_ai"),
        );

        headers
    }

    fn resolve_session_key_from_env() -> Option<String> {
        for env_name in ["CLAUDE_AI_SESSION_KEY", "CLAUDE_WEB_SESSION_KEY"] {
            let Ok(value) = std::env::var(env_name) else {
                continue;
            };

            let trimmed = value.trim();
            if trimmed.is_empty() {
                continue;
            }

            let normalized = trimmed
                .strip_prefix("sessionKey=")
                .unwrap_or(trimmed)
                .trim();

            if !normalized.is_empty() {
                return Some(normalized.to_string());
            }
        }

        None
    }

    /// Get the organization ID
    async fn get_organization_id(
        &self,
        cookie_header: &str,
        headers: &reqwest::header::HeaderMap,
    ) -> Result<String, ProviderError> {
        if let Some(org_id) = cookie_value(cookie_header, "lastActiveOrg") {
            return Ok(org_id);
        }

        if let Ok(account) = self.get_account_info(headers).await
            && let Some(org_id) = account.first_membership_org_id()
        {
            return Ok(org_id);
        }

        let url = format!("{}/organizations", Self::BASE_URL);

        let response = self
            .client
            .get(&url)
            .headers(headers.clone())
            .send()
            .await?;

        if !response.status().is_success() {
            return Err(ProviderError::Other(format!(
                "Failed to get organizations: {}",
                response.status()
            )));
        }

        let orgs: Vec<Organization> = parse_json_with_body(response, "organizations").await?;

        orgs.into_iter()
            .next()
            .map(|o| o.uuid)
            .ok_or_else(|| ProviderError::Parse("No organizations found".to_string()))
    }

    /// Get usage data
    async fn get_usage(
        &self,
        org_id: &str,
        headers: &reqwest::header::HeaderMap,
    ) -> Result<UsageResponse, ProviderError> {
        let url = format!("{}/organizations/{}/usage", Self::BASE_URL, org_id);

        let response = self
            .client
            .get(&url)
            .headers(headers.clone())
            .send()
            .await?;

        if !response.status().is_success() {
            return Err(ProviderError::Other(format!(
                "Failed to get usage: {}",
                response.status()
            )));
        }

        parse_json_with_body(response, "usage").await
    }

    /// Get extra usage (credits)
    async fn get_extra_usage(
        &self,
        org_id: &str,
        headers: &reqwest::header::HeaderMap,
    ) -> Result<ExtraUsageResponse, ProviderError> {
        let url = format!(
            "{}/organizations/{}/overage_spend_limit",
            Self::BASE_URL,
            org_id
        );

        let response = self
            .client
            .get(&url)
            .headers(headers.clone())
            .send()
            .await?;

        if !response.status().is_success() {
            return Err(ProviderError::Other(format!(
                "Failed to get extra usage: {}",
                response.status()
            )));
        }

        parse_json_with_body(response, "extra usage").await
    }

    /// Get account info
    async fn get_account_info(
        &self,
        headers: &reqwest::header::HeaderMap,
    ) -> Result<AccountResponse, ProviderError> {
        let url = format!("{}/account", Self::BASE_URL);

        let response = self
            .client
            .get(&url)
            .headers(headers.clone())
            .send()
            .await?;

        if !response.status().is_success() {
            return Err(ProviderError::Other(format!(
                "Failed to get account: {}",
                response.status()
            )));
        }

        parse_json_with_body(response, "account").await
    }

    /// Convert a usage window to a RateWindow
    fn to_rate_window(
        &self,
        window: &UsageWindow,
        window_minutes: Option<u32>,
        scale: UtilizationScale,
    ) -> RateWindow {
        let used_percent = scale.to_percent(window.utilization.unwrap_or(0.0));

        let resets_at = window
            .resets_at
            .as_ref()
            .and_then(|s| Self::parse_iso8601(s));

        let reset_description = resets_at.map(Self::format_reset_time);

        RateWindow::with_details(used_percent, window_minutes, resets_at, reset_description)
    }

    /// Parse ISO8601 date string
    fn parse_iso8601(s: &str) -> Option<DateTime<Utc>> {
        DateTime::parse_from_rfc3339(s)
            .ok()
            .map(|dt| dt.with_timezone(&Utc))
            .or_else(|| {
                chrono::NaiveDateTime::parse_from_str(s, "%Y-%m-%dT%H:%M:%S%.f")
                    .ok()
                    .map(|ndt| ndt.and_utc())
            })
    }

    /// Format reset time for display
    fn format_reset_time(dt: DateTime<Utc>) -> String {
        dt.format("%b %-d at %-I:%M%p").to_string()
    }

    /// Convert rate limit tier to plan name
    fn tier_to_plan_name(tier: &str) -> String {
        super::claude_plan_label(tier)
    }
}

impl Default for ClaudeWebApiFetcher {
    fn default() -> Self {
        Self::new()
    }
}

impl UsageResponse {
    /// Decide the utilization unit from every window this response carries.
    fn utilization_scale(&self) -> UtilizationScale {
        UtilizationScale::detect(
            [
                self.five_hour.as_ref(),
                self.seven_day.as_ref(),
                self.seven_day_opus.as_ref(),
                self.seven_day_sonnet.as_ref(),
                self.seven_day_oauth_apps.as_ref(),
                self.seven_day_design.as_ref(),
                self.seven_day_promotional.as_ref(),
                self.seven_day_routines.as_ref(),
            ]
            .into_iter()
            .flatten()
            .filter_map(|window| window.utilization),
        )
    }
}

fn cookie_value(cookie_header: &str, name: &str) -> Option<String> {
    cookie_header.split(';').find_map(|part| {
        let (key, value) = part.trim().split_once('=')?;
        if key.trim() != name {
            return None;
        }
        let value = value.trim();
        if value.is_empty() {
            None
        } else {
            Some(value.to_string())
        }
    })
}

#[cfg(test)]
mod tests {
    use super::{
        AccountResponse, ClaudeWebApiFetcher, UsageWindow, UtilizationScale,
        claude_desktop_data_dirs_from, cookie_value,
    };
    use reqwest::header;
    use std::path::PathBuf;
    use std::sync::{Mutex, OnceLock};

    fn env_lock() -> &'static Mutex<()> {
        static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
        LOCK.get_or_init(|| Mutex::new(()))
    }

    #[test]
    fn converts_fractional_utilization_to_percent() {
        let window = UsageWindow {
            utilization: Some(0.23),
            resets_at: None,
        };

        let rate = ClaudeWebApiFetcher::new().to_rate_window(
            &window,
            Some(300),
            UtilizationScale::Fraction,
        );

        assert!((rate.used_percent - 23.0).abs() < f64::EPSILON);
    }

    #[test]
    fn preserves_existing_percentage_utilization() {
        let window = UsageWindow {
            utilization: Some(23.0),
            resets_at: None,
        };

        let rate = ClaudeWebApiFetcher::new().to_rate_window(
            &window,
            Some(300),
            UtilizationScale::Percent,
        );

        assert!((rate.used_percent - 23.0).abs() < f64::EPSILON);
    }

    #[test]
    fn labels_max_5x_and_20x_plans() {
        assert_eq!(
            ClaudeWebApiFetcher::tier_to_plan_name("default_claude_max_5x"),
            "Claude Max 5x"
        );
        assert_eq!(
            ClaudeWebApiFetcher::tier_to_plan_name("v2_default_claude_max_20x"),
            "Claude Max 20x"
        );
    }

    #[test]
    fn discovers_packaged_and_legacy_claude_desktop_profiles() {
        let paths = claude_desktop_data_dirs_from(
            Some(PathBuf::from(r"C:\Users\person\AppData\Local")),
            Some(PathBuf::from(r"C:\Users\person\AppData\Roaming")),
        );

        assert_eq!(
            paths,
            vec![
                PathBuf::from(
                    r"C:\Users\person\AppData\Local\Packages\Claude_pzs8sxrjxfjjc\LocalCache\Roaming\Claude"
                ),
                PathBuf::from(r"C:\Users\person\AppData\Roaming\Claude"),
            ]
        );
    }

    #[test]
    fn resolves_raw_session_key_from_primary_env_var() {
        let _guard = env_lock().lock().expect("env lock");
        unsafe {
            std::env::remove_var("CLAUDE_AI_SESSION_KEY");
            std::env::remove_var("CLAUDE_WEB_SESSION_KEY");
            std::env::set_var("CLAUDE_AI_SESSION_KEY", "sk-ant-primary");
            std::env::set_var("CLAUDE_WEB_SESSION_KEY", "sk-ant-secondary");
        }

        let session_key = ClaudeWebApiFetcher::resolve_session_key_from_env();

        assert_eq!(session_key.as_deref(), Some("sk-ant-primary"));

        unsafe {
            std::env::remove_var("CLAUDE_AI_SESSION_KEY");
            std::env::remove_var("CLAUDE_WEB_SESSION_KEY");
        }
    }

    #[test]
    fn resolves_session_key_assignment_from_env_var() {
        let _guard = env_lock().lock().expect("env lock");
        unsafe {
            std::env::remove_var("CLAUDE_AI_SESSION_KEY");
            std::env::remove_var("CLAUDE_WEB_SESSION_KEY");
            std::env::set_var("CLAUDE_WEB_SESSION_KEY", "sessionKey=sk-ant-cookie-format");
        }

        let session_key = ClaudeWebApiFetcher::resolve_session_key_from_env();

        assert_eq!(session_key.as_deref(), Some("sk-ant-cookie-format"));

        unsafe {
            std::env::remove_var("CLAUDE_AI_SESSION_KEY");
            std::env::remove_var("CLAUDE_WEB_SESSION_KEY");
        }
    }

    #[test]
    fn build_headers_include_required_browser_context() {
        let headers = ClaudeWebApiFetcher::build_headers("sessionKey=sk-ant-cookie-format");

        assert_eq!(
            headers
                .get(header::COOKIE)
                .and_then(|value| value.to_str().ok()),
            Some("sessionKey=sk-ant-cookie-format")
        );
        assert_eq!(
            headers
                .get(header::ACCEPT)
                .and_then(|value| value.to_str().ok()),
            Some("application/json")
        );
        assert_eq!(
            headers
                .get(header::ORIGIN)
                .and_then(|value| value.to_str().ok()),
            Some("https://claude.ai")
        );
        assert_eq!(
            headers
                .get(header::REFERER)
                .and_then(|value| value.to_str().ok()),
            Some("https://claude.ai/settings/usage")
        );
        assert_eq!(
            headers
                .get("anthropic-client-platform")
                .and_then(|value| value.to_str().ok()),
            Some("web_claude_ai")
        );
        assert!(headers.contains_key(header::USER_AGENT));
    }

    #[test]
    fn extracts_last_active_org_from_cookie_header() {
        let org = cookie_value(
            "foo=bar; sessionKey=sk-ant-session; lastActiveOrg=org-123; other=value",
            "lastActiveOrg",
        );

        assert_eq!(org.as_deref(), Some("org-123"));
    }

    #[test]
    fn account_membership_prefers_nested_organization_uuid() {
        let account: AccountResponse = serde_json::from_str(
            r#"{
                "email_address": "user@example.com",
                "memberships": [
                    {
                        "uuid": "membership-id",
                        "organization": { "uuid": "org-id" }
                    }
                ]
            }"#,
        )
        .unwrap();

        assert_eq!(account.first_membership_org_id().as_deref(), Some("org-id"));
    }

    #[test]
    fn parses_extra_design_and_routines_aliases() {
        let usage: super::UsageResponse = serde_json::from_str(
            r#"{
                "five_hour": { "utilization": 0.1 },
                "seven_day_design": { "utilization": 31 },
                "seven_day_omelette": { "utilization": 26 },
                "seven_day_cowork": { "utilization": 11 }
            }"#,
        )
        .unwrap();

        let fetcher = ClaudeWebApiFetcher::new();
        let design = usage
            .seven_day_design
            .as_ref()
            .map(|w| fetcher.to_rate_window(w, Some(10080), usage.utilization_scale()))
            .expect("design window");
        let promo = usage
            .seven_day_promotional
            .as_ref()
            .map(|w| fetcher.to_rate_window(w, Some(10080), usage.utilization_scale()))
            .expect("promotional omelette window");
        let routines = usage
            .seven_day_routines
            .as_ref()
            .map(|w| fetcher.to_rate_window(w, Some(10080), usage.utilization_scale()))
            .expect("routines window");

        assert!((design.used_percent - 31.0).abs() < f64::EPSILON);
        assert!((promo.used_percent - 26.0).abs() < f64::EPSILON);
        assert!((routines.used_percent - 11.0).abs() < f64::EPSILON);
    }

    #[test]
    fn maps_scoped_weekly_limits_even_when_inactive() {
        let usage: super::UsageResponse = serde_json::from_str(
            r#"{
                "limits": [{
                    "kind": "weekly_scoped",
                    "group": "weekly",
                    "percent": 7,
                    "resets_at": "2026-07-16T10:00:00Z",
                    "scope": {"model": {"id": null, "display_name": "Fable"}},
                    "is_active": false
                }]
            }"#,
        )
        .unwrap();

        let windows = super::super::scoped_weekly::scoped_weekly_windows(&usage.limits);
        assert_eq!(windows.len(), 1);
        assert_eq!(windows[0].id, "claude-weekly-scoped-fable");
        assert_eq!(windows[0].title, "Fable only");
    }

    #[test]
    fn parses_duplicate_design_and_routines_aliases_with_preferred_key() {
        let usage: super::UsageResponse = serde_json::from_str(
            r#"{
                "seven_day_design": { "utilization": 31 },
                "seven_day_omelette": { "utilization": 26 },
                "seven_day_routines": { "utilization": 19 },
                "seven_day_cowork": { "utilization": 11 }
            }"#,
        )
        .unwrap();

        let fetcher = ClaudeWebApiFetcher::new();
        let design = usage
            .seven_day_design
            .as_ref()
            .map(|w| fetcher.to_rate_window(w, Some(10080), usage.utilization_scale()))
            .expect("design window");
        let routines = usage
            .seven_day_routines
            .as_ref()
            .map(|w| fetcher.to_rate_window(w, Some(10080), usage.utilization_scale()))
            .expect("routines window");

        assert!((design.used_percent - 31.0).abs() < f64::EPSILON);
        assert!((routines.used_percent - 19.0).abs() < f64::EPSILON);
    }

    #[test]
    fn parses_oauth_apps_window_and_embedded_extra_usage() {
        let usage: super::UsageResponse = serde_json::from_str(
            r#"{
                "five_hour": { "utilization": 0.1 },
                "seven_day_oauth_apps": { "utilization": 42 },
                "extra_usage": {
                    "is_enabled": true,
                    "monthly_credit_limit": 2000,
                    "used_credits": 550,
                    "currency": "USD"
                }
            }"#,
        )
        .unwrap();

        let fetcher = ClaudeWebApiFetcher::new();
        let oauth_apps = usage
            .seven_day_oauth_apps
            .as_ref()
            .map(|w| fetcher.to_rate_window(w, Some(10080), usage.utilization_scale()))
            .expect("oauth apps window");
        let extra = usage.extra_usage.expect("extra usage");

        assert!((oauth_apps.used_percent - 42.0).abs() < f64::EPSILON);
        assert_eq!(extra.is_enabled, Some(true));
        assert_eq!(extra.monthly_credit_limit, Some(2000.0));
        assert_eq!(extra.used_credits, Some(550.0));
    }
}
