//! StepFun provider implementation.
//!
//! Supports an existing Oasis-Token via Preferences/environment. The upstream
//! username/password login flow is intentionally not automated in the Windows
//! shell yet; storing the resulting token keeps the provider usable without
//! retaining a password.

use async_trait::async_trait;
use chrono::{DateTime, TimeZone, Utc};
use reqwest::Client;
use serde::Deserialize;

use crate::core::{
    FetchContext, Provider, ProviderError, ProviderFetchResult, ProviderId, ProviderMetadata,
    RateWindow, SourceMode, UsageSnapshot, format_remaining_countdown,
};

const STEPFUN_RATE_LIMIT_URL: &str =
    "https://platform.stepfun.com/api/step.openapi.devcenter.Dashboard/QueryStepPlanRateLimit";
const STEPFUN_PLAN_STATUS_URL: &str =
    "https://platform.stepfun.com/api/step.openapi.devcenter.Dashboard/GetStepPlanStatus";
const STEPFUN_REFRESH_TOKEN_URL: &str =
    "https://platform.stepfun.com/passport/proto.api.passport.v1.PassportService/RefreshToken";
const STEPFUN_CREDENTIAL_TARGET: &str = "codexbar-stepfun";
const STEPFUN_WEB_ID: &str = "734152690100432";
const STEPFUN_APP_ID: &str = "111003695";

#[derive(Debug, Deserialize)]
struct StepFunRateLimitResponse {
    status: Option<i64>,
    code: Option<i64>,
    message: Option<String>,
    desc: Option<String>,
    five_hour_usage_left_rate: Option<FlexibleNumber>,
    weekly_usage_left_rate: Option<FlexibleNumber>,
    five_hour_usage_reset_time: Option<FlexibleTimestamp>,
    weekly_usage_reset_time: Option<FlexibleTimestamp>,
}

#[derive(Debug, Deserialize)]
struct FlexibleNumber(#[serde(deserialize_with = "deserialize_f64")] f64);

#[derive(Debug, Deserialize)]
struct FlexibleTimestamp(#[serde(deserialize_with = "deserialize_i64")] i64);

#[derive(Debug, Deserialize)]
struct StepFunPlanStatusResponse {
    status: Option<i64>,
    subscription: Option<StepFunSubscription>,
}

#[derive(Debug, Deserialize)]
struct StepFunSubscription {
    name: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct StepFunRefreshTokenResponse {
    access_token: Option<StepFunTokenPair>,
    refresh_token: Option<StepFunTokenPair>,
}

#[derive(Debug, Deserialize)]
struct StepFunTokenPair {
    raw: String,
}

pub struct StepFunProvider {
    metadata: ProviderMetadata,
    client: Client,
}

impl StepFunProvider {
    pub fn new() -> Self {
        Self {
            metadata: ProviderMetadata {
                id: ProviderId::StepFun,
                display_name: "StepFun",
                session_label: "5-hour",
                weekly_label: "Weekly",
                supports_opus: false,
                supports_credits: false,
                default_enabled: false,
                is_primary: false,
                dashboard_url: Some("https://platform.stepfun.com/dashboard"),
                status_page_url: None,
            },
            client: crate::core::credentialed_http_client_builder()
                .timeout(std::time::Duration::from_secs(15))
                .build()
                .unwrap_or_else(|_| Client::new()),
        }
    }

    fn token(api_key: Option<&str>) -> Result<String, ProviderError> {
        resolve_token(
            api_key,
            STEPFUN_CREDENTIAL_TARGET,
            &["STEPFUN_OASIS_TOKEN", "STEPFUN_TOKEN"],
        )
    }

    async fn fetch_token(&self, token: &str) -> Result<UsageSnapshot, ProviderError> {
        match self.fetch_token_once(token).await {
            Ok(snapshot) => Ok(snapshot),
            Err(error)
                if is_authentication_failure(&error)
                    && token_parts(token).refresh_token.is_some() =>
            {
                let refreshed = self.refresh_token(token).await?;
                self.persist_refreshed_token(&refreshed);
                self.fetch_token_once(&refreshed)
                    .await
                    .map_err(|retry_error| {
                        if is_authentication_failure(&retry_error) {
                            ProviderError::AuthRequired
                        } else {
                            retry_error
                        }
                    })
            }
            Err(error) => Err(error),
        }
    }

    async fn fetch_token_once(&self, token: &str) -> Result<UsageSnapshot, ProviderError> {
        let normalized = normalize_token(token);
        let rate_limit = self
            .post_json::<StepFunRateLimitResponse>(STEPFUN_RATE_LIMIT_URL, &normalized)
            .await?;
        let plan_name = self
            .post_json::<StepFunPlanStatusResponse>(STEPFUN_PLAN_STATUS_URL, &normalized)
            .await
            .ok()
            .and_then(|response| {
                (response.status == Some(1))
                    .then_some(response.subscription)
                    .flatten()
                    .and_then(|subscription| subscription.name)
            });
        snapshot_from_response(&rate_limit, plan_name)
    }

    async fn refresh_token(&self, token: &str) -> Result<String, ProviderError> {
        let normalized = normalize_token(token);
        let response = self
            .post_json::<StepFunRefreshTokenResponse>(STEPFUN_REFRESH_TOKEN_URL, &normalized)
            .await?;
        let access = response
            .access_token
            .map(|token| token.raw)
            .filter(|token| !token.trim().is_empty())
            .ok_or_else(|| ProviderError::AuthRequired)?;
        Ok(combined_token(
            &access,
            response
                .refresh_token
                .as_ref()
                .map(|token| token.raw.as_str()),
        ))
    }

    fn persist_refreshed_token(&self, token: &str) {
        persist_refreshed_token_in(&OsTokenSecretStore, token);
    }

    async fn post_json<T: for<'de> Deserialize<'de>>(
        &self,
        url: &str,
        token: &str,
    ) -> Result<T, ProviderError> {
        let response = self
            .client
            .post(url)
            .header("content-type", "application/json")
            .header("oasis-appid", STEPFUN_APP_ID)
            .header("oasis-platform", "web")
            .header("oasis-webid", STEPFUN_WEB_ID)
            .header(
                "Cookie",
                format!("Oasis-Token={token}; Oasis-Webid={STEPFUN_WEB_ID}"),
            )
            .body("{}")
            .send()
            .await?;

        if response.status() == reqwest::StatusCode::UNAUTHORIZED
            || response.status() == reqwest::StatusCode::FORBIDDEN
        {
            return Err(ProviderError::AuthRequired);
        }
        if !response.status().is_success() {
            return Err(ProviderError::Other(format!(
                "StepFun API returned status {}",
                response.status()
            )));
        }
        response
            .json::<T>()
            .await
            .map_err(|e| ProviderError::Parse(format!("Failed to parse StepFun response: {e}")))
    }
}

fn snapshot_from_response(
    response: &StepFunRateLimitResponse,
    plan_name: Option<String>,
) -> Result<UsageSnapshot, ProviderError> {
    if response.status != Some(1) {
        let msg = response
            .message
            .clone()
            .or_else(|| response.desc.clone())
            .or_else(|| response.code.map(|code| code.to_string()))
            .unwrap_or_else(|| "unknown".into());
        if is_authentication_message(&msg) {
            return Err(ProviderError::AuthRequired);
        }
        return Err(ProviderError::Other(format!("StepFun API error: {msg}")));
    }

    let five_left = response
        .five_hour_usage_left_rate
        .as_ref()
        .ok_or_else(|| ProviderError::Parse("Missing StepFun five-hour usage".into()))?
        .0;
    let weekly_left = response
        .weekly_usage_left_rate
        .as_ref()
        .ok_or_else(|| ProviderError::Parse("Missing StepFun weekly usage".into()))?
        .0;
    let five_reset = response
        .five_hour_usage_reset_time
        .as_ref()
        .and_then(|ts| Utc.timestamp_opt(ts.0, 0).single());
    let weekly_reset = response
        .weekly_usage_reset_time
        .as_ref()
        .and_then(|ts| Utc.timestamp_opt(ts.0, 0).single());

    let primary = RateWindow::with_details(
        (1.0 - five_left).clamp(0.0, 1.0) * 100.0,
        Some(300),
        five_reset,
        five_reset.map(reset_description),
    );
    let secondary = RateWindow::with_details(
        (1.0 - weekly_left).clamp(0.0, 1.0) * 100.0,
        Some(10080),
        weekly_reset,
        weekly_reset.map(reset_description),
    );

    let mut snapshot = UsageSnapshot::new(primary).with_secondary(secondary);
    if let Some(plan_name) = plan_name.filter(|value| !value.trim().is_empty()) {
        snapshot = snapshot.with_login_method(plan_name);
    } else {
        snapshot = snapshot.with_login_method("Oasis-Token");
    }
    Ok(snapshot)
}

struct StepFunTokenParts {
    access_token: String,
    refresh_token: Option<String>,
}

fn token_parts(token: &str) -> StepFunTokenParts {
    let normalized = normalize_token(token);
    let (access_token, refresh_token) = normalized
        .split_once("...")
        .map(|(access, refresh)| {
            (
                access.trim().to_string(),
                Some(refresh.trim().to_string()).filter(|value| !value.is_empty()),
            )
        })
        .unwrap_or_else(|| (normalized.trim().to_string(), None));
    StepFunTokenParts {
        access_token,
        refresh_token,
    }
}

fn normalize_token(raw: &str) -> String {
    let trimmed = raw.trim();
    if let Some((_, tail)) = trimmed.split_once("Oasis-Token=") {
        return tail.split(';').next().unwrap_or(tail).trim().to_string();
    }
    trimmed.to_string()
}

fn combined_token(access_token: &str, refresh_token: Option<&str>) -> String {
    match refresh_token
        .map(str::trim)
        .filter(|token| !token.is_empty())
    {
        Some(refresh_token) => format!("{}...{}", access_token.trim(), refresh_token),
        None => access_token.trim().to_string(),
    }
}

fn is_authentication_failure(error: &ProviderError) -> bool {
    matches!(error, ProviderError::AuthRequired)
        || match error {
            ProviderError::Other(message) | ProviderError::Parse(message) => {
                is_authentication_message(message)
            }
            _ => false,
        }
}

fn is_authentication_message(message: &str) -> bool {
    let lower = message.to_lowercase();
    lower.contains("401")
        || lower.contains("403")
        || lower.contains("unauthorized")
        || lower.contains("unauthenticated")
        || lower.contains("invalid credentials")
        || lower.contains("invalid token")
        || lower.contains("token expired")
        || lower.contains("expired token")
}

fn reset_description(date: DateTime<Utc>) -> String {
    let now = Utc::now();
    if date <= now {
        return "resets now".into();
    }
    format!("resets in {}", format_remaining_countdown(date - now))
}

fn deserialize_f64<'de, D>(deserializer: D) -> Result<f64, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let value = serde_json::Value::deserialize(deserializer)?;
    match value {
        serde_json::Value::Number(n) => n
            .as_f64()
            .ok_or_else(|| serde::de::Error::custom("invalid number")),
        serde_json::Value::String(s) => s
            .parse::<f64>()
            .map_err(|_| serde::de::Error::custom("invalid number string")),
        _ => Ok(0.0),
    }
}

fn deserialize_i64<'de, D>(deserializer: D) -> Result<i64, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let value = serde_json::Value::deserialize(deserializer)?;
    match value {
        serde_json::Value::Number(n) => n
            .as_i64()
            .ok_or_else(|| serde::de::Error::custom("invalid timestamp")),
        serde_json::Value::String(s) => s
            .parse::<i64>()
            .map_err(|_| serde::de::Error::custom("invalid timestamp string")),
        _ => Ok(0),
    }
}

impl Default for StepFunProvider {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Provider for StepFunProvider {
    fn id(&self) -> ProviderId {
        ProviderId::StepFun
    }

    fn metadata(&self) -> &ProviderMetadata {
        &self.metadata
    }

    async fn fetch_usage(&self, ctx: &FetchContext) -> Result<ProviderFetchResult, ProviderError> {
        match ctx.source_mode {
            SourceMode::Auto | SourceMode::OAuth => {
                let token = Self::token(ctx.api_key.as_deref())?;
                Ok(ProviderFetchResult::new(
                    self.fetch_token(&token).await?,
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

/// Write the refreshed Oasis token to the keyring, unless the credential it
/// belongs to was revoked while the refresh was in flight.
///
/// A refresh only runs after an auth failure, which is also the moment someone
/// is most likely to be signing out. Revoke takes the state write lock, deletes
/// the keyring copy, confirms it is gone, and reports success; an unguarded
/// write landing after that put a live token back and the session stayed signed
/// in (SBS-920). Taking the same lock and re-reading Preferences under it is
/// what makes the two orders decide, rather than race.
fn persist_refreshed_token_in(store: &impl TokenSecretStore, token: &str) {
    persist_refreshed_token_checked(store, token, stepfun_credential_configured)
}

/// Persist under the state lock, asking `configured` while holding it.
///
/// The predicate is a parameter so a test can drive the real locked path
/// rather than only the decision it reaches.
fn persist_refreshed_token_checked(
    store: &impl TokenSecretStore,
    token: &str,
    configured: impl FnOnce() -> bool,
) {
    let locked = crate::secure_file::with_state_write_lock(|| {
        Ok(persist_refreshed_token_when(store, token, configured()))
    });
    if let Err(error) = locked {
        tracing::debug!(
            "Could not take the state lock to persist refreshed StepFun token: {error}"
        );
    }
}

/// Whether StepFun still has a credential this refreshed token could belong to.
///
/// Preferences is what revoke clears. An environment variable is not Ceiling's
/// to remove and authenticates on its own, so a machine configured that way
/// keeps refreshing normally.
fn stepfun_credential_configured() -> bool {
    if crate::settings::provider_credential_present(crate::core::ProviderId::StepFun) {
        return true;
    }
    ["STEPFUN_OASIS_TOKEN", "STEPFUN_TOKEN"]
        .iter()
        .any(|name| std::env::var(name).is_ok_and(|value| !value.trim().is_empty()))
}

/// The decision itself, separated from the lock and the disk read so a test can
/// state the revoked case directly.
fn persist_refreshed_token_when(
    store: &impl TokenSecretStore,
    token: &str,
    still_configured: bool,
) -> bool {
    if !still_configured {
        tracing::debug!("StepFun credential was revoked mid-refresh; refreshed token not stored");
        return false;
    }
    if let Err(error) = store.set(STEPFUN_CREDENTIAL_TARGET, "api_key", token) {
        tracing::debug!("Could not persist refreshed StepFun token: {error}");
        return false;
    }
    true
}

/// Delete the refreshed Oasis token Ceiling wrote to the OS keyring.
///
/// `revoke_managed_credentials` clears Preferences / cookies / token-accounts
/// only. StepFun's live refresh path also writes `codexbar-stepfun` / `api_key`,
/// and `resolve_token` reads that copy after the Preferences key is gone
/// (SBS-920). Missing is success; any other keyring error fails closed so
/// revoke cannot report success while the token remains.
pub(crate) fn clear_persisted_credentials() -> anyhow::Result<()> {
    clear_token_secret(&OsTokenSecretStore)
}

fn clear_token_secret(store: &impl TokenSecretStore) -> anyhow::Result<()> {
    store
        .delete(STEPFUN_CREDENTIAL_TARGET, "api_key")
        .map_err(|error| anyhow::anyhow!("Could not delete StepFun keyring token: {error}"))?;
    match store.get(STEPFUN_CREDENTIAL_TARGET, "api_key") {
        Ok(None) => Ok(()),
        Ok(Some(_)) => Err(anyhow::anyhow!(
            "StepFun keyring token was still present after delete"
        )),
        Err(error) => Err(anyhow::anyhow!(
            "Could not confirm StepFun keyring token was deleted: {error}"
        )),
    }
}

trait TokenSecretStore {
    fn get(&self, service: &str, user: &str) -> Result<Option<String>, String>;
    fn set(&self, service: &str, user: &str, value: &str) -> Result<(), String>;
    fn delete(&self, service: &str, user: &str) -> Result<(), String>;
}

struct OsTokenSecretStore;

impl TokenSecretStore for OsTokenSecretStore {
    fn get(&self, service: &str, user: &str) -> Result<Option<String>, String> {
        match crate::keychain::get_password(crate::keychain::Scope::Any, service, user) {
            Ok(value) if !value.trim().is_empty() => Ok(Some(value)),
            Ok(_) | Err(crate::keychain::Error::NotFound) => Ok(None),
            Err(error) => Err(error.to_string()),
        }
    }

    fn set(&self, service: &str, user: &str, value: &str) -> Result<(), String> {
        crate::keychain::set_password(crate::keychain::Scope::Any, service, user, value)
            .map_err(|error| error.to_string())
    }

    fn delete(&self, service: &str, user: &str) -> Result<(), String> {
        match crate::keychain::delete_credential(crate::keychain::Scope::Any, service, user) {
            Ok(()) | Err(crate::keychain::Error::NotFound) => Ok(()),
            Err(error) => Err(error.to_string()),
        }
    }
}

fn resolve_token(
    explicit: Option<&str>,
    credential_target: &str,
    env_names: &[&str],
) -> Result<String, ProviderError> {
    resolve_token_in(&OsTokenSecretStore, explicit, credential_target, env_names)
}

fn resolve_token_in(
    store: &impl TokenSecretStore,
    explicit: Option<&str>,
    credential_target: &str,
    env_names: &[&str],
) -> Result<String, ProviderError> {
    if let Some(key) = explicit
        && !key.trim().is_empty()
    {
        return Ok(key.trim().to_string());
    }
    match store.get(credential_target, "api_key") {
        Ok(Some(key)) if !key.trim().is_empty() => return Ok(key),
        // Empty, missing, or unreadable: fall through to env, matching the
        // previous resolver. Revoke does not use this path —
        // `clear_token_secret` fails closed on the same unknown.
        Ok(Some(_)) | Ok(None) | Err(_) => {}
    }
    for env in env_names {
        if let Ok(key) = std::env::var(env)
            && !key.trim().is_empty()
        {
            return Ok(key);
        }
    }
    Err(ProviderError::NotInstalled(format!(
        "StepFun token not found. Set {} in Preferences or environment.",
        env_names.join(" / ")
    )))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use std::sync::Mutex;

    struct MemoryTokenSecretStore {
        inner: Mutex<HashMap<(String, String), String>>,
    }

    impl MemoryTokenSecretStore {
        fn new() -> Self {
            Self {
                inner: Mutex::new(HashMap::new()),
            }
        }
    }

    impl TokenSecretStore for MemoryTokenSecretStore {
        fn get(&self, service: &str, user: &str) -> Result<Option<String>, String> {
            Ok(self
                .inner
                .lock()
                .expect("memory token store lock")
                .get(&(service.to_string(), user.to_string()))
                .cloned())
        }

        fn set(&self, service: &str, user: &str, value: &str) -> Result<(), String> {
            self.inner
                .lock()
                .expect("memory token store lock")
                .insert((service.to_string(), user.to_string()), value.to_string());
            Ok(())
        }

        fn delete(&self, service: &str, user: &str) -> Result<(), String> {
            self.inner
                .lock()
                .expect("memory token store lock")
                .remove(&(service.to_string(), user.to_string()));
            Ok(())
        }
    }

    struct FailingDeleteStore;

    impl TokenSecretStore for FailingDeleteStore {
        fn get(&self, _service: &str, _user: &str) -> Result<Option<String>, String> {
            Ok(Some("leftover-oasis-token".to_string()))
        }

        fn set(&self, _service: &str, _user: &str, _value: &str) -> Result<(), String> {
            Ok(())
        }

        fn delete(&self, _service: &str, _user: &str) -> Result<(), String> {
            Err("keyring locked".to_string())
        }
    }

    /// Delete reports success but leaves the secret in place — the revoke
    /// fail-open that SBS-920 is. Confirmation must reject this.
    struct LyingDeleteStore {
        inner: MemoryTokenSecretStore,
    }

    impl LyingDeleteStore {
        fn new() -> Self {
            Self {
                inner: MemoryTokenSecretStore::new(),
            }
        }
    }

    impl TokenSecretStore for LyingDeleteStore {
        fn get(&self, service: &str, user: &str) -> Result<Option<String>, String> {
            self.inner.get(service, user)
        }

        fn set(&self, service: &str, user: &str, value: &str) -> Result<(), String> {
            self.inner.set(service, user, value)
        }

        fn delete(&self, _service: &str, _user: &str) -> Result<(), String> {
            Ok(())
        }
    }

    #[test]
    fn stepfun_snapshot_converts_left_rates_to_used_percent() {
        let response = StepFunRateLimitResponse {
            status: Some(1),
            code: None,
            message: None,
            desc: None,
            five_hour_usage_left_rate: Some(FlexibleNumber(0.25)),
            weekly_usage_left_rate: Some(FlexibleNumber(0.75)),
            five_hour_usage_reset_time: Some(FlexibleTimestamp(1_800_000_000)),
            weekly_usage_reset_time: Some(FlexibleTimestamp(1_800_000_000)),
        };
        let snapshot = snapshot_from_response(&response, Some("Step Plan".into())).unwrap();
        assert_eq!(snapshot.primary.used_percent, 75.0);
        assert_eq!(snapshot.secondary.unwrap().used_percent, 25.0);
    }

    #[test]
    fn stepfun_token_parts_extract_cookie_and_refresh_token() {
        let parts = token_parts("Cookie: Oasis-Token=access...refresh; Oasis-Webid=abc");
        assert_eq!(parts.access_token, "access");
        assert_eq!(parts.refresh_token.as_deref(), Some("refresh"));
        assert_eq!(
            combined_token("new-access", Some("new-refresh")),
            "new-access...new-refresh"
        );
    }

    #[test]
    fn stepfun_authentication_messages_are_actionable() {
        assert!(is_authentication_message("token expired"));
        assert!(is_authentication_message("HTTP 401"));
        assert!(!is_authentication_message("rate limit"));
    }

    /// SBS-920: after revoke, `resolve_token` must not revive the session from
    /// the leftover keyring copy `persist_refreshed_token` wrote.
    #[test]
    fn revoke_clears_refreshed_keyring_token_so_resolve_cannot_revive_session() {
        let store = MemoryTokenSecretStore::new();
        persist_refreshed_token_when(&store, "access...refresh", true);
        assert_eq!(
            resolve_token_in(&store, None, STEPFUN_CREDENTIAL_TARGET, &[]).unwrap(),
            "access...refresh"
        );

        clear_token_secret(&store).expect("revoke must delete the leftover token");

        let error = resolve_token_in(&store, None, STEPFUN_CREDENTIAL_TARGET, &[])
            .expect_err("leftover keyring token must not authenticate after revoke");
        assert!(
            matches!(error, ProviderError::NotInstalled(_)),
            "expected NotInstalled after revoke, got {error:?}"
        );
    }

    /// SBS-920: a refresh that lands after revoke must not put the session
    /// back. Revoke deletes the keyring copy, confirms it, and reports
    /// success; the write that follows has to see a revoked credential and
    /// leave the store empty.
    #[test]
    fn a_refresh_landing_after_revoke_does_not_restore_the_token() {
        let store = MemoryTokenSecretStore::new();
        persist_refreshed_token_when(&store, "access...refresh", true);
        clear_token_secret(&store).expect("revoke must delete the leftover token");

        let persisted = persist_refreshed_token_when(&store, "refreshed-after-revoke", false);

        assert!(!persisted, "a revoked credential must not be written back");
        let error = resolve_token_in(&store, None, STEPFUN_CREDENTIAL_TARGET, &[])
            .expect_err("a refresh after revoke must not authenticate");
        assert!(
            matches!(error, ProviderError::NotInstalled(_)),
            "expected NotInstalled after revoke, got {error:?}"
        );
    }

    /// The locked path itself, not just the decision it reaches.
    ///
    /// Without this, dropping the lock or hard-coding the check to true would
    /// leave every other test here green while the race this closes came back.
    #[test]
    fn the_locked_persist_path_writes_nothing_for_a_revoked_credential() {
        let store = MemoryTokenSecretStore::new();

        persist_refreshed_token_checked(&store, "refreshed-after-revoke", || false);

        let error = resolve_token_in(&store, None, STEPFUN_CREDENTIAL_TARGET, &[])
            .expect_err("a revoked credential must leave the keyring empty");
        assert!(matches!(error, ProviderError::NotInstalled(_)));
    }

    #[test]
    fn the_locked_persist_path_writes_for_a_live_credential() {
        let store = MemoryTokenSecretStore::new();

        persist_refreshed_token_checked(&store, "fresh-token", || true);

        assert_eq!(
            resolve_token_in(&store, None, STEPFUN_CREDENTIAL_TARGET, &[]).unwrap(),
            "fresh-token"
        );
    }

    #[test]
    fn a_refresh_for_a_live_credential_is_still_persisted() {
        let store = MemoryTokenSecretStore::new();

        assert!(persist_refreshed_token_when(&store, "fresh-token", true));

        assert_eq!(
            resolve_token_in(&store, None, STEPFUN_CREDENTIAL_TARGET, &[]).unwrap(),
            "fresh-token"
        );
    }
    #[test]
    fn clear_persisted_credentials_is_idempotent_when_keyring_has_no_entry() {
        let store = MemoryTokenSecretStore::new();
        clear_token_secret(&store).expect("missing keyring entry is already revoked");
        let error = resolve_token_in(&store, None, STEPFUN_CREDENTIAL_TARGET, &[])
            .expect_err("empty store must not resolve a token");
        assert!(matches!(error, ProviderError::NotInstalled(_)));
    }

    #[test]
    fn clear_persisted_credentials_fails_closed_when_keyring_delete_errors() {
        let error =
            clear_token_secret(&FailingDeleteStore).expect_err("a locked keyring must fail revoke");
        assert!(
            error
                .to_string()
                .contains("Could not delete StepFun keyring token"),
            "got {error}"
        );
        assert_eq!(
            resolve_token_in(&FailingDeleteStore, None, STEPFUN_CREDENTIAL_TARGET, &[]).unwrap(),
            "leftover-oasis-token"
        );
    }

    #[test]
    fn clear_persisted_credentials_fails_closed_when_keychain_is_disabled() {
        let _guard = crate::keychain::with_mock_store(
            false,
            &[(STEPFUN_CREDENTIAL_TARGET, "api_key", "leftover")],
        );
        let error = clear_token_secret(&OsTokenSecretStore)
            .expect_err("a disabled keychain must fail revoke");
        let message = error.to_string();
        assert!(
            message.contains("Could not delete StepFun keyring token")
                && message.contains("disabled"),
            "got {error}"
        );
        assert!(
            OsTokenSecretStore
                .get(STEPFUN_CREDENTIAL_TARGET, "api_key")
                .is_err(),
            "Disabled must not look like a missing token"
        );
    }

    #[test]
    fn clear_persisted_credentials_fails_closed_when_delete_lies() {
        let store = LyingDeleteStore::new();
        persist_refreshed_token_when(&store, "still-here", true);
        let error = clear_token_secret(&store)
            .expect_err("reporting success while the token remains is fail-open");
        assert!(
            error.to_string().contains("still present after delete"),
            "got {error}"
        );
    }

    #[test]
    fn resolve_token_prefers_explicit_preferences_key_over_keyring() {
        let store = MemoryTokenSecretStore::new();
        persist_refreshed_token_when(&store, "keyring-copy", true);
        assert_eq!(
            resolve_token_in(
                &store,
                Some(" preferences-copy "),
                STEPFUN_CREDENTIAL_TARGET,
                &[]
            )
            .unwrap(),
            "preferences-copy"
        );
    }
}
