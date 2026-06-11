//! Doubao / Volcengine Ark provider implementation.
//!
//! Probes Ark chat-completions with a one-token request and reads rate-limit headers.

use async_trait::async_trait;
use chrono::{DateTime, TimeZone, Utc};
use reqwest::Client;
use serde_json::json;

use crate::core::{
    FetchContext, Provider, ProviderError, ProviderFetchResult, ProviderId, ProviderMetadata,
    RateWindow, SourceMode, UsageSnapshot,
};

const DOUBAO_API_URL: &str = "https://ark.cn-beijing.volces.com/api/coding/v3/chat/completions";
const DOUBAO_CREDENTIAL_TARGET: &str = "codexbar-doubao";
const PROBE_MODELS: &[&str] = &[
    "doubao-seed-2.0-code",
    "doubao-1.5-pro-32k",
    "doubao-lite-32k",
];

pub struct DoubaoProvider {
    metadata: ProviderMetadata,
    client: Client,
}

impl DoubaoProvider {
    pub fn new() -> Self {
        Self {
            metadata: ProviderMetadata {
                id: ProviderId::Doubao,
                display_name: "Doubao",
                session_label: "Requests",
                weekly_label: "Usage",
                supports_opus: false,
                supports_credits: false,
                default_enabled: false,
                is_primary: false,
                dashboard_url: Some(
                    "https://console.volcengine.com/ark/region:ark+cn-beijing/usage",
                ),
                status_page_url: None,
            },
            client: crate::core::credentialed_http_client_builder()
                .timeout(std::time::Duration::from_secs(15))
                .build()
                .unwrap_or_else(|_| Client::new()),
        }
    }

    fn api_key(api_key: Option<&str>) -> Result<String, ProviderError> {
        resolve_api_key(
            api_key,
            DOUBAO_CREDENTIAL_TARGET,
            &["ARK_API_KEY", "DOUBAO_API_KEY", "VOLCENGINE_API_KEY"],
        )
    }

    async fn fetch_api(&self, api_key: &str) -> Result<UsageSnapshot, ProviderError> {
        let mut last_error = None;
        for model in PROBE_MODELS {
            match self.probe(api_key, model).await {
                Ok(result) => {
                    return Ok(self
                        .confirm_ambiguous_zero_remaining(api_key, model, result)
                        .await);
                }
                Err(error @ ProviderError::AuthRequired) => return Err(error),
                Err(error) => {
                    last_error = Some(error);
                }
            }
        }
        Err(last_error
            .unwrap_or_else(|| ProviderError::Other("All Doubao probe models failed".into())))
    }

    async fn confirm_ambiguous_zero_remaining(
        &self,
        api_key: &str,
        model: &str,
        initial: DoubaoProbeResult,
    ) -> UsageSnapshot {
        if !initial.has_ambiguous_zero_remaining() {
            return initial.snapshot;
        }

        match self.probe(api_key, model).await {
            Ok(confirmation) if confirmation.status == reqwest::StatusCode::TOO_MANY_REQUESTS => {
                initial.snapshot
            }
            Ok(confirmation) if confirmation.has_ambiguous_zero_remaining() => snapshot_from_parts(
                confirmation.remaining,
                confirmation.limit,
                confirmation.resets_at,
                confirmation.total_tokens,
                false,
            ),
            Ok(confirmation) => confirmation.snapshot,
            Err(error) => {
                tracing::warn!(
                    "Doubao zero-remaining confirmation failed; preserving initial exhausted state: {error}"
                );
                initial.snapshot
            }
        }
    }

    async fn probe(&self, api_key: &str, model: &str) -> Result<DoubaoProbeResult, ProviderError> {
        let response = self
            .client
            .post(DOUBAO_API_URL)
            .bearer_auth(api_key)
            .header("Accept", "application/json")
            .header("Content-Type", "application/json")
            .json(&json!({
                "model": model,
                "max_tokens": 1,
                "messages": [{"role": "user", "content": "hi"}],
            }))
            .send()
            .await?;

        if response.status() == reqwest::StatusCode::UNAUTHORIZED {
            return Err(ProviderError::AuthRequired);
        }

        let status = response.status();
        if status != reqwest::StatusCode::OK && status != reqwest::StatusCode::TOO_MANY_REQUESTS {
            return Err(ProviderError::Other(format!(
                "Doubao probe model {model} returned status {status}"
            )));
        }

        let headers = response.headers().clone();
        let body: serde_json::Value = response.json().await.unwrap_or_else(|_| json!({}));
        Ok(probe_result_from_response(status, &headers, &body))
    }
}

#[derive(Debug)]
struct DoubaoProbeResult {
    snapshot: UsageSnapshot,
    status: reqwest::StatusCode,
    remaining: Option<i64>,
    limit: Option<i64>,
    resets_at: Option<DateTime<Utc>>,
    total_tokens: Option<i64>,
    request_limits_reliable: bool,
}

impl DoubaoProbeResult {
    fn has_ambiguous_zero_remaining(&self) -> bool {
        self.status == reqwest::StatusCode::OK
            && self.request_limits_reliable
            && self.limit.is_some_and(|limit| limit > 0)
            && self.remaining == Some(0)
    }
}

fn probe_result_from_response(
    status: reqwest::StatusCode,
    headers: &reqwest::header::HeaderMap,
    body: &serde_json::Value,
) -> DoubaoProbeResult {
    let remaining = int_header(headers, "x-ratelimit-remaining-requests");
    let limit = int_header(headers, "x-ratelimit-limit-requests");
    let resets_at = string_header(headers, "x-ratelimit-reset-requests").and_then(parse_reset_time);
    let total_tokens = body
        .get("usage")
        .and_then(|usage| usage.get("total_tokens"))
        .and_then(|value| value.as_i64());
    let request_limits_reliable = if status == reqwest::StatusCode::TOO_MANY_REQUESTS {
        limit.is_some()
    } else {
        limit.is_some() && remaining.is_some()
    };

    let snapshot = snapshot_from_parts(
        remaining,
        limit,
        resets_at,
        total_tokens,
        request_limits_reliable,
    );

    DoubaoProbeResult {
        snapshot,
        status,
        remaining,
        limit,
        resets_at,
        total_tokens,
        request_limits_reliable,
    }
}

fn snapshot_from_parts(
    remaining: Option<i64>,
    limit: Option<i64>,
    resets_at: Option<DateTime<Utc>>,
    total_tokens: Option<i64>,
    request_limits_reliable: bool,
) -> UsageSnapshot {
    let effective_remaining = remaining.unwrap_or(0);

    let (used_percent, detail) = if let (Some(remaining), Some(limit)) = (remaining, limit) {
        if request_limits_reliable && limit > 0 {
            let used = (limit - remaining).max(0);
            let percent = used as f64 / limit as f64 * 100.0;
            (percent, format!("{used}/{limit} requests"))
        } else {
            (0.0, "Active - check dashboard for details".to_string())
        }
    } else if let Some(limit) = limit.filter(|limit| request_limits_reliable && *limit > 0) {
        let used = (limit - effective_remaining).max(0);
        let percent = if limit > 0 {
            used as f64 / limit as f64 * 100.0
        } else {
            0.0
        };
        (percent, format!("{used}/{limit} requests"))
    } else if let Some(total_tokens) = total_tokens {
        (0.0, format!("Active - {total_tokens} tokens observed"))
    } else {
        (0.0, "Active - check dashboard for details".to_string())
    };

    let mut window = RateWindow::with_details(used_percent, None, resets_at, Some(detail));
    if window.used_percent.is_nan() {
        window.used_percent = 0.0;
    }
    UsageSnapshot::new(window)
}

fn int_header(headers: &reqwest::header::HeaderMap, name: &str) -> Option<i64> {
    headers
        .get(name)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.parse::<i64>().ok())
}

fn string_header(headers: &reqwest::header::HeaderMap, name: &str) -> Option<String> {
    headers
        .get(name)
        .and_then(|value| value.to_str().ok())
        .map(|value| value.to_string())
}

fn parse_reset_time(value: String) -> Option<DateTime<Utc>> {
    let trimmed = value.trim();
    if let Ok(ts) = trimmed.parse::<i64>() {
        return Utc.timestamp_opt(ts, 0).single();
    }
    DateTime::parse_from_rfc3339(trimmed)
        .ok()
        .map(|dt| dt.with_timezone(&Utc))
}

impl Default for DoubaoProvider {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Provider for DoubaoProvider {
    fn id(&self) -> ProviderId {
        ProviderId::Doubao
    }

    fn metadata(&self) -> &ProviderMetadata {
        &self.metadata
    }

    async fn fetch_usage(&self, ctx: &FetchContext) -> Result<ProviderFetchResult, ProviderError> {
        match ctx.source_mode {
            SourceMode::Auto | SourceMode::OAuth => {
                let api_key = Self::api_key(ctx.api_key.as_deref())?;
                Ok(ProviderFetchResult::new(
                    self.fetch_api(&api_key).await?,
                    "api",
                ))
            }
            SourceMode::Web | SourceMode::Cli => {
                Err(ProviderError::UnsupportedSource(ctx.source_mode))
            }
        }
    }

    fn available_sources(&self) -> Vec<SourceMode> {
        vec![SourceMode::Auto, SourceMode::OAuth]
    }
}

fn resolve_api_key(
    explicit: Option<&str>,
    credential_target: &str,
    env_names: &[&str],
) -> Result<String, ProviderError> {
    if let Some(key) = explicit
        && !key.trim().is_empty()
    {
        return Ok(key.trim().to_string());
    }
    if let Ok(entry) = keyring::Entry::new(credential_target, "api_key")
        && let Ok(key) = entry.get_password()
        && !key.trim().is_empty()
    {
        return Ok(key);
    }
    for env in env_names {
        if let Ok(key) = std::env::var(env)
            && !key.trim().is_empty()
        {
            return Ok(key);
        }
    }
    Err(ProviderError::NotInstalled(format!(
        "API key not found. Set {} in Preferences or environment.",
        env_names.join(" / ")
    )))
}

#[cfg(test)]
mod tests {
    use super::*;
    use reqwest::header::{HeaderMap, HeaderValue};

    #[test]
    fn doubao_snapshot_uses_rate_limit_headers() {
        let mut headers = HeaderMap::new();
        headers.insert(
            "x-ratelimit-remaining-requests",
            HeaderValue::from_static("25"),
        );
        headers.insert(
            "x-ratelimit-limit-requests",
            HeaderValue::from_static("100"),
        );
        let snapshot =
            probe_result_from_response(reqwest::StatusCode::OK, &headers, &json!({})).snapshot;
        assert_eq!(snapshot.primary.used_percent, 75.0);
    }

    #[test]
    fn doubao_repeated_successful_zero_remaining_falls_back_to_active() {
        let mut headers = HeaderMap::new();
        headers.insert(
            "x-ratelimit-remaining-requests",
            HeaderValue::from_static("0"),
        );
        headers.insert(
            "x-ratelimit-limit-requests",
            HeaderValue::from_static("1000"),
        );

        let result = probe_result_from_response(reqwest::StatusCode::OK, &headers, &json!({}));
        assert!(result.has_ambiguous_zero_remaining());

        let snapshot = snapshot_from_parts(
            result.remaining,
            result.limit,
            result.resets_at,
            result.total_tokens,
            false,
        );
        assert_eq!(snapshot.primary.used_percent, 0.0);
        assert_eq!(
            snapshot.primary.reset_description.as_deref(),
            Some("Active - check dashboard for details")
        );
    }

    #[test]
    fn doubao_rate_limit_with_limit_header_reports_exhausted() {
        let mut headers = HeaderMap::new();
        headers.insert(
            "x-ratelimit-limit-requests",
            HeaderValue::from_static("1000"),
        );
        let snapshot = probe_result_from_response(
            reqwest::StatusCode::TOO_MANY_REQUESTS,
            &headers,
            &json!({}),
        )
        .snapshot;

        assert_eq!(snapshot.primary.used_percent, 100.0);
        assert_eq!(
            snapshot.primary.reset_description.as_deref(),
            Some("1000/1000 requests")
        );
    }

    #[test]
    fn doubao_bare_rate_limit_uses_active_fallback() {
        let snapshot = probe_result_from_response(
            reqwest::StatusCode::TOO_MANY_REQUESTS,
            &HeaderMap::new(),
            &json!({}),
        )
        .snapshot;

        assert_eq!(snapshot.primary.used_percent, 0.0);
        assert_eq!(
            snapshot.primary.reset_description.as_deref(),
            Some("Active - check dashboard for details")
        );
    }
}
