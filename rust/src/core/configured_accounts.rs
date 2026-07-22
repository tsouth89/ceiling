//! The directory-backed accounts a user has configured, across every provider
//! that supports them.
//!
//! Loaded once per refresh cycle and shared by the desktop app and the CLI, so
//! both resolve "which account am I fetching" identically.

use std::path::PathBuf;

use super::{
    ClaudeAccountData, ClaudeAccountStore, CodexAccountData, CodexAccountStore, ProviderId,
};

/// One account Ceiling should fetch for a provider.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AccountTarget {
    /// Stable id, used to key this account's reading in the provider cache.
    pub id: String,
    pub label: String,
    pub tint: Option<String>,
    pub config_dir: PathBuf,
}

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

    /// Every account to fetch for `provider`.
    ///
    /// Empty means nothing is configured, so the caller should do a single fetch
    /// against whichever account the CLI is signed in as. That is deliberately
    /// distinct from "one configured account": the ambient case must not be
    /// pinned to a path resolved when the refresh cycle started.
    pub fn targets_for(&self, provider: ProviderId) -> Vec<AccountTarget> {
        fn targets<I: crate::core::AccountIdentity>(
            data: &crate::core::DirectoryAccountData<I>,
        ) -> Vec<AccountTarget> {
            data.accounts
                .iter()
                .map(|account| AccountTarget {
                    id: account.id.to_string(),
                    label: account.label.clone(),
                    tint: account.tint.clone(),
                    config_dir: account.config_dir.clone(),
                })
                .collect()
        }
        match provider {
            ProviderId::Codex => targets(&self.codex),
            ProviderId::Claude => targets(&self.claude),
            _ => Vec::new(),
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

    /// Accent color of the active account for `provider`, when one is set.
    pub fn active_tint_for(&self, provider: ProviderId) -> Option<&str> {
        match provider {
            ProviderId::Codex => self.codex.active_account()?.tint.as_deref(),
            ProviderId::Claude => self.claude.active_account()?.tint.as_deref(),
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
    fn every_configured_account_is_fetched_not_just_the_active_one() {
        let mut accounts = ConfiguredAccounts::default();
        accounts
            .codex
            .add_account(DirectoryAccount::<CodexIdentity>::new(
                Some("personal".to_string()),
                PathBuf::from("/homes/personal"),
            ));
        accounts
            .codex
            .add_account(DirectoryAccount::<CodexIdentity>::new(
                Some("work".to_string()),
                PathBuf::from("/homes/work"),
            ));

        let targets = accounts.targets_for(ProviderId::Codex);

        // Both seats are read side by side; fetching only the active one is what
        // made a second account replace the first rather than join it.
        assert_eq!(targets.len(), 2);
        let dirs: Vec<_> = targets.iter().map(|t| t.config_dir.clone()).collect();
        assert!(dirs.contains(&PathBuf::from("/homes/personal")));
        assert!(dirs.contains(&PathBuf::from("/homes/work")));
        // Ids must be distinct, or the cache would collapse them again.
        assert_ne!(targets[0].id, targets[1].id);
    }

    #[test]
    fn nothing_configured_yields_no_targets_so_the_caller_follows_the_cli() {
        let accounts = ConfiguredAccounts::default();

        assert!(accounts.targets_for(ProviderId::Codex).is_empty());
        assert!(accounts.targets_for(ProviderId::Claude).is_empty());
        assert!(accounts.targets_for(ProviderId::Cursor).is_empty());
    }

    #[test]
    fn only_directory_backed_providers_are_supported() {
        assert!(ConfiguredAccounts::supports(ProviderId::Codex));
        assert!(ConfiguredAccounts::supports(ProviderId::Claude));
        assert!(!ConfiguredAccounts::supports(ProviderId::Cursor));
        assert!(!ConfiguredAccounts::supports(ProviderId::Gemini));
    }
}
