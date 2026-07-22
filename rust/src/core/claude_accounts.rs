//! Claude accounts, keyed by `CLAUDE_CONFIG_DIR`.
//!
//! See [`crate::core::account_dirs`] for why an account is a directory rather
//! than a stored token. Nothing here reads `accessToken` or `refreshToken`.
//!
//! Ceiling's log scanners already honored `CLAUDE_CONFIG_DIR`; the OAuth
//! credential path did not, so a second Claude profile's activity was readable
//! while its credentials were not. Both sides resolve through
//! [`ambient_claude_config_dir`] now.

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

use super::account_dirs::{
    AccountIdentity, DirectoryAccount, DirectoryAccountData, DirectoryAccountStore, json_string,
};

/// Directory name Claude Code uses under the user's home when
/// `CLAUDE_CONFIG_DIR` is unset.
const DEFAULT_CLAUDE_DIR: &str = ".claude";

/// Where the OAuth credential lives inside a config directory.
pub const CLAUDE_CREDENTIALS_FILE: &str = ".credentials.json";

/// Where the account profile lives. Note this sits *beside* the default config
/// directory (`~/.claude.json` next to `~/.claude/`) but *inside* an explicit
/// `CLAUDE_CONFIG_DIR`, so both placements are checked.
const CLAUDE_PROFILE_FILE: &str = ".claude.json";

pub type ClaudeAccount = DirectoryAccount<ClaudeIdentity>;
pub type ClaudeAccountData = DirectoryAccountData<ClaudeIdentity>;
pub type ClaudeAccountStore = DirectoryAccountStore<ClaudeIdentity>;

/// Resolve the Claude config directory the CLI itself would use:
/// `CLAUDE_CONFIG_DIR` when set to a non-empty value, otherwise `~/.claude`.
pub fn ambient_claude_config_dir() -> PathBuf {
    if let Ok(dir) = std::env::var("CLAUDE_CONFIG_DIR") {
        let trimmed = dir.trim();
        if !trimmed.is_empty() {
            return PathBuf::from(trimmed);
        }
    }
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(DEFAULT_CLAUDE_DIR)
}

/// The OAuth credential file for a config directory.
pub fn claude_credentials_path(config_dir: &Path) -> PathBuf {
    config_dir.join(CLAUDE_CREDENTIALS_FILE)
}

/// The profile file for a config directory, checking inside it first and then
/// beside it, which is where the default `~/.claude` layout puts it.
pub fn claude_profile_path(config_dir: &Path) -> Option<PathBuf> {
    let inside = config_dir.join(CLAUDE_PROFILE_FILE);
    if inside.exists() {
        return Some(inside);
    }
    let beside = config_dir.parent()?.join(CLAUDE_PROFILE_FILE);
    if beside.exists() { Some(beside) } else { None }
}

/// Identity read from a Claude config directory.
///
/// Assembled from the profile's `oauthAccount` block and the subscription tier
/// recorded alongside the OAuth credential. Display values only.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClaudeIdentity {
    /// Account email address.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub email: Option<String>,
    /// Stable account uuid.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub account_uuid: Option<String>,
    /// Organization display name, which is what separates a work seat from a
    /// personal one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub organization_name: Option<String>,
    /// Stable organization uuid.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub organization_uuid: Option<String>,
    /// Subscription bucket (`max`, `pro`, ...).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subscription_type: Option<String>,
}

impl ClaudeIdentity {
    /// Whether any field was resolved. An all-empty identity is not worth storing.
    pub fn is_empty(&self) -> bool {
        self.email.is_none()
            && self.account_uuid.is_none()
            && self.organization_name.is_none()
            && self.organization_uuid.is_none()
            && self.subscription_type.is_none()
    }
}

impl AccountIdentity for ClaudeIdentity {
    fn ambient_dir() -> PathBuf {
        ambient_claude_config_dir()
    }

    fn store_file_name() -> &'static str {
        "claude-accounts.json"
    }

    fn read(config_dir: &Path) -> Option<Self> {
        let profile = claude_profile_path(config_dir)
            .and_then(|path| crate::secure_file::read_string(&path).ok());
        let credentials =
            crate::secure_file::read_string(&claude_credentials_path(config_dir)).ok();

        identity_from_files(profile.as_deref(), credentials.as_deref())
    }

    fn is_signed_in(config_dir: &Path) -> bool {
        claude_credentials_path(config_dir).exists()
    }

    /// e.g. `person@example.com (Acme Inc)`, falling back to the subscription
    /// bucket when there is no organization worth naming.
    fn suggested_label(&self) -> Option<String> {
        let base = self
            .email
            .clone()
            .or_else(|| self.organization_name.clone())
            .or_else(|| self.account_uuid.clone())?;
        let qualifier = self
            .organization_name
            .clone()
            .filter(|org| !restates_email(org, self.email.as_deref()))
            .or_else(|| self.subscription_type.clone());
        match qualifier {
            Some(qualifier) => Some(format!("{base} ({qualifier})")),
            None => Some(base),
        }
    }
}

/// Whether an organization name only restates the account's email and so adds
/// nothing to a label. A personal Claude account gets an organization named
/// `"<email>'s Organization"`, which would otherwise render as
/// `person@example.com (person@example.com's Organization)`.
fn restates_email(organization: &str, email: Option<&str>) -> bool {
    let Some(email) = email else {
        return false;
    };
    organization.eq_ignore_ascii_case(email)
        || organization
            .to_ascii_lowercase()
            .starts_with(&email.to_ascii_lowercase())
}

/// Assemble identity from a profile file and a credentials file, either of which
/// may be absent.
pub fn identity_from_files(
    profile: Option<&str>,
    credentials: Option<&str>,
) -> Option<ClaudeIdentity> {
    let mut identity = ClaudeIdentity::default();

    if let Some(account) = profile
        .and_then(|raw| serde_json::from_str::<serde_json::Value>(raw).ok())
        .and_then(|json| json.get("oauthAccount").cloned())
    {
        identity.email = json_string(&account, "emailAddress");
        identity.account_uuid = json_string(&account, "accountUuid");
        identity.organization_name = json_string(&account, "organizationName");
        identity.organization_uuid = json_string(&account, "organizationUuid");
    }

    if let Some(oauth) = credentials
        .and_then(|raw| serde_json::from_str::<serde_json::Value>(raw).ok())
        .and_then(|json| json.get("claudeAiOauth").cloned())
    {
        identity.subscription_type = json_string(&oauth, "subscriptionType");
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

    const PROFILE: &str = r#"{
        "oauthAccount": {
            "emailAddress": "person@example.com",
            "accountUuid": "acct-uuid",
            "organizationName": "Acme Inc",
            "organizationUuid": "org-uuid"
        }
    }"#;

    const CREDENTIALS: &str = r#"{
        "claudeAiOauth": {
            "accessToken": "secret",
            "refreshToken": "secret",
            "subscriptionType": "max"
        }
    }"#;

    #[test]
    fn identity_combines_the_profile_and_credential_files() {
        let identity = identity_from_files(Some(PROFILE), Some(CREDENTIALS)).expect("identity");

        assert_eq!(identity.email.as_deref(), Some("person@example.com"));
        assert_eq!(identity.account_uuid.as_deref(), Some("acct-uuid"));
        assert_eq!(identity.organization_name.as_deref(), Some("Acme Inc"));
        assert_eq!(identity.organization_uuid.as_deref(), Some("org-uuid"));
        assert_eq!(identity.subscription_type.as_deref(), Some("max"));
        assert_eq!(
            identity.suggested_label().as_deref(),
            Some("person@example.com (Acme Inc)")
        );
    }

    #[test]
    fn the_organization_is_what_separates_two_seats_for_one_person() {
        let work = identity_from_files(Some(PROFILE), None).expect("work");
        let personal_profile = r#"{"oauthAccount":{"emailAddress":"person@example.com",
            "organizationName":"person@example.com's Organization"}}"#;
        let personal =
            identity_from_files(Some(personal_profile), Some(CREDENTIALS)).expect("personal");

        assert_ne!(work.suggested_label(), personal.suggested_label());
    }

    #[test]
    fn a_personal_organization_named_after_the_email_is_not_repeated() {
        // What a real personal account looks like: Claude auto-names the
        // organization after the email, so using it as the qualifier would
        // render "person@example.com (person@example.com's Organization)".
        let profile = r#"{"oauthAccount":{"emailAddress":"person@example.com",
            "organizationName":"person@example.com's Organization"}}"#;

        let identity = identity_from_files(Some(profile), Some(CREDENTIALS)).expect("identity");

        assert_eq!(
            identity.suggested_label().as_deref(),
            Some("person@example.com (max)")
        );
    }

    #[test]
    fn a_real_organization_is_kept_even_when_it_contains_the_word_organization() {
        let profile = r#"{"oauthAccount":{"emailAddress":"person@example.com",
            "organizationName":"Acme Organization"}}"#;

        let identity = identity_from_files(Some(profile), Some(CREDENTIALS)).expect("identity");

        assert_eq!(
            identity.suggested_label().as_deref(),
            Some("person@example.com (Acme Organization)")
        );
    }

    #[test]
    fn label_falls_back_to_the_subscription_when_there_is_no_organization() {
        let profile = r#"{"oauthAccount":{"emailAddress":"person@example.com"}}"#;

        let identity = identity_from_files(Some(profile), Some(CREDENTIALS)).expect("identity");

        assert_eq!(
            identity.suggested_label().as_deref(),
            Some("person@example.com (max)")
        );
    }

    #[test]
    fn a_credentials_file_alone_still_yields_a_usable_identity() {
        let identity = identity_from_files(None, Some(CREDENTIALS)).expect("identity");

        assert_eq!(identity.email, None);
        assert_eq!(identity.subscription_type.as_deref(), Some("max"));
    }

    #[test]
    fn an_empty_directory_yields_no_identity() {
        assert!(identity_from_files(None, None).is_none());
        assert!(identity_from_files(Some("{}"), Some("{}")).is_none());
        assert!(identity_from_files(Some("not json"), None).is_none());
    }

    #[test]
    fn the_profile_is_found_beside_the_default_config_directory() {
        let root = tempfile::tempdir().expect("tempdir");
        let config_dir = root.path().join(".claude");
        std::fs::create_dir_all(&config_dir).expect("create config dir");
        std::fs::write(root.path().join(".claude.json"), PROFILE).expect("write profile");

        let found = claude_profile_path(&config_dir).expect("profile path");

        assert_eq!(found, root.path().join(".claude.json"));
    }

    #[test]
    fn a_profile_inside_the_config_directory_wins() {
        let root = tempfile::tempdir().expect("tempdir");
        let config_dir = root.path().join("work");
        std::fs::create_dir_all(&config_dir).expect("create config dir");
        std::fs::write(root.path().join(".claude.json"), PROFILE).expect("write outer profile");
        std::fs::write(config_dir.join(".claude.json"), PROFILE).expect("write inner profile");

        let found = claude_profile_path(&config_dir).expect("profile path");

        assert_eq!(found, config_dir.join(".claude.json"));
    }

    #[test]
    fn accounts_are_stored_under_their_own_file() {
        assert!(
            ClaudeAccountStore::default_path().ends_with("claude-accounts.json"),
            "claude accounts must not share a file with another provider"
        );
    }
}
