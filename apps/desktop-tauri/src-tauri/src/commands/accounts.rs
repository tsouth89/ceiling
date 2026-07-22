//! Directory-backed account commands and DTOs.
//!
//! An account here is a provider config directory (`CODEX_HOME`,
//! `CLAUDE_CONFIG_DIR`), never a credential Ceiling stores. Only the directory
//! path and display identity cross the bridge; no token material does.

use codexbar::core::{
    AccountIdentity, ClaudeAccountStore, ClaudeIdentity, CodexAccountStore, CodexIdentity,
    DirectoryAccount, DirectoryAccountData, DirectoryAccountStore, ProviderId,
};
use serde::Serialize;
use std::path::{Path, PathBuf};

/// Bridge-friendly account. `configDir` is a path the user chose themselves, so
/// it is shown as-is; nothing else here is derived from a credential value.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DirectoryAccountBridge {
    pub id: String,
    pub label: String,
    pub config_dir: String,
    pub tint: Option<String>,
    pub is_active: bool,
    /// Whether the directory currently holds a usable sign-in.
    pub signed_in: bool,
    pub email: Option<String>,
    pub organization: Option<String>,
    pub plan: Option<String>,
    pub added_at: String,
    pub last_used: Option<String>,
}

/// Bridge-friendly snapshot of one provider's accounts.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderAccountsBridge {
    pub provider_id: String,
    pub display_name: String,
    /// Environment variable that selects this provider's config directory, shown
    /// in the UI because adding an account means running the CLI with it set.
    pub env_var: String,
    pub accounts: Vec<DirectoryAccountBridge>,
    pub active_index: usize,
    /// True when no accounts are configured and Ceiling follows whichever
    /// account the CLI is signed in as, which is the default.
    pub following_cli: bool,
    /// The directory being followed in that case.
    pub ambient_dir: String,
}

/// Result of inspecting a directory before adding it as an account.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AccountProbeBridge {
    pub config_dir: String,
    pub exists: bool,
    pub signed_in: bool,
    pub suggested_label: Option<String>,
    pub email: Option<String>,
    pub organization: Option<String>,
    pub plan: Option<String>,
    /// Set when the directory is already configured as an account.
    pub already_added_as: Option<String>,
}

/// Display fields pulled out of a provider's identity type.
struct Display {
    email: Option<String>,
    organization: Option<String>,
    plan: Option<String>,
}

trait Displayable: AccountIdentity {
    fn display(&self) -> Display;
    fn env_var() -> &'static str;
    fn provider() -> ProviderId;
}

impl Displayable for CodexIdentity {
    fn display(&self) -> Display {
        Display {
            email: self.email.clone(),
            // Codex reports no organization name, only the plan bucket.
            organization: None,
            plan: self.plan_type.clone(),
        }
    }
    fn env_var() -> &'static str {
        "CODEX_HOME"
    }
    fn provider() -> ProviderId {
        ProviderId::Codex
    }
}

impl Displayable for ClaudeIdentity {
    fn display(&self) -> Display {
        Display {
            email: self.email.clone(),
            organization: self.organization_name.clone(),
            plan: self.subscription_type.clone(),
        }
    }
    fn env_var() -> &'static str {
        "CLAUDE_CONFIG_DIR"
    }
    fn provider() -> ProviderId {
        ProviderId::Claude
    }
}

fn format_date(ts: i64) -> String {
    chrono::DateTime::from_timestamp(ts, 0)
        .map(|dt: chrono::DateTime<chrono::Utc>| dt.format("%b %d, %Y").to_string())
        .unwrap_or_else(|| "Unknown".to_string())
}

fn bridge_account<I: Displayable>(
    account: &DirectoryAccount<I>,
    is_active: bool,
) -> DirectoryAccountBridge {
    // Prefer what the directory says right now over what was cached when the
    // account was added, so a re-login inside a directory is reflected.
    let identity = I::read(&account.config_dir).or_else(|| account.identity.clone());
    let display = identity.as_ref().map(Displayable::display);

    DirectoryAccountBridge {
        id: account.id.to_string(),
        label: account.label.clone(),
        config_dir: account.config_dir.display().to_string(),
        tint: account.tint.clone(),
        is_active,
        signed_in: I::is_signed_in(&account.config_dir),
        email: display.as_ref().and_then(|d| d.email.clone()),
        organization: display.as_ref().and_then(|d| d.organization.clone()),
        plan: display.as_ref().and_then(|d| d.plan.clone()),
        added_at: format_date(account.added_at),
        last_used: account.last_used.map(format_date),
    }
}

fn bridge_provider<I: Displayable>(data: &DirectoryAccountData<I>) -> ProviderAccountsBridge {
    let active = data.clamped_active_index();
    ProviderAccountsBridge {
        provider_id: I::provider().cli_name().to_string(),
        display_name: I::provider().display_name().to_string(),
        env_var: I::env_var().to_string(),
        accounts: data
            .accounts
            .iter()
            .enumerate()
            .map(|(index, account)| bridge_account(account, index == active))
            .collect(),
        active_index: active,
        following_cli: !data.is_explicit(),
        ambient_dir: I::ambient_dir().display().to_string(),
    }
}

/// Validate a caller-supplied directory path.
fn parse_config_dir(config_dir: &str) -> Result<PathBuf, String> {
    let trimmed = config_dir.trim();
    if trimmed.is_empty() {
        return Err("Choose a config directory.".to_string());
    }
    let path = PathBuf::from(trimmed);
    if !path.is_absolute() {
        return Err("Use a full path to the config directory.".to_string());
    }
    if !path.is_dir() {
        return Err(format!("{} is not a directory.", path.display()));
    }
    Ok(path)
}

fn parse_uuid(account_id: &str) -> Result<uuid::Uuid, String> {
    uuid::Uuid::parse_str(account_id).map_err(|e| e.to_string())
}

/// Accent colors are written into the UI, so accept only a plain hex color
/// rather than arbitrary CSS.
fn parse_tint(tint: Option<String>) -> Result<Option<String>, String> {
    let Some(tint) = tint else {
        return Ok(None);
    };
    let trimmed = tint.trim();
    if trimmed.is_empty() {
        return Ok(None);
    }
    let valid = trimmed.starts_with('#')
        && matches!(trimmed.len(), 4 | 7)
        && trimmed[1..].chars().all(|c| c.is_ascii_hexdigit());
    if !valid {
        return Err("Use a hex color like #4f8ff7.".to_string());
    }
    Ok(Some(trimmed.to_string()))
}

/// Run `op` against a provider's store, saving and returning the new snapshot.
fn with_store<I: Displayable>(
    op: impl FnOnce(&mut DirectoryAccountData<I>) -> Result<(), String>,
) -> Result<ProviderAccountsBridge, String> {
    let store: DirectoryAccountStore<I> = DirectoryAccountStore::new();
    let mut data = store.load().map_err(|e| e.to_string())?;
    op(&mut data)?;
    store.save(&data).map_err(|e| e.to_string())?;
    Ok(bridge_provider(&data))
}

/// Load every provider's directory-backed accounts.
#[tauri::command]
pub fn get_directory_accounts() -> Result<Vec<ProviderAccountsBridge>, String> {
    let codex = CodexAccountStore::new().load().map_err(|e| e.to_string())?;
    let claude = ClaudeAccountStore::new()
        .load()
        .map_err(|e| e.to_string())?;
    Ok(vec![bridge_provider(&codex), bridge_provider(&claude)])
}

/// Inspect a directory before adding it, so the user sees whose account it is.
#[tauri::command]
pub fn probe_account_directory(
    provider_id: String,
    config_dir: String,
) -> Result<AccountProbeBridge, String> {
    let id = super::parse_provider_arg(&provider_id)?;
    let trimmed = config_dir.trim().to_string();
    let path = PathBuf::from(&trimmed);

    fn probe<I: Displayable>(path: &Path) -> Result<AccountProbeBridge, String> {
        let store: DirectoryAccountStore<I> = DirectoryAccountStore::new();
        let already_added_as = store.load().ok().and_then(|data| {
            data.accounts
                .iter()
                .find(|account| codexbar::core::same_dir(&account.config_dir, path))
                .map(|account| account.label.clone())
        });
        let identity = I::read(path);
        let display = identity.as_ref().map(Displayable::display);

        Ok(AccountProbeBridge {
            config_dir: path.display().to_string(),
            exists: path.is_dir(),
            signed_in: I::is_signed_in(path),
            suggested_label: identity.as_ref().and_then(I::suggested_label),
            email: display.as_ref().and_then(|d| d.email.clone()),
            organization: display.as_ref().and_then(|d| d.organization.clone()),
            plan: display.as_ref().and_then(|d| d.plan.clone()),
            already_added_as,
        })
    }

    match id {
        ProviderId::Codex => probe::<CodexIdentity>(&path),
        ProviderId::Claude => probe::<ClaudeIdentity>(&path),
        other => Err(format!(
            "{} does not use config-directory accounts.",
            other.display_name()
        )),
    }
}

/// Add a config directory as an account and make it active.
#[tauri::command]
pub fn add_directory_account(
    provider_id: String,
    config_dir: String,
    label: Option<String>,
) -> Result<ProviderAccountsBridge, String> {
    let id = super::parse_provider_arg(&provider_id)?;
    let path = parse_config_dir(&config_dir)?;
    let label = super::sanitize_optional_label(label)?;

    fn add<I: Displayable>(
        path: PathBuf,
        label: Option<String>,
    ) -> Result<ProviderAccountsBridge, String> {
        with_store::<I>(|data| {
            data.add_account(DirectoryAccount::<I>::new(label, path));
            Ok(())
        })
    }

    match id {
        ProviderId::Codex => add::<CodexIdentity>(path, label),
        ProviderId::Claude => add::<ClaudeIdentity>(path, label),
        other => Err(format!(
            "{} does not use config-directory accounts.",
            other.display_name()
        )),
    }
}

/// Remove an account. Removing the last one returns to following the CLI.
#[tauri::command]
pub fn remove_directory_account(
    provider_id: String,
    account_id: String,
) -> Result<ProviderAccountsBridge, String> {
    let id = super::parse_provider_arg(&provider_id)?;
    let uuid = parse_uuid(&account_id)?;

    fn remove<I: Displayable>(uuid: uuid::Uuid) -> Result<ProviderAccountsBridge, String> {
        with_store::<I>(|data| {
            data.remove_account(uuid);
            Ok(())
        })
    }

    match id {
        ProviderId::Codex => remove::<CodexIdentity>(uuid),
        ProviderId::Claude => remove::<ClaudeIdentity>(uuid),
        other => Err(format!(
            "{} does not use config-directory accounts.",
            other.display_name()
        )),
    }
}

/// Switch which account Ceiling tracks for a provider.
#[tauri::command]
pub fn set_active_directory_account(
    provider_id: String,
    account_id: String,
) -> Result<ProviderAccountsBridge, String> {
    let id = super::parse_provider_arg(&provider_id)?;
    let uuid = parse_uuid(&account_id)?;

    fn activate<I: Displayable>(uuid: uuid::Uuid) -> Result<ProviderAccountsBridge, String> {
        with_store::<I>(|data| {
            if !data.set_active_by_id(uuid) {
                return Err("That account is no longer configured.".to_string());
            }
            Ok(())
        })
    }

    match id {
        ProviderId::Codex => activate::<CodexIdentity>(uuid),
        ProviderId::Claude => activate::<ClaudeIdentity>(uuid),
        other => Err(format!(
            "{} does not use config-directory accounts.",
            other.display_name()
        )),
    }
}

/// Relabel an account and/or set its accent color.
#[tauri::command]
pub fn update_directory_account(
    provider_id: String,
    account_id: String,
    label: Option<String>,
    tint: Option<String>,
) -> Result<ProviderAccountsBridge, String> {
    let id = super::parse_provider_arg(&provider_id)?;
    let uuid = parse_uuid(&account_id)?;
    let label = super::sanitize_optional_label(label)?;
    let tint = parse_tint(tint)?;

    fn update<I: Displayable>(
        uuid: uuid::Uuid,
        label: Option<String>,
        tint: Option<String>,
    ) -> Result<ProviderAccountsBridge, String> {
        with_store::<I>(|data| {
            let Some(account) = data.accounts.iter_mut().find(|account| account.id == uuid) else {
                return Err("That account is no longer configured.".to_string());
            };
            if let Some(label) = label {
                account.label = label;
            }
            // An explicit empty tint clears it; see `parse_tint`.
            account.tint = tint;
            Ok(())
        })
    }

    match id {
        ProviderId::Codex => update::<CodexIdentity>(uuid, label, tint),
        ProviderId::Claude => update::<ClaudeIdentity>(uuid, label, tint),
        other => Err(format!(
            "{} does not use config-directory accounts.",
            other.display_name()
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_relative_path_is_rejected() {
        assert!(parse_config_dir("relative/dir").is_err());
        assert!(parse_config_dir("   ").is_err());
    }

    #[test]
    fn a_path_that_is_not_a_directory_is_rejected() {
        let dir = tempfile::tempdir().expect("tempdir");
        let file = dir.path().join("not-a-dir.txt");
        std::fs::write(&file, "x").expect("write");

        assert!(parse_config_dir(&file.display().to_string()).is_err());
        assert!(parse_config_dir(&dir.path().display().to_string()).is_ok());
    }

    #[test]
    fn only_plain_hex_colors_are_accepted_as_tints() {
        assert_eq!(
            parse_tint(Some("#4f8ff7".into())),
            Ok(Some("#4f8ff7".into()))
        );
        assert_eq!(parse_tint(Some("#abc".into())), Ok(Some("#abc".into())));
        assert_eq!(parse_tint(Some("  ".into())), Ok(None));
        assert_eq!(parse_tint(None), Ok(None));

        // These would otherwise be interpolated into the UI as-is.
        assert!(parse_tint(Some("red".into())).is_err());
        assert!(parse_tint(Some("#4f8ff7; background: url(x)".into())).is_err());
        assert!(parse_tint(Some("var(--accent)".into())).is_err());
        assert!(parse_tint(Some("#zzzzzz".into())).is_err());
    }
}
