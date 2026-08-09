//! Config command implementation
//!
//! Utilities for validating and inspecting configuration.

use clap::{Parser, Subcommand};
use serde::de::DeserializeOwned;
use std::path::Path;

use crate::core::{ProviderId, TokenAccountStore, instantiate_provider};
use crate::settings::{ApiKeys, ManualCookies, Settings};

/// Arguments for the config command
#[derive(Parser, Debug)]
pub struct ConfigArgs {
    #[command(subcommand)]
    pub command: ConfigCommand,
}

#[derive(Subcommand, Debug)]
pub enum ConfigCommand {
    /// Validate configuration files
    Validate,
    /// Dump configuration to stdout
    Dump {
        /// Output format: json or toml
        #[arg(short, long, default_value = "json")]
        format: String,
    },
    /// List providers and enabled state
    Providers,
    /// Enable a provider
    Enable {
        /// Provider CLI name or alias
        provider: String,
    },
    /// Disable a provider
    Disable {
        /// Provider CLI name or alias
        provider: String,
    },
    /// Store an API key for a provider
    SetApiKey {
        /// Provider CLI name or alias
        provider: String,
        /// API key to store
        #[arg(long = "api-key")]
        api_key: Option<String>,
        /// Read API key from stdin
        #[arg(long)]
        stdin: bool,
        /// Store the key without enabling the provider
        #[arg(long = "no-enable")]
        no_enable: bool,
    },
    /// Show configuration file paths
    Path,
}

/// Run the config command
pub async fn run(args: ConfigArgs) -> anyhow::Result<()> {
    match args.command {
        ConfigCommand::Validate => validate_config().await,
        ConfigCommand::Dump { format } => dump_config(&format).await,
        ConfigCommand::Providers => list_providers().await,
        ConfigCommand::Enable { provider } => set_provider_enabled(&provider, true).await,
        ConfigCommand::Disable { provider } => set_provider_enabled(&provider, false).await,
        ConfigCommand::SetApiKey {
            provider,
            api_key,
            stdin,
            no_enable,
        } => set_api_key(&provider, api_key.as_deref(), stdin, !no_enable).await,
        ConfigCommand::Path => show_paths().await,
    }
}

/// Validate configuration files
async fn validate_config() -> anyhow::Result<()> {
    let mut report = ValidationReport::default();
    validate_settings_config(&mut report);
    validate_manual_cookies_config(&mut report);
    validate_token_accounts_config(&mut report);
    report.finish()
}

#[derive(Default)]
struct ValidationReport {
    errors: Vec<String>,
    warnings: Vec<String>,
}

impl ValidationReport {
    fn error(&mut self, message: impl Into<String>) {
        self.errors.push(message.into());
    }

    fn warning(&mut self, message: impl Into<String>) {
        self.warnings.push(message.into());
    }

    fn finish(self) -> anyhow::Result<()> {
        print_validation_summary(&self.errors, &self.warnings)
    }
}

fn validate_settings_config(report: &mut ValidationReport) {
    print!("Checking settings.json... ");
    let Some(path) = Settings::settings_path() else {
        println!("ERROR");
        report.error("settings.json: Could not determine config path");
        return;
    };

    if validate_optional_json_file::<Settings>(&path, "settings.json", report) {
        return;
    }

    println!("NOT FOUND (using defaults)");
    report.warning("settings.json: File does not exist, using defaults");
}

fn validate_manual_cookies_config(report: &mut ValidationReport) {
    print!("Checking manual_cookies.json... ");
    let Some(path) = ManualCookies::cookies_path() else {
        println!("SKIP");
        return;
    };

    if !validate_optional_json_file::<ManualCookies>(&path, "manual_cookies.json", report) {
        println!("NOT FOUND (none configured)");
    }
}

fn validate_optional_json_file<T>(path: &Path, label: &str, report: &mut ValidationReport) -> bool
where
    T: DeserializeOwned,
{
    if !path.exists() {
        return false;
    }

    match read_json_config::<T>(path) {
        Ok(()) => println!("OK"),
        Err(ConfigFileError::Read(e)) => {
            println!("ERROR");
            report.error(format!("{label}: Could not read file: {e}"));
        }
        Err(ConfigFileError::Parse(e)) => {
            println!("INVALID");
            report.error(format!("{label}: {e}"));
        }
    }

    true
}

fn read_json_config<T>(path: &Path) -> Result<(), ConfigFileError>
where
    T: DeserializeOwned,
{
    let content = std::fs::read_to_string(path).map_err(ConfigFileError::Read)?;
    serde_json::from_str::<T>(&content)
        .map(|_| ())
        .map_err(ConfigFileError::Parse)
}

enum ConfigFileError {
    Read(std::io::Error),
    Parse(serde_json::Error),
}

fn validate_token_accounts_config(report: &mut ValidationReport) {
    print!("Checking token-accounts.json... ");
    let path = TokenAccountStore::default_path();
    if !path.exists() {
        println!("NOT FOUND (none configured)");
        return;
    }

    match TokenAccountStore::new().load() {
        Ok(_) => println!("OK"),
        Err(e) => {
            println!("INVALID");
            report.error(format!("token-accounts.json: {e}"));
        }
    }
}

fn print_validation_summary(errors: &[String], warnings: &[String]) -> anyhow::Result<()> {
    println!();
    if errors.is_empty() && warnings.is_empty() {
        println!("Configuration is valid.");
    } else {
        if !warnings.is_empty() {
            println!("Warnings:");
            for w in warnings {
                println!("  - {}", w);
            }
        }
        if !errors.is_empty() {
            println!("Errors:");
            for e in errors {
                println!("  - {}", e);
            }
            anyhow::bail!(
                "Configuration validation failed with {} error(s).",
                errors.len()
            );
        }
    }

    Ok(())
}

/// Dump configuration to stdout
async fn dump_config(format: &str) -> anyhow::Result<()> {
    let settings = Settings::load();

    match format.to_lowercase().as_str() {
        "json" => {
            let json = serde_json::to_string_pretty(&settings)?;
            println!("{}", json);
        }
        "toml" => {
            let toml = toml::to_string_pretty(&settings)?;
            println!("{}", toml);
        }
        _ => {
            anyhow::bail!("Unknown format '{}'. Supported formats: json, toml", format);
        }
    }

    Ok(())
}

/// List provider enabled state.
async fn list_providers() -> anyhow::Result<()> {
    let settings = Settings::load();
    for id in ProviderId::all() {
        let state = if settings.is_provider_enabled(*id) {
            "enabled"
        } else {
            "disabled"
        };
        let default_marker = if instantiate_provider(*id).metadata().default_enabled {
            " default"
        } else {
            ""
        };
        println!(
            "{}: {}{} ({})",
            id.cli_name(),
            state,
            default_marker,
            id.display_name()
        );
    }
    Ok(())
}

/// Enable or disable a provider by CLI name.
async fn set_provider_enabled(provider: &str, enabled: bool) -> anyhow::Result<()> {
    let id = parse_provider(provider)?;
    let mut settings = Settings::load();
    if enabled {
        settings.enable_provider(id);
    } else {
        settings.disable_provider(id);
    }
    settings.save()?;
    let state = if enabled { "enabled" } else { "disabled" };
    println!("Config: {state} {}", id.display_name());
    Ok(())
}

/// Store an API key and optionally enable the provider.
async fn set_api_key(
    provider: &str,
    api_key: Option<&str>,
    read_from_stdin: bool,
    enable_provider: bool,
) -> anyhow::Result<()> {
    let id = parse_provider(provider)?;
    ensure_provider_accepts_api_key(id)?;
    let api_key = resolve_api_key_input(api_key, read_from_stdin)?;

    let mut keys = ApiKeys::load_for_update()?;
    keys.set(id.cli_name(), &api_key, None);
    keys.save()?;

    if enable_provider {
        let mut settings = Settings::load();
        settings.enable_provider(id);
        settings.save()?;
    }

    let suffix = if enable_provider { " and enabled" } else { "" };
    println!("Config: stored API key for {}{suffix}", id.display_name());
    Ok(())
}

fn parse_provider(raw: &str) -> anyhow::Result<ProviderId> {
    ProviderId::from_cli_name(raw).ok_or_else(|| {
        anyhow::anyhow!(
            "Unknown provider '{}'. Run `codexbar config providers` to list providers.",
            raw
        )
    })
}

fn ensure_provider_accepts_api_key(id: ProviderId) -> anyhow::Result<()> {
    if crate::settings::get_api_key_providers()
        .iter()
        .any(|provider| provider.id == id)
    {
        return Ok(());
    }
    anyhow::bail!("{} does not support stored API keys.", id.display_name())
}

fn resolve_api_key_input(api_key: Option<&str>, read_from_stdin: bool) -> anyhow::Result<String> {
    if api_key.is_some() && read_from_stdin {
        anyhow::bail!("Use either --api-key or --stdin, not both.");
    }

    let raw = if read_from_stdin {
        let mut buffer = String::new();
        use std::io::Read;
        std::io::stdin().read_to_string(&mut buffer)?;
        Some(buffer)
    } else {
        api_key.map(ToString::to_string)
    };

    let mut value = raw
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .ok_or_else(|| anyhow::anyhow!("Missing API key. Pass --api-key <key> or use --stdin."))?;

    if (value.starts_with('"') && value.ends_with('"'))
        || (value.starts_with('\'') && value.ends_with('\''))
    {
        value.remove(0);
        value.pop();
    }

    let value = value.trim().to_string();
    if value.is_empty() {
        anyhow::bail!("Missing API key. Pass --api-key <key> or use --stdin.");
    }
    Ok(value)
}

/// Show configuration file paths
async fn show_paths() -> anyhow::Result<()> {
    println!("Configuration paths:");

    if let Some(path) = Settings::settings_path() {
        let exists = if path.exists() { "" } else { " (not found)" };
        println!("  Settings:       {}{}", path.display(), exists);
    } else {
        println!("  Settings:       (could not determine path)");
    }

    if let Some(path) = ManualCookies::cookies_path() {
        let exists = if path.exists() { "" } else { " (not found)" };
        println!("  Manual cookies: {}{}", path.display(), exists);
    } else {
        println!("  Manual cookies: (could not determine path)");
    }

    let token_path = TokenAccountStore::default_path();
    let exists = if token_path.exists() {
        ""
    } else {
        " (not found)"
    };
    println!("  Token accounts: {}{}", token_path.display(), exists);

    // Show config directory
    if let Some(config_dir) = dirs::config_dir() {
        let codexbar_dir = config_dir.join("CodexBar");
        println!();
        println!("Config directory: {}", codexbar_dir.display());
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_provider_accepts_a_known_provider() {
        assert_eq!(parse_provider("codex").unwrap(), ProviderId::Codex);
    }

    #[test]
    fn parse_provider_rejects_an_unknown_provider() {
        assert!(parse_provider("unknown").is_err());
    }

    #[test]
    fn ensure_provider_accepts_api_key_for_key_providers() {
        assert!(ensure_provider_accepts_api_key(ProviderId::Amp).is_ok());
    }

    #[test]
    fn ensure_provider_rejects_providers_without_stored_keys() {
        assert!(ensure_provider_accepts_api_key(ProviderId::Codex).is_err());
    }

    #[test]
    fn resolve_api_key_input_uses_an_inline_key() {
        assert_eq!(
            resolve_api_key_input(Some("secret"), false).unwrap(),
            "secret"
        );
    }

    #[test]
    fn resolve_api_key_input_rejects_both_inline_and_stdin() {
        assert!(resolve_api_key_input(Some("secret"), true).is_err());
    }

    #[test]
    fn resolve_api_key_input_rejects_missing_input() {
        assert!(resolve_api_key_input(None, false).is_err());
        assert!(resolve_api_key_input(Some("   "), false).is_err());
    }

    #[test]
    fn resolve_api_key_input_strips_surrounding_quotes() {
        assert_eq!(
            resolve_api_key_input(Some("\"secret\""), false).unwrap(),
            "secret"
        );
    }
}
