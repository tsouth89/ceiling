//! Grok provider implementation.
//!
//! Reads SuperGrok / Grok Build usage from grok.com via:
//! - `~/.grok/auth.json` produced by `grok login` (primary, Claude/Codex-style), or
//! - browser cookies for grok.com when available.
//!
//! Billing RPC: `GrokBuildBilling/GetGrokCreditsConfig` (weekly shared usage pool).

use async_trait::async_trait;
use chrono::{DateTime, TimeZone, Utc};
use reqwest::Client;
use serde_json::Value;
use std::path::PathBuf;

#[cfg(windows)]
use std::os::windows::process::CommandExt;

use crate::core::{
    FetchContext, Provider, ProviderError, ProviderFetchResult, ProviderId, ProviderMetadata,
    RateWindow, SourceMode, UsageSnapshot,
};

const BILLING_ENDPOINT: &str = "https://grok.com/grok_api_v2.GrokBuildBilling/GetGrokCreditsConfig";
const SUBSCRIPTIONS_ENDPOINT: &str = "https://grok.com/rest/subscriptions";
const WEEKLY_MINUTES: u32 = 7 * 24 * 60;

/// Whether a usable `~/.grok/auth.json` (or `$GROK_HOME/auth.json`) exists.
/// True when an access token or refresh token is present (expired access is OK
/// if we can refresh, same idea as Claude OAuth).
pub fn local_credentials_available() -> bool {
    GrokCredentials::load_from_disk()
        .map(|creds| !creds.access_token.is_empty() || creds.refresh_token.is_some())
        .unwrap_or(false)
}

/// Whether the `grok` CLI appears on PATH or in known Windows install locations.
pub fn cli_installed() -> bool {
    which::which("grok").is_ok()
        || GrokProvider::detect_cli_version().is_some()
        || dirs::data_local_dir().is_some_and(|base| {
            base.join("Programs")
                .join("grok")
                .join("grok.exe")
                .is_file()
                || base.join("grok").join("grok.exe").is_file()
        })
        || std::env::var_os("USERPROFILE").is_some_and(|home| {
            PathBuf::from(home)
                .join(".grok")
                .join("bin")
                .join("grok.exe")
                .is_file()
        })
}

pub struct GrokProvider {
    metadata: ProviderMetadata,
    client: Client,
}

impl GrokProvider {
    pub fn new() -> Self {
        Self {
            metadata: ProviderMetadata {
                id: ProviderId::Grok,
                display_name: "Grok",
                session_label: "Weekly pool",
                weekly_label: "Extra credits",
                supports_opus: false,
                // Extra-credit balance is parsed when the billing RPC includes
                // prepaid fields; otherwise only the weekly pool is shown.
                supports_credits: true,
                default_enabled: true,
                is_primary: true,
                dashboard_url: Some("https://grok.com/?_s=usage"),
                status_page_url: Some("https://status.x.ai"),
            },
            client: crate::core::credentialed_http_client_builder()
                .timeout(std::time::Duration::from_secs(15))
                .build()
                .unwrap_or_else(|_| Client::new()),
        }
    }

    fn auth_file_path() -> Option<PathBuf> {
        if let Ok(home) = std::env::var("GROK_HOME")
            && !home.trim().is_empty()
        {
            return Some(PathBuf::from(home).join("auth.json"));
        }
        dirs::home_dir().map(|home| home.join(".grok").join("auth.json"))
    }

    /// Load credentials and refresh the access token when expired (or about to).
    async fn load_fresh_credentials(&self) -> Result<GrokCredentials, ProviderError> {
        let mut credentials = GrokCredentials::load_from_disk()?;
        if credentials.needs_refresh() {
            credentials = self.refresh_and_persist(credentials).await?;
        }
        if credentials.access_token.is_empty() {
            return Err(ProviderError::AuthRequired);
        }
        Ok(credentials)
    }

    async fn refresh_and_persist(
        &self,
        current: GrokCredentials,
    ) -> Result<GrokCredentials, ProviderError> {
        let Some(refresh_token) = current.refresh_token.as_deref().filter(|s| !s.is_empty()) else {
            return Err(ProviderError::AuthRequired);
        };
        let client_id = current
            .oidc_client_id
            .clone()
            .filter(|s| !s.is_empty())
            .ok_or(ProviderError::AuthRequired)?;
        let token_url = current
            .oidc_issuer
            .as_deref()
            .filter(|s| !s.is_empty())
            .map(|issuer| format!("{}/oauth2/token", issuer.trim_end_matches('/')))
            .unwrap_or_else(|| "https://auth.x.ai/oauth2/token".to_string());

        let response = self
            .client
            .post(token_url)
            .header("Accept", "application/json")
            .header("Content-Type", "application/x-www-form-urlencoded")
            .body(format!(
                "grant_type=refresh_token&refresh_token={}&client_id={}",
                urlencoding_form(refresh_token),
                urlencoding_form(&client_id),
            ))
            .send()
            .await?;
        if !response.status().is_success() {
            let status = response.status();
            let text = response.text().await.unwrap_or_default();
            tracing::warn!(
                "Grok token refresh failed ({status}): {}",
                text.chars().take(160).collect::<String>()
            );
            return Err(ProviderError::AuthRequired);
        }
        let body: Value = response
            .json()
            .await
            .map_err(|e| ProviderError::Parse(format!("Grok refresh response: {e}")))?;
        let access_token = body
            .get("access_token")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .ok_or(ProviderError::AuthRequired)?
            .to_string();
        let new_refresh = body
            .get("refresh_token")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(ToOwned::to_owned)
            .or_else(|| current.refresh_token.clone());
        let ttl = body
            .get("expires_in")
            .and_then(Value::as_i64)
            .filter(|v| *v > 0)
            .unwrap_or(21_600);
        let expires_at = Some(Utc::now() + chrono::Duration::seconds(ttl));
        let mut next = current;
        next.access_token = access_token;
        next.refresh_token = new_refresh;
        next.expires_at = expires_at;
        if let Err(err) = next.persist_to_disk() {
            tracing::warn!("Grok token refreshed but could not persist auth.json: {err}");
        }
        Ok(next)
    }

    async fn fetch_with_auth(
        &self,
        credentials: &GrokCredentials,
        source_label: &str,
    ) -> Result<ProviderFetchResult, ProviderError> {
        let auth_header = format!("Bearer {}", credentials.access_token);
        let billing = match self.fetch_billing(Some(auth_header.clone()), None).await {
            Ok(billing) => billing,
            Err(ProviderError::AuthRequired) if credentials.refresh_token.is_some() => {
                // Access token rejected; force one refresh and retry once.
                let refreshed = self.refresh_and_persist(credentials.clone()).await?;
                let retry_header = format!("Bearer {}", refreshed.access_token);
                let billing = self.fetch_billing(Some(retry_header.clone()), None).await?;
                let plan = self
                    .fetch_plan_name(Some(retry_header), None)
                    .await
                    .or_else(|| refreshed.login_method());
                return Ok(result_from_billing(
                    billing,
                    source_label,
                    refreshed.email.clone(),
                    refreshed.team_id.clone(),
                    plan,
                ));
            }
            Err(e) => return Err(e),
        };
        let plan = self
            .fetch_plan_name(Some(auth_header), None)
            .await
            .or_else(|| credentials.login_method());
        Ok(result_from_billing(
            billing,
            source_label,
            credentials.email.clone(),
            credentials.team_id.clone(),
            plan,
        ))
    }

    async fn fetch_with_cookie(
        &self,
        cookie_header: &str,
    ) -> Result<ProviderFetchResult, ProviderError> {
        let billing = self
            .fetch_billing(None, Some(cookie_header.to_string()))
            .await?;
        let plan = self
            .fetch_plan_name(None, Some(cookie_header.to_string()))
            .await;
        Ok(result_from_billing(
            billing,
            "grok-browser",
            None,
            None,
            plan,
        ))
    }

    async fn fetch_billing(
        &self,
        authorization: Option<String>,
        cookie_header: Option<String>,
    ) -> Result<GrokBillingSnapshot, ProviderError> {
        let mut request = self
            .client
            .post(BILLING_ENDPOINT)
            .body(vec![0, 0, 0, 0, 0])
            .header("Origin", "https://grok.com")
            .header("Referer", "https://grok.com/?_s=usage")
            .header("Accept", "*/*")
            .header("Content-Type", "application/grpc-web+proto")
            .header("x-grpc-web", "1")
            .header("x-user-agent", "connect-es/2.1.1")
            .header("User-Agent", "Ceiling");
        if let Some(auth) = authorization {
            request = request.header("Authorization", auth);
        }
        if let Some(cookie) = cookie_header {
            request = request.header("Cookie", cookie);
        }

        let response = request.send().await?;
        let status = response.status();
        let headers = response.headers().clone();
        let bytes = response.bytes().await?;
        if !status.is_success() {
            if status == reqwest::StatusCode::UNAUTHORIZED
                || status == reqwest::StatusCode::FORBIDDEN
            {
                return Err(ProviderError::AuthRequired);
            }
            return Err(ProviderError::Other(format!(
                "Grok web billing returned status {status}"
            )));
        }
        validate_grpc_headers(&headers)?;
        parse_grpc_web_response(&bytes)
    }

    /// Best-effort plan label from grok.com (e.g. SuperGrok Heavy).
    async fn fetch_plan_name(
        &self,
        authorization: Option<String>,
        cookie_header: Option<String>,
    ) -> Option<String> {
        let mut request = self
            .client
            .get(SUBSCRIPTIONS_ENDPOINT)
            .header("Origin", "https://grok.com")
            .header("Referer", "https://grok.com/?_s=usage")
            .header("Accept", "application/json")
            .header("User-Agent", "Ceiling");
        if let Some(auth) = authorization {
            request = request.header("Authorization", auth);
        }
        if let Some(cookie) = cookie_header {
            request = request.header("Cookie", cookie);
        }
        let response = request.send().await.ok()?;
        if !response.status().is_success() {
            return None;
        }
        let value: Value = response.json().await.ok()?;
        plan_name_from_subscriptions(&value)
    }

    /// Local CLI auth (used for Auto fallback, Cli, and OAuth source modes).
    async fn fetch_local_cli_auth(&self) -> Result<ProviderFetchResult, ProviderError> {
        let credentials = self.load_fresh_credentials().await?;
        self.fetch_with_auth(&credentials, "cli").await
    }

    /// Prefer `grok login` credentials (Claude/Codex-style), then cookies.
    async fn fetch_auto(&self, ctx: &FetchContext) -> Result<ProviderFetchResult, ProviderError> {
        if let Some(ref cookie_header) = ctx.manual_cookie_header {
            match self.fetch_with_cookie(cookie_header).await {
                Ok(result) => return Ok(result),
                Err(ProviderError::AuthRequired) => {}
                Err(e) => return Err(e),
            }
        }
        match self.fetch_local_cli_auth().await {
            Ok(result) => return Ok(result),
            Err(ProviderError::AuthRequired) | Err(ProviderError::NotInstalled(_)) => {}
            Err(e) => return Err(e),
        }
        match crate::providers::browser_cookie_header(&["grok.com"]) {
            Ok(cookie_header) => self.fetch_with_cookie(&cookie_header).await,
            Err(ProviderError::NoCookies) => Err(ProviderError::AuthRequired),
            Err(e) => Err(e),
        }
    }

    fn detect_cli_version() -> Option<String> {
        let mut candidates = vec![PathBuf::from("grok")];
        if let Some(home) = std::env::var_os("USERPROFILE") {
            candidates.push(
                PathBuf::from(home)
                    .join(".grok")
                    .join("bin")
                    .join("grok.exe"),
            );
        }
        if let Some(base) = dirs::data_local_dir() {
            candidates.push(base.join("Programs").join("grok").join("grok.exe"));
            candidates.push(base.join("grok").join("grok.exe"));
        }
        for bin in candidates {
            let mut command = std::process::Command::new(&bin);
            command.arg("--version");
            hide_windows_console(&mut command);
            let Ok(output) = command.output() else {
                continue;
            };
            let text = String::from_utf8_lossy(&output.stdout);
            let trimmed = text
                .lines()
                .next()
                .map(str::trim)
                .unwrap_or("")
                .strip_prefix("grok ")
                .unwrap_or(text.trim());
            if !trimmed.is_empty() {
                return Some(trimmed.to_string());
            }
        }
        None
    }
}

/// Minimal form-encoding for OAuth refresh (tokens are base64url-safe).
fn urlencoding_form(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for b in value.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char);
            }
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

#[cfg(windows)]
fn hide_windows_console(command: &mut std::process::Command) {
    const CREATE_NO_WINDOW: u32 = 0x08000000;
    command.creation_flags(CREATE_NO_WINDOW);
}

#[cfg(not(windows))]
fn hide_windows_console(_command: &mut std::process::Command) {}

impl Default for GrokProvider {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Provider for GrokProvider {
    fn id(&self) -> ProviderId {
        ProviderId::Grok
    }

    fn metadata(&self) -> &ProviderMetadata {
        &self.metadata
    }

    async fn fetch_usage(&self, ctx: &FetchContext) -> Result<ProviderFetchResult, ProviderError> {
        match ctx.source_mode {
            // Default shell path for Grok with no pasted cookie is often Cli;
            // treat it like Gemini/Codex: use local `grok login` credentials.
            SourceMode::Auto | SourceMode::Web => self.fetch_auto(ctx).await,
            SourceMode::Cli | SourceMode::OAuth => self.fetch_local_cli_auth().await,
        }
    }

    fn available_sources(&self) -> Vec<SourceMode> {
        vec![
            SourceMode::Auto,
            SourceMode::Web,
            SourceMode::Cli,
            SourceMode::OAuth,
        ]
    }

    fn supports_web(&self) -> bool {
        true
    }

    fn supports_cli(&self) -> bool {
        true
    }

    fn supports_oauth(&self) -> bool {
        true
    }

    fn detect_version(&self) -> Option<String> {
        Self::detect_cli_version()
    }
}

#[derive(Debug, Clone)]
struct GrokCredentials {
    scope: String,
    access_token: String,
    refresh_token: Option<String>,
    auth_mode: Option<String>,
    email: Option<String>,
    team_id: Option<String>,
    expires_at: Option<DateTime<Utc>>,
    oidc_issuer: Option<String>,
    oidc_client_id: Option<String>,
}

impl GrokCredentials {
    fn load_from_disk() -> Result<Self, ProviderError> {
        let path = GrokProvider::auth_file_path()
            .ok_or_else(|| ProviderError::NotInstalled("Grok auth path not found".to_string()))?;
        let text = std::fs::read_to_string(&path).map_err(|_| {
            ProviderError::NotInstalled("Grok auth.json not found. Run `grok login`.".to_string())
        })?;
        Self::parse(&text)
    }

    fn parse(text: &str) -> Result<Self, ProviderError> {
        let root: Value = serde_json::from_str(text)
            .map_err(|e| ProviderError::Parse(format!("Failed to decode Grok auth.json: {e}")))?;
        let map = root
            .as_object()
            .ok_or_else(|| ProviderError::Parse("Invalid Grok auth.json".to_string()))?;
        let mut selected: Option<(String, &Value)> = None;
        for (scope, entry) in map {
            let has_key = entry
                .get("key")
                .and_then(Value::as_str)
                .is_some_and(|s| !s.is_empty());
            let has_refresh = entry
                .get("refresh_token")
                .and_then(Value::as_str)
                .is_some_and(|s| !s.is_empty());
            if !(has_key || has_refresh) {
                continue;
            }
            let prefer = scope.starts_with("https://auth.x.ai::")
                || selected.is_none()
                || scope.contains("/sign-in");
            if prefer {
                selected = Some((scope.clone(), entry));
                if scope.starts_with("https://auth.x.ai::") {
                    break;
                }
            }
        }
        let (scope, entry) = selected.ok_or(ProviderError::AuthRequired)?;
        let access_token = entry
            .get("key")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .unwrap_or("")
            .to_string();
        let refresh_token = text_field(entry, "refresh_token");
        if access_token.is_empty() && refresh_token.is_none() {
            return Err(ProviderError::AuthRequired);
        }
        let expires_at = entry
            .get("expires_at")
            .and_then(Value::as_str)
            .and_then(parse_expires_at);
        Ok(Self {
            scope,
            access_token,
            refresh_token,
            auth_mode: text_field(entry, "auth_mode"),
            email: text_field(entry, "email"),
            team_id: text_field(entry, "team_id"),
            expires_at,
            oidc_issuer: text_field(entry, "oidc_issuer"),
            oidc_client_id: text_field(entry, "oidc_client_id"),
        })
    }

    fn needs_refresh(&self) -> bool {
        if self.refresh_token.as_ref().is_none_or(|s| s.is_empty()) {
            return false;
        }
        if self.access_token.is_empty() {
            return true;
        }
        match self.expires_at {
            Some(exp) => exp <= Utc::now() + chrono::Duration::minutes(2),
            // Unknown expiry: still try refresh when billing returns 401.
            None => false,
        }
    }

    fn persist_to_disk(&self) -> Result<(), String> {
        let path = GrokProvider::auth_file_path().ok_or_else(|| "no auth path".to_string())?;
        let text = std::fs::read_to_string(&path).map_err(|e| e.to_string())?;
        let mut root: Value =
            serde_json::from_str(&text).map_err(|e| format!("decode auth.json: {e}"))?;
        let entry = root
            .as_object_mut()
            .and_then(|map| map.get_mut(&self.scope))
            .and_then(Value::as_object_mut)
            .ok_or_else(|| "auth scope missing".to_string())?;
        entry.insert("key".to_string(), Value::String(self.access_token.clone()));
        if let Some(refresh) = &self.refresh_token {
            entry.insert("refresh_token".to_string(), Value::String(refresh.clone()));
        }
        if let Some(exp) = self.expires_at {
            entry.insert(
                "expires_at".to_string(),
                Value::String(exp.to_rfc3339_opts(chrono::SecondsFormat::Millis, true)),
            );
        }
        let encoded = serde_json::to_string_pretty(&root).map_err(|e| e.to_string())?;
        std::fs::write(&path, encoded).map_err(|e| e.to_string())
    }

    fn login_method(&self) -> Option<String> {
        match self.auth_mode.as_deref().map(str::to_lowercase).as_deref() {
            Some("oidc") => Some("SuperGrok".to_string()),
            Some("session") => Some("session".to_string()),
            Some(other) => Some(other.to_string()),
            None if self.expires_at.is_some() => Some("Grok".to_string()),
            None => None,
        }
    }
}

fn parse_expires_at(raw: &str) -> Option<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(raw)
        .ok()
        .map(|dt| dt.with_timezone(&Utc))
        .or_else(|| {
            chrono::NaiveDateTime::parse_from_str(raw, "%Y-%m-%dT%H:%M:%S%.f")
                .or_else(|_| chrono::NaiveDateTime::parse_from_str(raw, "%Y-%m-%d %H:%M:%S"))
                .ok()
                .map(|naive| naive.and_utc())
        })
}

fn text_field(value: &Value, key: &str) -> Option<String> {
    value
        .get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(ToOwned::to_owned)
}

/// Active subscription tier ranking (higher wins). Matches grok.com labels.
fn plan_name_from_subscriptions(root: &Value) -> Option<String> {
    let list = root.get("subscriptions")?.as_array()?;
    let mut best: Option<(i32, String)> = None;
    for sub in list {
        let status = sub.get("status").and_then(Value::as_str).unwrap_or("");
        if status != "SUBSCRIPTION_STATUS_ACTIVE" {
            continue;
        }
        let tier = sub.get("tier").and_then(Value::as_str).unwrap_or("");
        let (rank, label) = match tier {
            "SUBSCRIPTION_TIER_SUPER_GROK_PRO" => (60, "SuperGrok Heavy"),
            "SUBSCRIPTION_TIER_GROK_PRO" => (50, "SuperGrok"),
            "SUBSCRIPTION_TIER_SUPER_GROK_LITE" => (40, "SuperGrok Lite"),
            "SUBSCRIPTION_TIER_X_PREMIUM_PLUS" => (30, "X Premium+"),
            "SUBSCRIPTION_TIER_X_PREMIUM" => (20, "X Premium"),
            "SUBSCRIPTION_TIER_X_BASIC" => (10, "X Basic"),
            _ => continue,
        };
        if best.as_ref().is_none_or(|(r, _)| rank > *r) {
            best = Some((rank, label.to_string()));
        }
    }
    best.map(|(_, label)| label)
}

#[derive(Debug, Clone)]
struct GrokBillingSnapshot {
    used_percent: f64,
    resets_at: Option<DateTime<Utc>>,
    window_minutes: Option<u32>,
    /// Extra usage credits remaining, when the RPC reports a prepaid balance.
    prepaid_balance_cents: Option<u64>,
}

fn result_from_billing(
    billing: GrokBillingSnapshot,
    source_label: &str,
    email: Option<String>,
    team_id: Option<String>,
    login_method: Option<String>,
) -> ProviderFetchResult {
    let mut usage = UsageSnapshot::new(RateWindow::with_details(
        billing.used_percent,
        billing.window_minutes.or(Some(WEEKLY_MINUTES)),
        billing.resets_at,
        None,
    ));
    // When prepaid/extra credits are present, surface a second meter as used%
    // of a synthetic window is not meaningful (balance is absolute). Show 0%
    // used when balance > 0 so the bar reads as "have credits", matching other
    // credit-balance providers that only know remaining funds.
    if let Some(cents) = billing.prepaid_balance_cents.filter(|c| *c > 0) {
        let dollars = cents as f64 / 100.0;
        usage = usage.with_secondary(RateWindow::with_details(
            0.0,
            None,
            None,
            Some(format!("${dollars:.2} extra credits")),
        ));
    }
    usage.account_email = email;
    usage.account_organization = team_id;
    usage.login_method = login_method;
    ProviderFetchResult::new(usage, source_label)
}

fn validate_grpc_headers(headers: &reqwest::header::HeaderMap) -> Result<(), ProviderError> {
    if let Some(status) = headers
        .get("grpc-status")
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.parse::<u16>().ok())
        && status != 0
    {
        if status == 16 {
            return Err(ProviderError::AuthRequired);
        }
        return Err(ProviderError::Other(format!(
            "Grok RPC failed with status {status}"
        )));
    }
    Ok(())
}

fn parse_grpc_web_response(data: &[u8]) -> Result<GrokBillingSnapshot, ProviderError> {
    let frames = grpc_web_data_frames(data);
    if frames.is_empty() {
        return Err(ProviderError::Parse(
            "Grok web billing returned no payload".to_string(),
        ));
    }
    let mut scan = ProtoScan::default();
    for frame in frames {
        scan.scan_message(&frame, &mut Vec::new(), 0);
    }

    // grok.com UI maps config.creditUsagePercent. Zero-usage responses often
    // omit the float entirely (protobuf default 0), so treat a valid config
    // message without a percent as 0% rather than a hard parse failure.
    let used_percent = scan
        .fixed32
        .iter()
        .filter(|field| {
            field.path.last() == Some(&1)
                && field.value.is_finite()
                && field.value >= 0.0
                && field.value <= 100.0
        })
        .min_by(|a, b| {
            a.path
                .len()
                .cmp(&b.path.len())
                .then_with(|| a.order.cmp(&b.order))
        })
        .map(|field| field.value as f64)
        .unwrap_or(0.0);

    // Prefer future timestamps (period end). SuperGrok Heavy returns a weekly
    // window as nested google.protobuf.Timestamp seconds.
    let now = Utc::now();
    let mut future_ts: Vec<DateTime<Utc>> = scan
        .varints
        .iter()
        .filter_map(|field| {
            (1_700_000_000..=2_100_000_000)
                .contains(&field.value)
                .then(|| Utc.timestamp_opt(field.value as i64, 0).single())
                .flatten()
        })
        .filter(|dt| *dt > now)
        .collect();
    future_ts.sort();
    // Period end is the latest future timestamp (start may also still be "future"
    // relative to fixtures; live accounts usually only have end in the future).
    let resets_at = future_ts.last().copied();

    // Heuristic: a ~7 day span between timestamps is the shared weekly pool.
    let window_minutes = if future_ts.len() >= 2 {
        let span = future_ts
            .last()
            .unwrap()
            .signed_duration_since(*future_ts.first().unwrap());
        let days = span.num_days().unsigned_abs();
        if (6..=8).contains(&days) {
            Some(WEEKLY_MINUTES)
        } else {
            None
        }
    } else {
        // Single future reset with no span: still label weekly (current product).
        resets_at.map(|_| WEEKLY_MINUTES)
    };

    // Prepaid/extra-credit balance is a nested Money `val` in the web client.
    // Zero balances are omitted from the protobuf; non-zero shapes need a
    // field-stable decode. Leave empty rather than guessing from stray varints.
    let prepaid_balance_cents = None;

    Ok(GrokBillingSnapshot {
        used_percent,
        resets_at,
        window_minutes,
        prepaid_balance_cents,
    })
}

fn grpc_web_data_frames(data: &[u8]) -> Vec<Vec<u8>> {
    let mut frames = Vec::new();
    let mut index = 0;
    while index + 5 <= data.len() {
        let flags = data[index];
        let len = ((data[index + 1] as usize) << 24)
            | ((data[index + 2] as usize) << 16)
            | ((data[index + 3] as usize) << 8)
            | (data[index + 4] as usize);
        let start = index + 5;
        let end = start.saturating_add(len);
        if end > data.len() {
            break;
        }
        if flags & 0x80 == 0 {
            frames.push(data[start..end].to_vec());
        }
        index = end;
    }
    frames
}

#[derive(Default)]
struct ProtoScan {
    fixed32: Vec<Fixed32Field>,
    varints: Vec<VarintField>,
    order: usize,
}

struct Fixed32Field {
    path: Vec<u64>,
    value: f32,
    order: usize,
}

struct VarintField {
    value: u64,
}

impl ProtoScan {
    fn scan_message(&mut self, data: &[u8], path: &mut Vec<u64>, depth: usize) {
        if depth > 8 {
            return;
        }
        let mut i = 0;
        while i < data.len() {
            let Some((field, wire, next)) = read_key(data, i) else {
                break;
            };
            i = next;
            path.push(field);
            let Some(next) = self.scan_field(data, i, path, depth, wire) else {
                path.pop();
                break;
            };
            i = next;
            path.pop();
        }
    }

    fn scan_field(
        &mut self,
        data: &[u8],
        i: usize,
        path: &mut Vec<u64>,
        depth: usize,
        wire: u64,
    ) -> Option<usize> {
        match wire {
            0 => self.scan_varint(data, i),
            2 => self.scan_length_delimited(data, i, path, depth),
            5 => self.scan_fixed32(data, i, path),
            1 => Some(i.saturating_add(8)),
            _ => None,
        }
    }

    fn scan_varint(&mut self, data: &[u8], i: usize) -> Option<usize> {
        let (value, next) = read_varint(data, i)?;
        self.varints.push(VarintField { value });
        Some(next)
    }

    fn scan_length_delimited(
        &mut self,
        data: &[u8],
        i: usize,
        path: &mut Vec<u64>,
        depth: usize,
    ) -> Option<usize> {
        let (len, next) = read_varint(data, i)?;
        let start = next;
        let end = start.saturating_add(len as usize);
        if end <= data.len() {
            self.scan_message(&data[start..end], path, depth + 1);
            Some(end)
        } else {
            None
        }
    }

    fn scan_fixed32(&mut self, data: &[u8], i: usize, path: &[u64]) -> Option<usize> {
        if i + 4 > data.len() {
            return None;
        }
        let bytes = [data[i], data[i + 1], data[i + 2], data[i + 3]];
        self.fixed32.push(Fixed32Field {
            path: path.to_vec(),
            value: f32::from_le_bytes(bytes),
            order: self.order,
        });
        self.order += 1;
        Some(i + 4)
    }
}

fn read_key(data: &[u8], i: usize) -> Option<(u64, u64, usize)> {
    let (key, next) = read_varint(data, i)?;
    Some((key >> 3, key & 0x07, next))
}

fn read_varint(data: &[u8], mut i: usize) -> Option<(u64, usize)> {
    let mut value = 0u64;
    let mut shift = 0;
    while i < data.len() && shift < 64 {
        let b = data[i];
        i += 1;
        value |= u64::from(b & 0x7f) << shift;
        if b & 0x80 == 0 {
            return Some((value, i));
        }
        shift += 7;
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_auth_file_prefer_oidc() {
        let auth = r#"{
          "https://accounts.x.ai/sign-in": {"key": "legacy"},
          "https://auth.x.ai::abc": {
            "key": "oidc",
            "auth_mode": "oidc",
            "email": "u@example.com",
            "refresh_token": "refresh",
            "oidc_client_id": "client",
            "oidc_issuer": "https://auth.x.ai"
          }
        }"#;
        let parsed = GrokCredentials::parse(auth).unwrap();
        assert_eq!(parsed.access_token, "oidc");
        assert_eq!(parsed.refresh_token.as_deref(), Some("refresh"));
        assert_eq!(parsed.login_method().as_deref(), Some("SuperGrok"));
        assert!(!parsed.needs_refresh());
    }

    #[test]
    fn expired_token_with_refresh_is_not_hard_fail() {
        let auth = r#"{
          "https://auth.x.ai::abc": {
            "key": "old",
            "refresh_token": "refresh",
            "oidc_client_id": "client",
            "expires_at": "2020-01-01T00:00:00.000Z"
          }
        }"#;
        let parsed = GrokCredentials::parse(auth).unwrap();
        assert!(parsed.needs_refresh());
    }

    #[test]
    fn splits_grpc_web_data_frames() {
        let data = [0, 0, 0, 0, 2, 1, 2, 0x80, 0, 0, 0, 1, b'x'];
        assert_eq!(grpc_web_data_frames(&data), vec![vec![1, 2]]);
    }

    /// Real SuperGrok Heavy zero-usage payload shape (no creditUsagePercent
    /// float; weekly window timestamps only). Must not hard-fail.
    #[test]
    fn parses_zero_usage_weekly_pool_without_percent_float() {
        // grpc-web frame wrapping a config message with period start/end only.
        // Timestamps are far in the future so the test is stable.
        // Field path mirrors live GetGrokCreditsConfig responses.
        let mut payload = Vec::new();
        // outer field 1 length-delimited
        // inner: field 4 Timestamp seconds=2000000000, field 5 Timestamp seconds=2000604800 (~7d)
        let start_secs: u64 = 2_000_000_000;
        let end_secs: u64 = 2_000_604_800;
        let mut inner = Vec::new();
        // field 4 = timestamp message with field 1 = start_secs
        let mut ts_start = Vec::new();
        write_key(&mut ts_start, 1, 0);
        write_varint(&mut ts_start, start_secs);
        write_key(&mut inner, 4, 2);
        write_varint(&mut inner, ts_start.len() as u64);
        inner.extend_from_slice(&ts_start);
        let mut ts_end = Vec::new();
        write_key(&mut ts_end, 1, 0);
        write_varint(&mut ts_end, end_secs);
        write_key(&mut inner, 5, 2);
        write_varint(&mut inner, ts_end.len() as u64);
        inner.extend_from_slice(&ts_end);

        write_key(&mut payload, 1, 2);
        write_varint(&mut payload, inner.len() as u64);
        payload.extend_from_slice(&inner);

        let mut frame = vec![0];
        let len = payload.len() as u32;
        frame.extend_from_slice(&len.to_be_bytes());
        frame.extend_from_slice(&payload);

        let snap = parse_grpc_web_response(&frame).unwrap();
        assert_eq!(snap.used_percent, 0.0);
        assert_eq!(
            snap.resets_at,
            Some(Utc.timestamp_opt(end_secs as i64, 0).single().unwrap())
        );
        assert_eq!(snap.window_minutes, Some(WEEKLY_MINUTES));
    }

    #[test]
    fn parses_percent_float_when_present() {
        // config { creditUsagePercent: 42.5f } as field 1 fixed32 at path [1,1]
        // Minimal: field 1 { field 1 fixed32 42.5 }
        let mut inner = Vec::new();
        write_key(&mut inner, 1, 5);
        inner.extend_from_slice(&42.5f32.to_le_bytes());
        let mut payload = Vec::new();
        write_key(&mut payload, 1, 2);
        write_varint(&mut payload, inner.len() as u64);
        payload.extend_from_slice(&inner);
        let mut frame = vec![0];
        let len = payload.len() as u32;
        frame.extend_from_slice(&len.to_be_bytes());
        frame.extend_from_slice(&payload);

        let snap = parse_grpc_web_response(&frame).unwrap();
        assert!((snap.used_percent - 42.5).abs() < 0.01);
    }

    #[test]
    fn maps_active_supergrok_pro_to_heavy_label() {
        let json = serde_json::json!({
            "subscriptions": [
                {
                    "tier": "SUBSCRIPTION_TIER_GROK_PRO",
                    "status": "SUBSCRIPTION_STATUS_INACTIVE"
                },
                {
                    "tier": "SUBSCRIPTION_TIER_SUPER_GROK_PRO",
                    "status": "SUBSCRIPTION_STATUS_ACTIVE"
                },
                {
                    "tier": "SUBSCRIPTION_TIER_X_PREMIUM",
                    "status": "SUBSCRIPTION_STATUS_ACTIVE"
                }
            ]
        });
        assert_eq!(
            plan_name_from_subscriptions(&json).as_deref(),
            Some("SuperGrok Heavy")
        );
    }

    fn write_key(buf: &mut Vec<u8>, field: u64, wire: u64) {
        write_varint(buf, (field << 3) | wire);
    }

    fn write_varint(buf: &mut Vec<u8>, mut value: u64) {
        loop {
            let mut byte = (value & 0x7f) as u8;
            value >>= 7;
            if value != 0 {
                byte |= 0x80;
            }
            buf.push(byte);
            if value == 0 {
                break;
            }
        }
    }
}
