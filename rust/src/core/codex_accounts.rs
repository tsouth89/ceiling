//! Codex accounts, keyed by `CODEX_HOME`.
//!
//! See [`crate::core::account_dirs`] for why an account is a directory rather
//! than a stored token. Nothing here reads `access_token` or `refresh_token`.

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

use super::account_dirs::{
    AccountIdentity, DirectoryAccount, DirectoryAccountData, DirectoryAccountStore,
    decode_jwt_claims, json_string,
};

/// Directory name Codex uses under the user's home when `CODEX_HOME` is unset.
const DEFAULT_CODEX_DIR: &str = ".codex";

/// Claims are namespaced under an absolute URI key in the Codex `id_token`.
const OPENAI_AUTH_CLAIM: &str = "https://api.openai.com/auth";

pub type CodexAccount = DirectoryAccount<CodexIdentity>;
pub type CodexAccountData = DirectoryAccountData<CodexIdentity>;
pub type CodexAccountStore = DirectoryAccountStore<CodexIdentity>;

/// Resolve the Codex home the CLI itself would use: `CODEX_HOME` when set to a
/// non-empty value, otherwise `~/.codex`.
pub fn ambient_codex_home() -> PathBuf {
    if let Ok(home) = std::env::var("CODEX_HOME") {
        let trimmed = home.trim();
        if !trimmed.is_empty() {
            return PathBuf::from(trimmed);
        }
    }
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(DEFAULT_CODEX_DIR)
}

/// Identity read from a Codex home's `auth.json`.
///
/// These come from the `id_token` JWT payload, which is decoded but **not**
/// signature-verified: it is a local file the user already controls and the
/// values are used only for display and for keying usage by account.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CodexIdentity {
    /// Account email, when the token carries one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub email: Option<String>,
    /// Stable ChatGPT account id, also sent as the `ChatGPT-Account-Id` header.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub account_id: Option<String>,
    /// Plan bucket (`plus`, `pro`, `prolite`, `team`, ...) as OpenAI reports it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub plan_type: Option<String>,
}

impl CodexIdentity {
    /// Whether any field was resolved. An all-empty identity is not worth storing.
    pub fn is_empty(&self) -> bool {
        self.email.is_none() && self.account_id.is_none() && self.plan_type.is_none()
    }
}

impl AccountIdentity for CodexIdentity {
    fn ambient_dir() -> PathBuf {
        ambient_codex_home()
    }

    fn store_file_name() -> &'static str {
        "codex-accounts.json"
    }

    fn read(config_dir: &Path) -> Option<Self> {
        let content = crate::secure_file::read_string(&config_dir.join("auth.json")).ok()?;
        identity_from_auth_json(&content)
    }

    /// e.g. `person@example.com (pro)`.
    fn suggested_label(&self) -> Option<String> {
        let base = self
            .email
            .clone()
            .or_else(|| self.account_id.clone())
            .or_else(|| self.plan_type.clone())?;
        match (&self.email, &self.plan_type) {
            (Some(_), Some(plan)) => Some(format!("{base} ({plan})")),
            _ => Some(base),
        }
    }
}

/// Parse identity out of the contents of a Codex `auth.json`.
///
/// Returns `None` when the file holds an API key rather than an OAuth session,
/// or when no claims could be resolved.
pub fn identity_from_auth_json(content: &str) -> Option<CodexIdentity> {
    let json: serde_json::Value = serde_json::from_str(content).ok()?;
    let tokens = json.get("tokens")?;

    let mut identity = CodexIdentity {
        email: None,
        // `account_id` sits alongside the tokens; richer claims are in `id_token`.
        account_id: json_string(tokens, "account_id"),
        plan_type: None,
    };

    let claims = tokens
        .get("id_token")
        .and_then(|value| value.as_str())
        .and_then(decode_jwt_claims);

    if let Some(claims) = claims {
        identity.email = json_string(&claims, "email");

        if let Some(auth) = claims.get(OPENAI_AUTH_CLAIM) {
            identity.plan_type = json_string(auth, "chatgpt_plan_type");
            if identity.account_id.is_none() {
                identity.account_id = json_string(auth, "chatgpt_account_id");
            }
        }
    }

    if identity.is_empty() {
        None
    } else {
        Some(identity)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identity_is_read_from_id_token_claims() {
        // Payload: {"email":"person@example.com",
        //   "https://api.openai.com/auth":{"chatgpt_plan_type":"pro",
        //   "chatgpt_account_id":"acct-from-claims"}}
        let payload = "eyJlbWFpbCI6InBlcnNvbkBleGFtcGxlLmNvbSIsImh0dHBzOi8vYXBpLm9wZW5haS5jb20vYXV0aCI6eyJjaGF0Z3B0X3BsYW5fdHlwZSI6InBybyIsImNoYXRncHRfYWNjb3VudF9pZCI6ImFjY3QtZnJvbS1jbGFpbXMifX0";
        let auth = format!(
            r#"{{"tokens":{{"id_token":"header.{payload}.signature","access_token":"secret","account_id":"acct-123"}}}}"#
        );

        let identity = identity_from_auth_json(&auth).expect("identity");

        assert_eq!(identity.email.as_deref(), Some("person@example.com"));
        assert_eq!(identity.plan_type.as_deref(), Some("pro"));
        // The top-level account_id wins; it is what the API header uses.
        assert_eq!(identity.account_id.as_deref(), Some("acct-123"));
        assert_eq!(
            identity.suggested_label().as_deref(),
            Some("person@example.com (pro)")
        );
    }

    #[test]
    fn identity_falls_back_to_account_id_without_an_id_token() {
        let auth = r#"{"tokens":{"access_token":"secret","account_id":"acct-123"}}"#;

        let identity = identity_from_auth_json(auth).expect("identity");

        assert_eq!(identity.email, None);
        assert_eq!(identity.account_id.as_deref(), Some("acct-123"));
        assert_eq!(identity.suggested_label().as_deref(), Some("acct-123"));
    }

    #[test]
    fn an_api_key_auth_file_yields_no_identity() {
        let auth = r#"{"OPENAI_API_KEY":"sk-test","tokens":null}"#;

        assert!(identity_from_auth_json(auth).is_none());
    }

    #[test]
    fn accounts_are_stored_under_their_own_file() {
        assert!(
            CodexAccountStore::default_path().ends_with("codex-accounts.json"),
            "codex accounts must not share a file with another provider"
        );
    }
}
