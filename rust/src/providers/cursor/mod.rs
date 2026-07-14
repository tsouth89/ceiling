//! Cursor provider implementation
//!
//! Fetches usage data from Cursor's API using:
//! 1. Pasted / imported session cookies
//! 2. The local Cursor IDE session (`state.vscdb`)
//! 3. Browser cookies for cursor.com / cursor.sh

mod api;
mod session;

use async_trait::async_trait;

use crate::core::{
    FetchContext, Provider, ProviderError, ProviderFetchResult, ProviderId, ProviderMetadata,
    SourceMode,
};

pub use api::CursorApi;
pub use session::normalize_cookie_header;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CursorIdeSessionStatus {
    Ready,
    Locked,
    SignedOut,
    Unavailable,
}

/// Probe Cursor's local IDE session while keeping the access token inside the
/// provider boundary.
pub fn ide_session_status() -> CursorIdeSessionStatus {
    match session::disk_session_cookie() {
        Ok(_) => CursorIdeSessionStatus::Ready,
        Err(ProviderError::NotInstalled(_)) => CursorIdeSessionStatus::Unavailable,
        Err(ProviderError::AuthRequired) | Err(ProviderError::Parse(_)) => {
            CursorIdeSessionStatus::SignedOut
        }
        Err(ProviderError::Other(message)) if is_windows_file_lock_error(&message) => {
            CursorIdeSessionStatus::Locked
        }
        Err(_) => CursorIdeSessionStatus::SignedOut,
    }
}

fn is_windows_file_lock_error(message: &str) -> bool {
    let lower = message.to_ascii_lowercase();
    lower.contains("os error 32")
        || lower.contains("being used by another process")
        || lower.contains("sharing violation")
}

/// Cursor provider for fetching AI usage limits
pub struct CursorProvider {
    metadata: ProviderMetadata,
    api: CursorApi,
}

impl CursorProvider {
    pub fn new() -> Self {
        Self {
            metadata: ProviderMetadata {
                id: ProviderId::Cursor,
                display_name: "Cursor",
                session_label: "Plan",
                weekly_label: "Auto",
                supports_opus: false,
                supports_credits: true,
                default_enabled: true,
                is_primary: false,
                dashboard_url: Some("https://cursor.com/dashboard/usage"),
                status_page_url: None,
            },
            api: CursorApi::new(),
        }
    }

    async fn fetch_usage_parts(
        &self,
        ctx: &FetchContext,
    ) -> Result<api::CursorUsageResult, ProviderError> {
        match ctx.source_mode {
            SourceMode::Auto | SourceMode::Web => self.fetch_web_usage_parts(ctx).await,
            SourceMode::Cli | SourceMode::OAuth => {
                Err(ProviderError::UnsupportedSource(ctx.source_mode))
            }
        }
    }

    async fn fetch_web_usage_parts(
        &self,
        ctx: &FetchContext,
    ) -> Result<api::CursorUsageResult, ProviderError> {
        let cookie_header = self.resolve_cookie_header(ctx)?;
        self.api
            .fetch_usage_with_cookie_header(&cookie_header)
            .await
    }

    fn resolve_cookie_header(&self, ctx: &FetchContext) -> Result<String, ProviderError> {
        if let Some(raw) = ctx.manual_cookie_header.as_deref() {
            if let Some(header) = normalize_cookie_header(raw) {
                return Ok(header);
            }
            return Err(ProviderError::Other(
                "Cursor session paste was not recognized. Paste WorkosCursorSessionToken=… from cursor.com cookies, or the bare session value / JWT.".to_string(),
            ));
        }

        match session::disk_session_cookie() {
            Ok(header) => {
                tracing::debug!("Using Cursor IDE disk session for usage fetch");
                return Ok(header);
            }
            Err(ProviderError::NotInstalled(_)) | Err(ProviderError::AuthRequired) => {}
            Err(err) => {
                tracing::debug!("Cursor IDE disk session unavailable: {err}");
            }
        }

        match self.api.get_cookie_header() {
            Ok(header) => {
                if let Some(normalized) = normalize_cookie_header(&header) {
                    Ok(normalized)
                } else {
                    Ok(header)
                }
            }
            Err(err) => {
                tracing::debug!("Cursor browser cookie lookup failed: {err}");
                Err(ProviderError::AuthRequired)
            }
        }
    }
}

impl Default for CursorProvider {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Provider for CursorProvider {
    fn id(&self) -> ProviderId {
        ProviderId::Cursor
    }

    fn metadata(&self) -> &ProviderMetadata {
        &self.metadata
    }

    async fn fetch_usage(&self, ctx: &FetchContext) -> Result<ProviderFetchResult, ProviderError> {
        tracing::debug!("Fetching Cursor usage via web API");

        match self.fetch_usage_parts(ctx).await {
            Ok((usage, cost)) => {
                let mut result = ProviderFetchResult::new(usage, "web");
                if let Some(c) = cost {
                    result = result.with_cost(c);
                }
                Ok(result)
            }
            Err(e) => {
                tracing::warn!("Cursor API fetch failed: {}", e);
                Err(match e {
                    ProviderError::AuthRequired => ProviderError::Other(
                        "Cursor auth failed. Automatic uses your signed-in Cursor IDE session when available. Otherwise paste WorkosCursorSessionToken from cursor.com (Application → Cookies), or import via Firefox if Chrome/Edge blocks cookie access.".to_string(),
                    ),
                    other => other,
                })
            }
        }
    }

    fn available_sources(&self) -> Vec<SourceMode> {
        vec![SourceMode::Auto, SourceMode::Web]
    }

    fn supports_web(&self) -> bool {
        true
    }
}
