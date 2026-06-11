//! Crof provider implementation.
//!
//! Fetches API key based credit/request quota data from Crof.

use async_trait::async_trait;
use reqwest::Client;
use serde::Deserialize;

use crate::core::{
    FetchContext, Provider, ProviderError, ProviderFetchResult, ProviderId, ProviderMetadata,
    RateWindow, SourceMode, UsageSnapshot,
};

const CROF_USAGE_URL: &str = "https://crof.ai/usage_api/";
const CROF_CREDENTIAL_TARGET: &str = "codexbar-crof";

#[derive(Debug, Deserialize)]
struct CrofUsageResponse {
    credits: f64,
    #[serde(rename = "requests_plan")]
    requests_plan: f64,
    #[serde(rename = "usable_requests")]
    usable_requests: f64,
}

pub struct CrofProvider {
    metadata: ProviderMetadata,
    client: Client,
}

impl CrofProvider {
    pub fn new() -> Self {
        Self {
            metadata: ProviderMetadata {
                id: ProviderId::Crof,
                display_name: "Crof",
                session_label: "Requests",
                weekly_label: "Credits",
                supports_opus: false,
                supports_credits: true,
                default_enabled: false,
                is_primary: false,
                dashboard_url: Some("https://crof.ai"),
                status_page_url: None,
            },
            client: crate::core::credentialed_http_client_builder()
                .timeout(std::time::Duration::from_secs(15))
                .build()
                .unwrap_or_else(|_| Client::new()),
        }
    }

    fn api_key(api_key: Option<&str>) -> Result<String, ProviderError> {
        super_key(api_key, CROF_CREDENTIAL_TARGET, &["CROF_API_KEY"])
    }

    async fn fetch_api(&self, api_key: &str) -> Result<UsageSnapshot, ProviderError> {
        let response = self
            .client
            .get(CROF_USAGE_URL)
            .bearer_auth(api_key)
            .header("Accept", "application/json")
            .send()
            .await?;

        if response.status() == reqwest::StatusCode::UNAUTHORIZED
            || response.status() == reqwest::StatusCode::FORBIDDEN
        {
            return Err(ProviderError::AuthRequired);
        }
        if !response.status().is_success() {
            return Err(ProviderError::Other(format!(
                "Crof API returned status {}",
                response.status()
            )));
        }

        let usage: CrofUsageResponse = response
            .json()
            .await
            .map_err(|e| ProviderError::Parse(format!("Failed to parse Crof usage: {e}")))?;
        Ok(snapshot_from_usage(&usage))
    }
}

fn snapshot_from_usage(usage: &CrofUsageResponse) -> UsageSnapshot {
    let used = (usage.requests_plan - usage.usable_requests).max(0.0);
    let used_percent = if usage.requests_plan > 0.0 {
        used / usage.requests_plan * 100.0
    } else {
        0.0
    };

    let mut primary = RateWindow::new(used_percent);
    primary.reset_description = Some(format!(
        "{:.0}/{:.0} requests used",
        used, usage.requests_plan
    ));

    let mut secondary = RateWindow::new(0.0);
    secondary.reset_description = Some(format!("{:.2} credits remaining", usage.credits));

    UsageSnapshot::new(primary)
        .with_secondary(secondary)
        .with_login_method(format!("{:.2} credits", usage.credits))
}

impl Default for CrofProvider {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Provider for CrofProvider {
    fn id(&self) -> ProviderId {
        ProviderId::Crof
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

fn super_key(
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

    #[test]
    fn crof_snapshot_formats_request_and_credit_windows() {
        let snapshot = snapshot_from_usage(&CrofUsageResponse {
            credits: 12.5,
            requests_plan: 100.0,
            usable_requests: 25.0,
        });
        assert_eq!(snapshot.primary.used_percent, 75.0);
        assert_eq!(
            snapshot.secondary.unwrap().reset_description.unwrap(),
            "12.50 credits remaining"
        );
    }
}
