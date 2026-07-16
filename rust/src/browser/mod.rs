//! Browser detection and cookie extraction for Windows and WSL

pub mod cookies;
pub mod detection;
pub mod watchdog;
pub mod wsl_paths;

/// Remove plaintext cookie-header caches written by older releases.
///
/// The legacy cache was never used by the current desktop app, but old files
/// may still contain reusable session cookies. Only regular files matching the
/// exact legacy suffix inside the dedicated CodexBar data directory are
/// removed; symlinks and directories are ignored.
pub fn remove_legacy_cookie_caches() -> std::io::Result<usize> {
    let Some(directory) = dirs::data_local_dir().map(|path| path.join("CodexBar")) else {
        return Ok(0);
    };
    remove_legacy_cookie_caches_from(&directory)
}

fn remove_legacy_cookie_caches_from(directory: &std::path::Path) -> std::io::Result<usize> {
    let entries = match std::fs::read_dir(directory) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(0),
        Err(error) => return Err(error),
    };

    let mut removed = 0;
    for entry in entries {
        let entry = entry?;
        let file_type = entry.file_type()?;
        let matches_legacy_name = entry
            .file_name()
            .to_str()
            .is_some_and(|name| name.ends_with("-cookie.json"));
        if matches_legacy_name && file_type.is_file() && !file_type.is_symlink() {
            std::fs::remove_file(entry.path())?;
            removed += 1;
        }
    }
    Ok(removed)
}

// Re-exports for future UI integration
#[allow(unused_imports)]
pub use watchdog::{WatchdogConfig, WatchdogError, WebProbeWatchdog, global_watchdog};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn legacy_cookie_cleanup_is_narrow_and_idempotent() {
        let directory = std::env::temp_dir().join(format!(
            "ceiling_legacy_cookie_cleanup_{}",
            uuid::Uuid::new_v4()
        ));
        std::fs::create_dir_all(&directory).unwrap();
        let legacy = directory.join("claude-cookie.json");
        let unrelated = directory.join("settings.json");
        let lookalike_directory = directory.join("nested-cookie.json");
        std::fs::write(&legacy, "secret").unwrap();
        std::fs::write(&unrelated, "keep").unwrap();
        std::fs::create_dir(&lookalike_directory).unwrap();

        assert_eq!(remove_legacy_cookie_caches_from(&directory).unwrap(), 1);
        assert!(!legacy.exists());
        assert!(unrelated.exists());
        assert!(lookalike_directory.is_dir());
        assert_eq!(remove_legacy_cookie_caches_from(&directory).unwrap(), 0);

        std::fs::remove_dir_all(directory).unwrap();
    }
}
