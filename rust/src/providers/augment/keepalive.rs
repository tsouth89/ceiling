//! Augment Session Keepalive
//!
//! Manages automatic session keepalive for Augment to prevent cookie expiration.
//! Monitors cookie expiration and proactively refreshes the session before
//! cookies expire, ensuring uninterrupted access to Augment APIs.

use chrono::{DateTime, Utc};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;
use tokio::sync::RwLock;

/// Configuration for the keepalive service
pub struct KeepaliveConfig {
    /// How often to check if session needs refresh (default: 5 minutes)
    pub check_interval: Duration,
    /// Refresh session this many seconds before cookie expiration (default: 5 minutes)
    pub refresh_buffer: Duration,
    /// Minimum time between refresh attempts (default: 2 minutes)
    pub min_refresh_interval: Duration,
    /// Maximum time to wait for session refresh (default: 30 seconds)
    pub refresh_timeout: Duration,
}

impl Default for KeepaliveConfig {
    fn default() -> Self {
        Self {
            check_interval: Duration::from_secs(300),       // 5 minutes
            refresh_buffer: Duration::from_secs(300),       // 5 minutes
            min_refresh_interval: Duration::from_secs(120), // 2 minutes
            refresh_timeout: Duration::from_secs(30),
        }
    }
}

/// Session keepalive manager for Augment
pub struct AugmentSessionKeepalive {
    config: KeepaliveConfig,
    is_running: Arc<AtomicBool>,
    last_refresh_attempt: Arc<RwLock<Option<DateTime<Utc>>>>,
    last_successful_refresh: Arc<RwLock<Option<DateTime<Utc>>>>,
    is_refreshing: Arc<AtomicBool>,
}

impl AugmentSessionKeepalive {
    /// Create a new keepalive manager with default config
    pub fn new() -> Self {
        Self::with_config(KeepaliveConfig::default())
    }

    /// Create a new keepalive manager with custom config
    pub fn with_config(config: KeepaliveConfig) -> Self {
        Self {
            config,
            is_running: Arc::new(AtomicBool::new(false)),
            last_refresh_attempt: Arc::new(RwLock::new(None)),
            last_successful_refresh: Arc::new(RwLock::new(None)),
            is_refreshing: Arc::new(AtomicBool::new(false)),
        }
    }

    /// Start the automatic session keepalive timer
    pub fn start(&self) -> tokio::task::JoinHandle<()> {
        if self.is_running.load(Ordering::SeqCst) {
            tracing::warn!("[AugmentKeepalive] Keepalive already running");
            return tokio::spawn(async {});
        }

        self.is_running.store(true, Ordering::SeqCst);

        tracing::info!("[AugmentKeepalive] Starting session keepalive");
        tracing::debug!(
            "[AugmentKeepalive] Check interval: {:?}, Refresh buffer: {:?}",
            self.config.check_interval,
            self.config.refresh_buffer
        );

        let is_running = self.is_running.clone();
        let config = KeepaliveConfig {
            check_interval: self.config.check_interval,
            refresh_buffer: self.config.refresh_buffer,
            min_refresh_interval: self.config.min_refresh_interval,
            refresh_timeout: self.config.refresh_timeout,
        };
        let last_refresh_attempt = self.last_refresh_attempt.clone();
        let last_successful_refresh = self.last_successful_refresh.clone();
        let is_refreshing = self.is_refreshing.clone();

        tokio::spawn(async move {
            while is_running.load(Ordering::SeqCst) {
                tokio::time::sleep(config.check_interval).await;

                if !is_running.load(Ordering::SeqCst) {
                    break;
                }

                // Check and refresh if needed
                let should_refresh = Self::should_refresh_session(
                    &config,
                    &last_refresh_attempt,
                    &last_successful_refresh,
                )
                .await;

                if should_refresh {
                    Self::perform_refresh(
                        &config,
                        &last_refresh_attempt,
                        &last_successful_refresh,
                        &is_refreshing,
                        false,
                    )
                    .await;
                }
            }
            tracing::info!("[AugmentKeepalive] Keepalive stopped");
        })
    }

    /// Stop the automatic session keepalive timer
    pub fn stop(&self) {
        tracing::info!("[AugmentKeepalive] Stopping session keepalive");
        self.is_running.store(false, Ordering::SeqCst);
    }

    /// Check if the keepalive is running
    pub fn is_running(&self) -> bool {
        self.is_running.load(Ordering::SeqCst)
    }

    /// Manually trigger a session refresh (bypasses rate limiting)
    pub async fn force_refresh(&self) {
        tracing::info!("[AugmentKeepalive] Force refresh requested");
        Self::perform_refresh(
            &self.config,
            &self.last_refresh_attempt,
            &self.last_successful_refresh,
            &self.is_refreshing,
            true,
        )
        .await;
    }

    /// Check if we should refresh the session
    async fn should_refresh_session(
        config: &KeepaliveConfig,
        last_refresh_attempt: &Arc<RwLock<Option<DateTime<Utc>>>>,
        last_successful_refresh: &Arc<RwLock<Option<DateTime<Utc>>>>,
    ) -> bool {
        // Rate limit check
        if let Some(last_attempt) = *last_refresh_attempt.read().await {
            let elapsed = Utc::now().signed_duration_since(last_attempt);
            if elapsed < chrono::Duration::from_std(config.min_refresh_interval).unwrap_or_default()
            {
                tracing::debug!(
                    "[AugmentKeepalive] Skipping refresh (last attempt {:?} ago)",
                    elapsed
                );
                return false;
            }
        }

        // Check if we need periodic refresh (every 30 minutes for session cookies)
        if let Some(last_success) = *last_successful_refresh.read().await {
            let elapsed = Utc::now().signed_duration_since(last_success);
            if elapsed > chrono::Duration::seconds(1800) {
                tracing::info!(
                    "[AugmentKeepalive] Need periodic refresh ({:?} since last refresh)",
                    elapsed
                );
                return true;
            }
        } else {
            // Never refreshed - do it now
            tracing::info!("[AugmentKeepalive] Never refreshed - doing initial refresh");
            return true;
        }

        false
    }

    /// Perform the session refresh
    async fn perform_refresh(
        config: &KeepaliveConfig,
        last_refresh_attempt: &Arc<RwLock<Option<DateTime<Utc>>>>,
        last_successful_refresh: &Arc<RwLock<Option<DateTime<Utc>>>>,
        is_refreshing: &Arc<AtomicBool>,
        forced: bool,
    ) {
        if is_refreshing.swap(true, Ordering::SeqCst) {
            tracing::warn!("[AugmentKeepalive] Refresh already in progress");
            return;
        }

        *last_refresh_attempt.write().await = Some(Utc::now());

        let action = if forced { "forced" } else { "automatic" };
        tracing::info!(
            "[AugmentKeepalive] Performing {} session refresh...",
            action
        );

        // Try to ping session endpoints
        match Self::ping_session_endpoint(config).await {
            Ok(success) => {
                if success {
                    tracing::info!("[AugmentKeepalive] Session refresh successful");
                    *last_successful_refresh.write().await = Some(Utc::now());
                } else {
                    tracing::warn!("[AugmentKeepalive] Session refresh returned no new data");
                }
            }
            Err(e) => {
                tracing::error!("[AugmentKeepalive] Session refresh failed: {}", e);
            }
        }

        is_refreshing.store(false, Ordering::SeqCst);
    }

    /// Ping Augment's session endpoint to trigger cookie refresh
    async fn ping_session_endpoint(config: &KeepaliveConfig) -> Result<bool, String> {
        let client = crate::core::credentialed_http_client_builder()
            .timeout(config.refresh_timeout)
            .build()
            .map_err(|e| e.to_string())?;

        // Session endpoints to try
        let endpoints = [
            "https://app.augmentcode.com/api/auth/session",
            "https://app.augmentcode.com/api/session",
            "https://app.augmentcode.com/api/user",
        ];

        for endpoint in endpoints {
            tracing::debug!("[AugmentKeepalive] Trying endpoint: {}", endpoint);

            match client
                .get(endpoint)
                .header("Accept", "application/json")
                .header("Origin", "https://app.augmentcode.com")
                .header("Referer", "https://app.augmentcode.com")
                .send()
                .await
            {
                Ok(resp) => {
                    let status = resp.status();
                    tracing::debug!("[AugmentKeepalive] Response: HTTP {}", status);

                    if status.is_success() {
                        if let Ok(json) = resp.json::<serde_json::Value>().await {
                            // Check if we got valid session data
                            if json.get("user").is_some()
                                || json.get("email").is_some()
                                || json.get("session").is_some()
                            {
                                tracing::debug!("[AugmentKeepalive] Valid session data found");
                                return Ok(true);
                            }
                        }
                    } else if status.as_u16() == 401 {
                        return Err("Session expired".to_string());
                    }
                    // Try next endpoint
                }
                Err(e) => {
                    tracing::debug!("[AugmentKeepalive] Request failed: {}", e);
                    // Try next endpoint
                }
            }
        }

        Ok(false)
    }
}

impl Default for AugmentSessionKeepalive {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_keepalive_config_default() {
        let config = KeepaliveConfig::default();
        assert_eq!(config.check_interval, Duration::from_secs(300));
        assert_eq!(config.refresh_buffer, Duration::from_secs(300));
    }

    #[test]
    fn test_keepalive_new() {
        let keepalive = AugmentSessionKeepalive::new();
        assert!(!keepalive.is_running());
    }
}
