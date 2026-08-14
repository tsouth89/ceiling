//! Resolve Windows-owned system utilities without consulting PATH.
//!
//! `powershell` and `where` belong to Windows. Launching them by bare name
//! lets a hostile current directory or PATH entry substitute the binary.
//! Third-party tools such as Claude, Codex, and `gh` still use PATH lookup.

use std::ffi::OsString;
use std::path::{Path, PathBuf};

/// Resolve `name` under `%SystemRoot%\System32` when that file exists.
pub fn windows_system_exe(name: &str) -> PathBuf {
    resolve_windows_system_exe(name, std::env::var_os("SystemRoot"), |path| path.exists())
}

/// Resolve Windows PowerShell under the trusted System32 tree.
pub fn windows_powershell_exe() -> PathBuf {
    resolve_windows_powershell(std::env::var_os("SystemRoot"), |path| path.exists())
}

/// Resolve `where.exe` under `%SystemRoot%\System32`.
pub fn windows_where_exe() -> PathBuf {
    windows_system_exe("where.exe")
}

pub(crate) fn resolve_windows_system_exe(
    name: &str,
    system_root: Option<OsString>,
    exists: impl Fn(&Path) -> bool,
) -> PathBuf {
    if let Some(root) = system_root {
        let candidate = PathBuf::from(root).join("System32").join(name);
        if exists(&candidate) {
            return candidate;
        }
    }
    PathBuf::from(name)
}

pub(crate) fn resolve_windows_powershell(
    system_root: Option<OsString>,
    exists: impl Fn(&Path) -> bool,
) -> PathBuf {
    if let Some(root) = system_root {
        let candidate = PathBuf::from(root)
            .join("System32")
            .join("WindowsPowerShell")
            .join("v1.0")
            .join("powershell.exe");
        if exists(&candidate) {
            return candidate;
        }
    }
    PathBuf::from("powershell.exe")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;
    use std::path::PathBuf;

    fn exists_in(trusted: HashSet<PathBuf>) -> impl Fn(&Path) -> bool {
        move |path| trusted.contains(path)
    }

    fn trusted_root() -> PathBuf {
        PathBuf::from("/Windows")
    }

    #[test]
    fn prefers_system32_over_bare_name() {
        let root = OsString::from(trusted_root());
        let powershell = trusted_root()
            .join("System32")
            .join("WindowsPowerShell")
            .join("v1.0")
            .join("powershell.exe");
        let where_exe = trusted_root().join("System32").join("where.exe");
        let exists = exists_in(HashSet::from([powershell.clone(), where_exe.clone()]));

        assert_eq!(
            resolve_windows_powershell(Some(root.clone()), &exists),
            powershell
        );
        assert_eq!(
            resolve_windows_system_exe("where.exe", Some(root), &exists),
            where_exe
        );
    }

    #[test]
    fn ignores_hostile_path_and_current_directory_entries() {
        let root = OsString::from(trusted_root());
        let trusted = trusted_root()
            .join("System32")
            .join("WindowsPowerShell")
            .join("v1.0")
            .join("powershell.exe");
        let exists = exists_in(HashSet::from([
            trusted.clone(),
            PathBuf::from("/evil/powershell.exe"),
            PathBuf::from("./powershell.exe"),
        ]));

        assert_eq!(resolve_windows_powershell(Some(root), exists), trusted);
    }

    #[test]
    fn falls_back_to_bare_name_when_system_root_is_missing() {
        assert_eq!(
            resolve_windows_powershell(None, |_| true),
            PathBuf::from("powershell.exe")
        );
        assert_eq!(
            resolve_windows_system_exe("where.exe", None, |_| true),
            PathBuf::from("where.exe")
        );
    }

    #[test]
    fn falls_back_when_the_trusted_file_is_absent() {
        let root = OsString::from(r"C:\NotWindows");
        assert_eq!(
            resolve_windows_powershell(Some(root.clone()), |_| false),
            PathBuf::from("powershell.exe")
        );
        assert_eq!(
            resolve_windows_system_exe("where.exe", Some(root), |_| false),
            PathBuf::from("where.exe")
        );
    }
}
