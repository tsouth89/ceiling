//! Codex local session directory discovery.

use std::collections::HashSet;
use std::fs;
use std::path::PathBuf;

pub(crate) fn codex_sessions_dir_candidates(
    home_dir: Option<PathBuf>,
    codex_home: Option<String>,
    custom_dirs: &[String],
    wsl_roots: &[PathBuf],
) -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    let mut seen = HashSet::new();

    if let Some(sessions_dir) = codex_home.as_deref().and_then(normalize_codex_sessions_dir) {
        push_unique_path(&mut dirs, &mut seen, sessions_dir);
    } else if codex_home
        .as_deref()
        .is_none_or(|value| value.trim().is_empty())
        && let Some(home) = home_dir
    {
        push_unique_path(&mut dirs, &mut seen, home.join(".codex").join("sessions"));
    }

    for custom_dir in custom_dirs {
        if let Some(sessions_dir) = normalize_codex_sessions_dir(custom_dir) {
            push_unique_path(&mut dirs, &mut seen, sessions_dir);
        }
    }

    for sessions_dir in discover_wsl_codex_sessions_dirs(wsl_roots) {
        push_unique_path(&mut dirs, &mut seen, sessions_dir);
    }

    dirs
}

pub(crate) fn default_wsl_roots() -> Vec<PathBuf> {
    if !cfg!(windows) {
        return Vec::new();
    }

    let preferred = PathBuf::from(r"\\wsl.localhost");
    if fs::read_dir(&preferred).is_ok() {
        return vec![preferred];
    }

    vec![PathBuf::from(r"\\wsl$")]
}

/// Final segment of a path string, treating `\\` as a separator on every host.
///
/// These strings arrive from configuration and session logs written by other
/// machines, so a Windows path can be read on Linux or inside WSL. There,
/// `Path::file_name` does not treat `\\` as a separator and hands back the whole
/// string, which is how a `cwd` of `C:\\projects\\ceiling` became a "project name".
pub(crate) fn last_path_segment(value: &str) -> Option<&str> {
    value
        .rsplit(['/', '\\'])
        .find(|segment| !segment.trim().is_empty())
}

fn normalize_codex_sessions_dir(path: impl AsRef<str>) -> Option<PathBuf> {
    let trimmed = path.as_ref().trim();
    if trimmed.is_empty() {
        return None;
    }

    if last_path_segment(trimmed).is_some_and(|name| name.eq_ignore_ascii_case("sessions")) {
        return Some(PathBuf::from(trimmed));
    }
    // Keep the separator the caller wrote, so a Windows path stays a Windows
    // path on a host whose own separator is `/`. On Windows the two agree.
    let separator = if trimmed.contains('\\') {
        '\\'
    } else {
        std::path::MAIN_SEPARATOR
    };
    Some(PathBuf::from(format!(
        "{}{separator}sessions",
        trimmed.trim_end_matches(['/', '\\'])
    )))
}

fn discover_wsl_codex_sessions_dirs(wsl_roots: &[PathBuf]) -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    let mut seen = HashSet::new();

    for wsl_root in wsl_roots {
        let Ok(distros) = fs::read_dir(wsl_root) else {
            continue;
        };

        for distro in distros.flatten() {
            let distro_path = distro.path();
            let homes_dir = distro_path.join("home");
            if let Ok(users) = fs::read_dir(&homes_dir) {
                for user in users.flatten() {
                    let sessions_dir = user.path().join(".codex").join("sessions");
                    if sessions_dir.exists() {
                        push_unique_path(&mut dirs, &mut seen, sessions_dir);
                    }
                }
            }

            let root_sessions_dir = distro_path.join("root").join(".codex").join("sessions");
            if root_sessions_dir.exists() {
                push_unique_path(&mut dirs, &mut seen, root_sessions_dir);
            }
        }
    }

    dirs
}

fn push_unique_path(dirs: &mut Vec<PathBuf>, seen: &mut HashSet<String>, path: PathBuf) {
    let key = path
        .to_string_lossy()
        .replace('/', "\\")
        .to_ascii_lowercase();
    if seen.insert(key) {
        dirs.push(path);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes_codex_root_to_sessions_dir() {
        assert_eq!(
            normalize_codex_sessions_dir(r"\\wsl.localhost\archlinux\home\kk\.codex"),
            Some(PathBuf::from(
                r"\\wsl.localhost\archlinux\home\kk\.codex\sessions"
            ))
        );
        assert_eq!(
            normalize_codex_sessions_dir(r"C:\Users\me\.codex\sessions"),
            Some(PathBuf::from(r"C:\Users\me\.codex\sessions"))
        );
        assert_eq!(normalize_codex_sessions_dir("  "), None);
    }

    /// Pins the cross-host part: these strings come from other machines, so a
    /// Windows path has to split the same way whatever host is reading it.
    /// `Path::file_name` only honours `\` on Windows, which is what made a
    /// `cwd` of `C:\projects\ceiling` read as a project called
    /// `C:\projects\ceiling`.
    #[test]
    fn a_windows_path_splits_the_same_on_every_host() {
        assert_eq!(
            last_path_segment(r"C:\projects\personal\ceiling"),
            Some("ceiling")
        );
        assert_eq!(
            last_path_segment(r"\\wsl.localhost\archlinux\home\kk\.codex"),
            Some(".codex")
        );
        assert_eq!(
            last_path_segment("/home/kk/projects/ceiling"),
            Some("ceiling")
        );
        // Mixed, and trailing separators, both of which appear in the wild.
        assert_eq!(
            last_path_segment(r"C:\projects/personal\ceiling\"),
            Some("ceiling")
        );
        assert_eq!(last_path_segment(""), None);
        assert_eq!(last_path_segment(r"\\"), None);
    }

    /// A Windows path keeps its own separator rather than picking up the
    /// host's, so the joined result is still a usable Windows path.
    #[test]
    fn joining_sessions_keeps_the_callers_separator() {
        assert_eq!(
            normalize_codex_sessions_dir(r"C:\Users\me\.codex"),
            Some(PathBuf::from(r"C:\Users\me\.codex\sessions"))
        );
        assert_eq!(
            normalize_codex_sessions_dir("/home/kk/.codex"),
            Some(PathBuf::from(format!(
                "/home/kk/.codex{}sessions",
                std::path::MAIN_SEPARATOR
            )))
        );
        // Already a sessions dir, in either style.
        assert_eq!(
            normalize_codex_sessions_dir(r"C:\Users\me\.codex\SESSIONS"),
            Some(PathBuf::from(r"C:\Users\me\.codex\SESSIONS"))
        );
    }

    #[test]
    fn discovers_wsl_codex_sessions_dirs_from_distro_homes() {
        let base = std::env::temp_dir().join(format!("codexbar-wsl-roots-{}", std::process::id()));
        let distro = base.join("Ubuntu");
        let user_sessions = distro
            .join("home")
            .join("alice")
            .join(".codex")
            .join("sessions");
        let root_sessions = distro.join("root").join(".codex").join("sessions");
        fs::create_dir_all(&user_sessions).unwrap();
        fs::create_dir_all(&root_sessions).unwrap();

        let dirs = discover_wsl_codex_sessions_dirs(std::slice::from_ref(&base));

        assert!(dirs.contains(&user_sessions));
        assert!(dirs.contains(&root_sessions));
        let _ = fs::remove_dir_all(base);
    }

    #[test]
    fn blank_codex_home_falls_back_to_default_home_sessions_dir() {
        let home = PathBuf::from(r"C:\Users\me");
        let dirs =
            codex_sessions_dir_candidates(Some(home.clone()), Some("  ".to_string()), &[], &[]);

        assert_eq!(dirs, vec![home.join(".codex").join("sessions")]);
    }
}
