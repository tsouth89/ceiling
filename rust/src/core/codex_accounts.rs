//! Ceiling-managed Codex accounts.
//!
//! Unlike [`crate::core::token_accounts`], a Codex account is a *directory* (a
//! `CODEX_HOME`) rather than a stored credential. The Codex CLI owns exactly one
//! `auth.json` per home and refreshes its OAuth tokens in place, so pointing at
//! the home keeps working where a token copied into Ceiling would go stale within
//! hours. It also means Ceiling never holds an OpenAI credential of its own, and
//! each account brings its own `sessions/` directory for local activity.
//!
//! Nothing in this module reads `access_token` or `refresh_token`. It reads only
//! the identity claims needed to label an account in the UI.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::{fs, io};
use uuid::Uuid;

/// Directory name Codex uses under the user's home when `CODEX_HOME` is unset.
const DEFAULT_CODEX_DIR: &str = ".codex";

/// Resolve the Codex home the CLI itself would use: `CODEX_HOME` when set to a
/// non-empty value, otherwise `~/.codex`. This is the account Ceiling tracks when
/// the user has not configured any explicitly.
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

/// Identity claims read from a Codex home's `auth.json`.
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

    /// Best-effort human label, e.g. `person@example.com (pro)`.
    pub fn suggested_label(&self) -> Option<String> {
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

/// Read the identity claims for a Codex home without touching its tokens.
///
/// Returns `None` when the home has no readable `auth.json`, when the file holds
/// an API key rather than an OAuth session, or when no claims could be resolved.
pub fn read_identity(codex_home: &Path) -> Option<CodexIdentity> {
    let content = crate::secure_file::read_string(&codex_home.join("auth.json")).ok()?;
    identity_from_auth_json(&content)
}

/// Parse identity claims out of the contents of an `auth.json`.
pub fn identity_from_auth_json(content: &str) -> Option<CodexIdentity> {
    let json: serde_json::Value = serde_json::from_str(content).ok()?;
    let tokens = json.get("tokens")?;

    // `account_id` sits alongside the tokens; the richer claims are in `id_token`.
    let account_id = tokens
        .get("account_id")
        .and_then(|value| value.as_str())
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string);

    let claims = tokens
        .get("id_token")
        .and_then(|value| value.as_str())
        .and_then(decode_jwt_claims);

    let mut identity = CodexIdentity {
        email: None,
        account_id,
        plan_type: None,
    };

    if let Some(claims) = claims {
        identity.email = claims
            .get("email")
            .and_then(|value| value.as_str())
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_string);

        // OpenAI namespaces the ChatGPT claims under an absolute URI key.
        if let Some(auth) = claims.get("https://api.openai.com/auth") {
            identity.plan_type = auth
                .get("chatgpt_plan_type")
                .and_then(|value| value.as_str())
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(str::to_string);
            if identity.account_id.is_none() {
                identity.account_id = auth
                    .get("chatgpt_account_id")
                    .and_then(|value| value.as_str())
                    .map(str::trim)
                    .filter(|value| !value.is_empty())
                    .map(str::to_string);
            }
        }
    }

    if identity.is_empty() {
        None
    } else {
        Some(identity)
    }
}

/// Decode the payload segment of a JWT. No signature verification: see
/// [`CodexIdentity`].
fn decode_jwt_claims(token: &str) -> Option<serde_json::Value> {
    use base64::Engine;

    let payload = token.split('.').nth(1)?;
    let mut padded = payload.to_string();
    while padded.len() % 4 != 0 {
        padded.push('=');
    }
    let bytes = base64::engine::general_purpose::URL_SAFE
        .decode(&padded)
        .or_else(|_| base64::engine::general_purpose::STANDARD.decode(&padded))
        .ok()?;
    serde_json::from_slice(&bytes).ok()
}

/// A single Codex account Ceiling can track.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CodexAccount {
    /// Unique identifier
    pub id: Uuid,
    /// User-facing label, seeded from the home's identity claims when available
    pub label: String,
    /// The `CODEX_HOME` directory backing this account
    pub codex_home: PathBuf,
    /// Optional accent color so multiple accounts stay distinguishable at a glance
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tint: Option<String>,
    /// Identity last resolved from the home, cached so the UI has a label even
    /// when the credential file is temporarily unreadable
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub identity: Option<CodexIdentity>,
    /// When this account was added (Unix timestamp in seconds)
    pub added_at: i64,
    /// When this account was last fetched (Unix timestamp in seconds)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_used: Option<i64>,
}

impl CodexAccount {
    /// Create an account for a home, labeling it from the home's own claims when
    /// the caller did not supply a label.
    pub fn new(label: Option<String>, codex_home: PathBuf) -> Self {
        let identity = read_identity(&codex_home);
        let label = label
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty())
            .or_else(|| identity.as_ref().and_then(CodexIdentity::suggested_label))
            .unwrap_or_else(|| fallback_label(&codex_home));

        Self {
            id: Uuid::new_v4(),
            label,
            codex_home,
            tint: None,
            identity,
            added_at: Utc::now().timestamp(),
            last_used: None,
        }
    }

    /// Re-read the home's claims, keeping the previous identity if the file is
    /// currently unreadable (a rotating `auth.json` should not blank the label).
    pub fn refresh_identity(&mut self) {
        if let Some(identity) = read_identity(&self.codex_home) {
            self.identity = Some(identity);
        }
    }

    /// Mark this account as used
    pub fn mark_used(&mut self) {
        self.last_used = Some(Utc::now().timestamp());
    }

    /// Get display name
    pub fn display_name(&self) -> &str {
        &self.label
    }

    /// Get added_at as DateTime
    pub fn added_at_datetime(&self) -> DateTime<Utc> {
        DateTime::from_timestamp(self.added_at, 0).unwrap_or_else(Utc::now)
    }

    /// Get last_used as DateTime
    pub fn last_used_datetime(&self) -> Option<DateTime<Utc>> {
        self.last_used
            .and_then(|ts| DateTime::from_timestamp(ts, 0))
    }
}

/// Label used when a home has no readable identity: its directory name, which is
/// what the user typed when they created it.
fn fallback_label(codex_home: &Path) -> String {
    codex_home
        .file_name()
        .and_then(|name| name.to_str())
        .filter(|name| !name.is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| codex_home.display().to_string())
}

/// The set of Codex accounts and which one is active.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct CodexAccountData {
    /// File format version
    #[serde(default = "default_version")]
    pub version: u32,
    /// Configured accounts, in display order
    #[serde(default)]
    pub accounts: Vec<CodexAccount>,
    /// Index of the active account
    #[serde(default)]
    pub active_index: usize,
}

fn default_version() -> u32 {
    1
}

impl CodexAccountData {
    /// Create empty account data
    pub fn new() -> Self {
        Self {
            version: default_version(),
            accounts: Vec::new(),
            active_index: 0,
        }
    }

    /// Active index clamped into range, so a stale index can never panic
    pub fn clamped_active_index(&self) -> usize {
        if self.accounts.is_empty() {
            0
        } else {
            self.active_index.min(self.accounts.len() - 1)
        }
    }

    /// The active account, if any are configured
    pub fn active_account(&self) -> Option<&CodexAccount> {
        self.accounts.get(self.clamped_active_index())
    }

    /// Mutable access to the active account
    pub fn active_account_mut(&mut self) -> Option<&mut CodexAccount> {
        let index = self.clamped_active_index();
        self.accounts.get_mut(index)
    }

    /// The home Ceiling should read for Codex.
    ///
    /// Falls back to [`ambient_codex_home`] when no accounts are configured, so
    /// users who never open the Accounts UI keep today's behavior exactly.
    pub fn active_home(&self) -> PathBuf {
        self.active_account()
            .map(|account| account.codex_home.clone())
            .unwrap_or_else(ambient_codex_home)
    }

    /// Add an account, keeping it active so the user sees the one they just added.
    ///
    /// Homes are unique: re-adding a configured home updates that entry rather
    /// than creating a duplicate that would double-count its sessions.
    pub fn add_account(&mut self, account: CodexAccount) -> Uuid {
        if let Some(existing) = self
            .accounts
            .iter_mut()
            .find(|candidate| same_home(&candidate.codex_home, &account.codex_home))
        {
            existing.label = account.label;
            existing.identity = account.identity;
            if account.tint.is_some() {
                existing.tint = account.tint;
            }
            let id = existing.id;
            self.set_active_by_id(id);
            return id;
        }

        let id = account.id;
        self.accounts.push(account);
        self.active_index = self.accounts.len() - 1;
        id
    }

    /// Remove an account, keeping the previously active account active when it
    /// survives the removal.
    pub fn remove_account(&mut self, id: Uuid) -> Option<CodexAccount> {
        let index = self.accounts.iter().position(|account| account.id == id)?;
        let active_id = self.active_account().map(|account| account.id);
        let removed = self.accounts.remove(index);

        self.active_index = match active_id {
            Some(active_id) if active_id != removed.id => self
                .accounts
                .iter()
                .position(|account| account.id == active_id)
                .unwrap_or(0),
            // The active account itself went away: fall back to its neighbor.
            _ => index.min(self.accounts.len().saturating_sub(1)),
        };

        Some(removed)
    }

    /// Set the active account by index
    pub fn set_active(&mut self, index: usize) {
        if index < self.accounts.len() {
            self.active_index = index;
        }
    }

    /// Set the active account by id, returning whether it was found
    pub fn set_active_by_id(&mut self, id: Uuid) -> bool {
        match self.accounts.iter().position(|account| account.id == id) {
            Some(index) => {
                self.active_index = index;
                true
            }
            None => false,
        }
    }

    /// Whether more than one account is configured
    pub fn has_multiple(&self) -> bool {
        self.accounts.len() > 1
    }

    /// Number of configured accounts
    pub fn count(&self) -> usize {
        self.accounts.len()
    }

    /// Every distinct home Ceiling should scan for local activity. Includes the
    /// ambient home when no accounts are configured.
    pub fn all_homes(&self) -> Vec<PathBuf> {
        if self.accounts.is_empty() {
            return vec![ambient_codex_home()];
        }
        let mut seen = HashSet::new();
        self.accounts
            .iter()
            .map(|account| account.codex_home.clone())
            .filter(|home| seen.insert(home_key(home)))
            .collect()
    }
}

/// Compare two homes for identity. Paths are normalized only by trimming
/// trailing separators and, on Windows, by case: the filesystem there is
/// case-insensitive, so `C:\Codex` and `c:\codex` are one account.
fn same_home(a: &Path, b: &Path) -> bool {
    home_key(a) == home_key(b)
}

fn home_key(home: &Path) -> String {
    let raw = home.to_string_lossy();
    let trimmed = raw.trim_end_matches(['/', '\\']);
    if cfg!(windows) {
        trimmed.to_lowercase()
    } else {
        trimmed.to_string()
    }
}

/// Persists [`CodexAccountData`] to disk.
pub struct CodexAccountStore {
    file_path: PathBuf,
}

/// Errors that can occur with Codex account storage
#[derive(Debug, thiserror::Error)]
pub enum CodexAccountError {
    #[error("IO error: {0}")]
    Io(#[from] io::Error),
    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),
}

impl CodexAccountStore {
    /// Create a new store with the default path
    pub fn new() -> Self {
        Self {
            file_path: Self::default_path(),
        }
    }

    /// Create a store with a custom path
    pub fn with_path(path: PathBuf) -> Self {
        Self { file_path: path }
    }

    /// Get the default storage path, alongside `token-accounts.json`
    pub fn default_path() -> PathBuf {
        dirs::config_dir()
            .map(|dir| dir.join("CodexBar"))
            .unwrap_or_else(|| {
                dirs::home_dir()
                    .unwrap_or_else(|| PathBuf::from("."))
                    .join(".codexbar")
            })
            .join("codex-accounts.json")
    }

    /// Load accounts from disk, returning empty data when nothing is configured
    pub fn load(&self) -> Result<CodexAccountData, CodexAccountError> {
        if !self.file_path.exists() {
            return Ok(CodexAccountData::new());
        }
        let data = crate::secure_file::read_string(&self.file_path)?;
        Ok(serde_json::from_str(&data)?)
    }

    /// Save accounts to disk
    pub fn save(&self, data: &CodexAccountData) -> Result<(), CodexAccountError> {
        if let Some(parent) = self.file_path.parent() {
            fs::create_dir_all(parent)?;
        }
        let json = serde_json::to_string_pretty(data)?;
        crate::secure_file::write_string(&self.file_path, &json)?;
        Ok(())
    }
}

impl Default for CodexAccountStore {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn account(label: &str, home: &str) -> CodexAccount {
        CodexAccount {
            id: Uuid::new_v4(),
            label: label.to_string(),
            codex_home: PathBuf::from(home),
            tint: None,
            identity: None,
            added_at: 0,
            last_used: None,
        }
    }

    #[test]
    fn no_accounts_resolves_to_the_ambient_home() {
        let data = CodexAccountData::new();
        assert!(data.active_account().is_none());
        assert_eq!(data.active_home(), ambient_codex_home());
        assert_eq!(data.all_homes(), vec![ambient_codex_home()]);
    }

    #[test]
    fn adding_an_account_makes_it_active() {
        let mut data = CodexAccountData::new();
        data.add_account(account("personal", "/homes/personal"));
        let id = data.add_account(account("work", "/homes/work"));

        assert_eq!(data.count(), 2);
        assert!(data.has_multiple());
        assert_eq!(data.active_account().map(|a| a.id), Some(id));
        assert_eq!(data.active_home(), PathBuf::from("/homes/work"));
    }

    #[test]
    fn re_adding_a_home_updates_in_place_instead_of_duplicating() {
        let mut data = CodexAccountData::new();
        let first = data.add_account(account("personal", "/homes/personal"));
        data.add_account(account("work", "/homes/work"));

        let again = data.add_account(account("renamed", "/homes/personal"));

        assert_eq!(data.count(), 2);
        assert_eq!(again, first, "the existing entry should be reused");
        assert_eq!(
            data.active_account().map(|a| a.label.as_str()),
            Some("renamed")
        );
    }

    #[test]
    fn removing_an_inactive_account_keeps_the_active_one() {
        let mut data = CodexAccountData::new();
        let first = data.add_account(account("personal", "/homes/personal"));
        let second = data.add_account(account("work", "/homes/work"));
        data.add_account(account("school", "/homes/school"));
        data.set_active_by_id(second);

        data.remove_account(first);

        assert_eq!(data.active_account().map(|a| a.id), Some(second));
    }

    #[test]
    fn removing_the_active_account_falls_back_to_a_neighbor() {
        let mut data = CodexAccountData::new();
        let first = data.add_account(account("personal", "/homes/personal"));
        let second = data.add_account(account("work", "/homes/work"));
        data.set_active_by_id(second);

        data.remove_account(second);

        assert_eq!(data.active_account().map(|a| a.id), Some(first));
        assert_eq!(data.active_home(), PathBuf::from("/homes/personal"));
    }

    #[test]
    fn removing_the_last_account_falls_back_to_the_ambient_home() {
        let mut data = CodexAccountData::new();
        let only = data.add_account(account("personal", "/homes/personal"));

        data.remove_account(only);

        assert_eq!(data.count(), 0);
        assert_eq!(data.active_home(), ambient_codex_home());
    }

    #[test]
    fn a_stale_active_index_is_clamped_rather_than_panicking() {
        let mut data = CodexAccountData::new();
        data.add_account(account("personal", "/homes/personal"));
        data.active_index = 17;

        assert_eq!(data.clamped_active_index(), 0);
        assert_eq!(data.active_home(), PathBuf::from("/homes/personal"));
    }

    #[test]
    fn all_homes_deduplicates() {
        let mut data = CodexAccountData::new();
        data.accounts.push(account("a", "/homes/shared"));
        data.accounts.push(account("b", "/homes/shared/"));
        data.accounts.push(account("c", "/homes/other"));

        assert_eq!(
            data.all_homes(),
            vec![
                PathBuf::from("/homes/shared"),
                PathBuf::from("/homes/other")
            ]
        );
    }

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
    fn round_trips_through_the_store() {
        let dir = tempfile::tempdir().expect("tempdir");
        let store = CodexAccountStore::with_path(dir.path().join("codex-accounts.json"));

        assert_eq!(store.load().expect("empty load").count(), 0);

        let mut data = CodexAccountData::new();
        data.add_account(account("personal", "/homes/personal"));
        let work = data.add_account(account("work", "/homes/work"));
        store.save(&data).expect("save");

        let loaded = store.load().expect("load");
        assert_eq!(loaded.count(), 2);
        assert_eq!(loaded.active_account().map(|a| a.id), Some(work));
    }
}
