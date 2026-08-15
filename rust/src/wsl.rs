//! WSL (Windows Subsystem for Linux) detection and environment helpers
//!
//! Provides utilities to detect if CodexBar is running inside WSL,
//! and to resolve Windows filesystem paths from within the Linux environment.

use std::path::{Path, PathBuf};

/// Explicit override for the Windows profile used from WSL.
/// Accepts a username (`Alice`) or a profile path (`C:\Users\Alice`).
pub const WINDOWS_USERNAME_SETTING_ENV: &str = "CEILING_WINDOWS_USERNAME";

/// WSL distribution information
#[derive(Debug, Clone)]
pub struct WslInfo {
    pub distro_name: String,
    pub windows_username: Option<String>,
    pub drive_mount: PathBuf,
}

/// Detect if we are running inside WSL
pub fn is_wsl() -> bool {
    if let Ok(version) = std::fs::read_to_string("/proc/version") {
        let v = version.to_lowercase();
        if v.contains("microsoft") || v.contains("wsl") {
            return true;
        }
    }

    if std::env::var("WSL_DISTRO_NAME").is_ok() {
        return true;
    }

    if std::path::Path::new("/run/WSL").exists() {
        return true;
    }

    false
}

/// Get WSL environment information.
/// Returns None if not running inside WSL.
pub fn get_wsl_info() -> Option<WslInfo> {
    if !is_wsl() {
        return None;
    }

    let distro_name = std::env::var("WSL_DISTRO_NAME")
        .or_else(|_| {
            std::fs::read_to_string("/etc/os-release").map(|content| {
                content
                    .lines()
                    .find(|l| l.starts_with("NAME="))
                    .map(|l| l.trim_start_matches("NAME=").trim_matches('"').to_string())
                    .unwrap_or_else(|| "Unknown".to_string())
            })
        })
        .unwrap_or_else(|_| "Unknown".to_string());

    let drive_mount = PathBuf::from("/mnt/c");
    let windows_username = resolve_windows_username(&drive_mount);

    Some(WslInfo {
        distro_name,
        windows_username,
        drive_mount,
    })
}

/// Resolve the Windows username by looking at /mnt/c/Users/.
///
/// Fail closed: never pick an arbitrary profile from readdir order. Require an
/// explicit match from `CEILING_WINDOWS_USERNAME`, `USERPROFILE`, or `USER`.
fn resolve_windows_username(drive_mount: &Path) -> Option<String> {
    let users_dir = drive_mount.join("Users");
    if !users_dir.is_dir() {
        return None;
    }

    resolve_windows_username_from(
        &users_dir,
        std::env::var(WINDOWS_USERNAME_SETTING_ENV).ok(),
        std::env::var("USERPROFILE").ok(),
        std::env::var("USER").ok(),
    )
}

fn resolve_windows_username_from(
    users_dir: &Path,
    setting: Option<impl AsRef<str>>,
    userprofile: Option<impl AsRef<str>>,
    user: Option<impl AsRef<str>>,
) -> Option<String> {
    if let Some(name) = setting
        .as_ref()
        .and_then(|value| explicit_profile_name(users_dir, value.as_ref()))
    {
        return Some(name);
    }
    if let Some(name) = userprofile
        .as_ref()
        .and_then(|value| username_from_userprofile(users_dir, value.as_ref()))
    {
        return Some(name);
    }
    if let Some(name) = user
        .as_ref()
        .and_then(|value| explicit_profile_name(users_dir, value.as_ref()))
    {
        return Some(name);
    }
    None
}

fn explicit_profile_name(users_dir: &Path, raw: &str) -> Option<String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return None;
    }

    if let Some(matched) =
        username_from_profile_path(trimmed).and_then(|name| existing_profile_name(users_dir, &name))
    {
        return Some(matched);
    }

    existing_profile_name(users_dir, trimmed)
}

fn username_from_userprofile(users_dir: &Path, userprofile: &str) -> Option<String> {
    let name = username_from_profile_path(userprofile)?;
    existing_profile_name(users_dir, &name)
}

fn username_from_profile_path(raw: &str) -> Option<String> {
    let normalized = raw.replace('\\', "/");
    let trimmed = normalized.trim().trim_end_matches('/');
    if trimmed.is_empty() {
        return None;
    }

    let path = Path::new(trimmed);
    let name = path.file_name()?.to_string_lossy();
    if name.is_empty() || name.contains(':') {
        return None;
    }
    Some(name.into_owned())
}

fn existing_profile_name(users_dir: &Path, name: &str) -> Option<String> {
    let name = name.trim().trim_end_matches(['/', '\\']);
    if name.is_empty() || is_system_user_dir(name) || name.contains(['/', '\\', ':']) {
        return None;
    }

    let direct = users_dir.join(name);
    if is_windows_profile_dir(&direct) {
        return Some(name.to_string());
    }

    let wanted = name.to_lowercase();
    let entries = std::fs::read_dir(users_dir).ok()?;
    entries.flatten().find_map(|entry| {
        let found = entry.file_name().to_string_lossy().to_string();
        (found.to_lowercase() == wanted && is_windows_profile_dir(&entry.path())).then_some(found)
    })
}

fn is_windows_profile_dir(path: &Path) -> bool {
    path.is_dir() && path.join("AppData").exists()
}

fn is_system_user_dir(name: &str) -> bool {
    matches!(
        name.to_lowercase().as_str(),
        "public"
            | "default"
            | "default user"
            | "all users"
            | "desktop"
            | "administrator"
            | "$recycle.bin"
            | "system volume information"
    )
}

/// Convert a Windows path to its WSL equivalent.
///
/// `C:\Users\John\AppData\Local` becomes `/mnt/c/Users/John/AppData/Local`.
#[allow(dead_code)]
pub fn windows_path_to_wsl(windows_path: &str) -> Option<PathBuf> {
    let path = windows_path.replace('\\', "/");

    if path.len() >= 2 && path.as_bytes()[1] == b':' {
        let drive_letter = (path.as_bytes()[0] as char).to_lowercase().next()?;
        let rest = path[2..].trim_start_matches('/');
        return Some(PathBuf::from(format!("/mnt/{}/{}", drive_letter, rest)));
    }

    None
}

/// Get the Windows AppData/Local path from within WSL
pub fn windows_appdata_local() -> Option<PathBuf> {
    let info = get_wsl_info()?;
    let user = info.windows_username?;
    Some(
        info.drive_mount
            .join("Users")
            .join(user)
            .join("AppData")
            .join("Local"),
    )
}

/// Get the Windows AppData/Roaming path from within WSL
pub fn windows_appdata_roaming() -> Option<PathBuf> {
    let info = get_wsl_info()?;
    let user = info.windows_username?;
    Some(
        info.drive_mount
            .join("Users")
            .join(user)
            .join("AppData")
            .join("Roaming"),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn write_profile(users_dir: &Path, name: &str) {
        fs::create_dir_all(users_dir.join(name).join("AppData")).expect("profile dir");
    }

    fn users_fixture(names: &[&str]) -> (tempfile::TempDir, PathBuf) {
        let temp = tempfile::tempdir().expect("temp dir");
        let users_dir = temp.path().join("Users");
        fs::create_dir_all(&users_dir).expect("users dir");
        for name in names {
            write_profile(&users_dir, name);
        }
        (temp, users_dir)
    }

    #[test]
    fn test_is_system_user_dir() {
        assert!(is_system_user_dir("Public"));
        assert!(is_system_user_dir("Default"));
        assert!(is_system_user_dir("Default User"));
        assert!(!is_system_user_dir("John"));
        assert!(!is_system_user_dir("alice"));
    }

    #[test]
    fn test_windows_path_to_wsl() {
        assert_eq!(
            windows_path_to_wsl(r"C:\Users\John\AppData\Local"),
            Some(PathBuf::from("/mnt/c/Users/John/AppData/Local"))
        );
        assert_eq!(
            windows_path_to_wsl("D:\\Games"),
            Some(PathBuf::from("/mnt/d/Games"))
        );
        assert_eq!(windows_path_to_wsl("/home/user"), None);
    }

    #[test]
    fn resolve_matches_user_when_profile_exists() {
        let (_temp, users_dir) = users_fixture(&["Alice", "Bob"]);
        assert_eq!(
            resolve_windows_username_from(
                &users_dir,
                None::<String>,
                None::<String>,
                Some("Alice")
            ),
            Some("Alice".into())
        );
    }

    #[test]
    fn resolve_fails_closed_when_user_does_not_match() {
        let (_temp, users_dir) = users_fixture(&["Alice", "Bob"]);
        assert_eq!(
            resolve_windows_username_from(
                &users_dir,
                None::<String>,
                None::<String>,
                Some("wsluser")
            ),
            None
        );
    }

    #[test]
    fn resolve_matches_userprofile_windows_path() {
        let (_temp, users_dir) = users_fixture(&["Alice", "Bob"]);
        assert_eq!(
            resolve_windows_username_from(
                &users_dir,
                None::<String>,
                Some(r"C:\Users\Bob"),
                Some("wsluser"),
            ),
            Some("Bob".into())
        );
    }

    #[test]
    fn resolve_matches_userprofile_wsl_path() {
        let (_temp, users_dir) = users_fixture(&["Alice", "Bob"]);
        assert_eq!(
            resolve_windows_username_from(
                &users_dir,
                None::<String>,
                Some("/mnt/c/Users/Alice/"),
                Some("wsluser"),
            ),
            Some("Alice".into())
        );
    }

    #[test]
    fn resolve_prefers_explicit_setting_over_userprofile() {
        let (_temp, users_dir) = users_fixture(&["Alice", "Bob"]);
        assert_eq!(
            resolve_windows_username_from(
                &users_dir,
                Some("Alice"),
                Some(r"C:\Users\Bob"),
                Some("Bob"),
            ),
            Some("Alice".into())
        );
    }

    #[test]
    fn resolve_matches_setting_profile_path() {
        let (_temp, users_dir) = users_fixture(&["Alice", "Bob"]);
        assert_eq!(
            resolve_windows_username_from(
                &users_dir,
                Some(r"C:\Users\Bob"),
                None::<String>,
                Some("wsluser"),
            ),
            Some("Bob".into())
        );
    }

    #[test]
    fn resolve_rejects_system_and_unknown_setting() {
        let (_temp, users_dir) = users_fixture(&["Alice", "Public"]);
        assert_eq!(
            resolve_windows_username_from(
                &users_dir,
                Some("Public"),
                None::<String>,
                None::<String>
            ),
            None
        );
        assert_eq!(
            resolve_windows_username_from(
                &users_dir,
                Some("Missing"),
                None::<String>,
                None::<String>
            ),
            None
        );
    }

    #[test]
    fn resolve_matches_user_case_insensitively() {
        let (_temp, users_dir) = users_fixture(&["Alice"]);
        assert_eq!(
            resolve_windows_username_from(
                &users_dir,
                None::<String>,
                None::<String>,
                Some("alice")
            ),
            Some("Alice".into())
        );
    }

    #[test]
    fn resolve_does_not_guess_first_profile() {
        let (_temp, users_dir) = users_fixture(&["Alice", "Bob"]);
        assert_eq!(
            resolve_windows_username_from(
                &users_dir,
                None::<String>,
                None::<String>,
                None::<String>
            ),
            None
        );
    }
}
