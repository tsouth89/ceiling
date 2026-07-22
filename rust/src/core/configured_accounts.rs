//! The directory-backed accounts a user has configured, across every provider
//! that supports them.
//!
//! Loaded once per refresh cycle and shared by the desktop app and the CLI, so
//! both resolve "which account am I fetching" identically.

use std::path::PathBuf;

use super::{
    ClaudeAccountData, ClaudeAccountStore, CodexAccountData, CodexAccountStore, ProviderId,
};

/// Accounts configured for each directory-backed provider.
#[derive(Debug, Default, Clone)]
pub struct ConfiguredAccounts {
    pub codex: CodexAccountData,
    pub claude: ClaudeAccountData,
}

impl ConfiguredAccounts {
    /// Load every provider's accounts from disk. A store that fails to load is
    /// treated as unconfigured, which falls back to following the CLI rather
    /// than failing the refresh.
    pub fn load() -> Self {
        Self {
            codex: CodexAccountStore::new().load().unwrap_or_else(|error| {
                tracing::warn!("failed to load Codex accounts: {error}");
                CodexAccountData::default()
            }),
            claude: ClaudeAccountStore::new().load().unwrap_or_else(|error| {
                tracing::warn!("failed to load Claude accounts: {error}");
                ClaudeAccountData::default()
            }),
        }
    }

    /// The config directory to fetch `provider` from, or `None` to follow
    /// whichever account its CLI is signed in as.
    ///
    /// Deliberately `None` rather than the ambient directory when nothing is
    /// configured: the provider then resolves the directory itself at fetch
    /// time, exactly as it always has, instead of being pinned to a path
    /// snapshotted when the refresh cycle started.
    pub fn active_dir_for(&self, provider: ProviderId) -> Option<PathBuf> {
        match provider {
            ProviderId::Codex => self.codex.is_explicit().then(|| self.codex.active_dir()),
            ProviderId::Claude => self.claude.is_explicit().then(|| self.claude.active_dir()),
            _ => None,
        }
    }

    /// Label of the active account for `provider`, when one is configured.
    pub fn active_label_for(&self, provider: ProviderId) -> Option<&str> {
        match provider {
            ProviderId::Codex => self.codex.active_account().map(|a| a.label.as_str()),
            ProviderId::Claude => self.claude.active_account().map(|a| a.label.as_str()),
            _ => None,
        }
    }

    /// Whether `provider` stores its accounts as config directories.
    pub fn supports(provider: ProviderId) -> bool {
        matches!(provider, ProviderId::Codex | ProviderId::Claude)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::{ClaudeIdentity, CodexIdentity, DirectoryAccount};

    #[test]
    fn nothing_configured_follows_the_cli_for_every_provider() {
        let accounts = ConfiguredAccounts::default();

        assert_eq!(accounts.active_dir_for(ProviderId::Codex), None);
        assert_eq!(accounts.active_dir_for(ProviderId::Claude), None);
        assert_eq!(accounts.active_label_for(ProviderId::Codex), None);
    }

    #[test]
    fn a_configured_account_pins_only_its_own_provider() {
        let mut accounts = ConfiguredAccounts::default();
        accounts
            .codex
            .add_account(DirectoryAccount::<CodexIdentity>::new(
                Some("work".to_string()),
                PathBuf::from("/homes/work"),
            ));

        assert_eq!(
            accounts.active_dir_for(ProviderId::Codex),
            Some(PathBuf::from("/homes/work"))
        );
        assert_eq!(accounts.active_label_for(ProviderId::Codex), Some("work"));
        // Configuring Codex must not pin Claude to anything.
        assert_eq!(accounts.active_dir_for(ProviderId::Claude), None);
    }

    #[test]
    fn switching_changes_which_directory_is_fetched() {
        let mut accounts = ConfiguredAccounts::default();
        let personal = accounts
            .claude
            .add_account(DirectoryAccount::<ClaudeIdentity>::new(
                Some("personal".to_string()),
                PathBuf::from("/dirs/personal"),
            ));
        accounts
            .claude
            .add_account(DirectoryAccount::<ClaudeIdentity>::new(
                Some("work".to_string()),
                PathBuf::from("/dirs/work"),
            ));

        assert_eq!(
            accounts.active_dir_for(ProviderId::Claude),
            Some(PathBuf::from("/dirs/work"))
        );

        accounts.claude.set_active_by_id(personal);

        assert_eq!(
            accounts.active_dir_for(ProviderId::Claude),
            Some(PathBuf::from("/dirs/personal"))
        );
        assert_eq!(
            accounts.active_label_for(ProviderId::Claude),
            Some("personal")
        );
    }

    #[test]
    fn only_directory_backed_providers_are_supported() {
        assert!(ConfiguredAccounts::supports(ProviderId::Codex));
        assert!(ConfiguredAccounts::supports(ProviderId::Claude));
        assert!(!ConfiguredAccounts::supports(ProviderId::Cursor));
        assert!(!ConfiguredAccounts::supports(ProviderId::Gemini));
    }
}
