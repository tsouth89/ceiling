//! Auto-start management for Windows
//!
//! Enables/disables CodexBar to start automatically when Windows boots

use clap::Args;

#[cfg(target_os = "windows")]
use crate::settings::Settings;

#[derive(Args, Debug)]
pub struct AutostartArgs {
    /// Enable auto-start on Windows boot
    #[arg(long, conflicts_with = "disable")]
    pub enable: bool,

    /// Disable auto-start
    #[arg(long, conflicts_with = "enable")]
    pub disable: bool,

    /// Show current auto-start status
    #[arg(long, conflicts_with_all = ["enable", "disable"])]
    pub status: bool,
}

pub async fn run(args: AutostartArgs) -> anyhow::Result<()> {
    if args.enable {
        enable_autostart()?;
        println!("Auto-start enabled. Ceiling will start when Windows boots.");
    } else if args.disable {
        disable_autostart()?;
        println!("Auto-start disabled.");
    } else {
        // Default: show status
        let enabled = is_autostart_enabled();
        if enabled {
            println!("Auto-start is enabled.");
        } else {
            println!("Auto-start is disabled.");
        }
    }
    Ok(())
}

/// Enable auto-start by adding registry entry
#[cfg(target_os = "windows")]
fn enable_autostart() -> anyhow::Result<()> {
    Settings::apply_start_at_login_registry(true)
}

/// Disable auto-start by removing registry entry
#[cfg(target_os = "windows")]
fn disable_autostart() -> anyhow::Result<()> {
    Settings::apply_start_at_login_registry(false)
}

/// Check if auto-start is enabled
#[cfg(target_os = "windows")]
fn is_autostart_enabled() -> bool {
    Settings::is_start_at_login_enabled()
}

#[cfg(not(target_os = "windows"))]
fn enable_autostart() -> anyhow::Result<()> {
    Err(anyhow::anyhow!("Auto-start is only supported on Windows"))
}

#[cfg(not(target_os = "windows"))]
fn disable_autostart() -> anyhow::Result<()> {
    Err(anyhow::anyhow!("Auto-start is only supported on Windows"))
}

#[cfg(not(target_os = "windows"))]
fn is_autostart_enabled() -> bool {
    false
}
