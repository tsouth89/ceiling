//! Small helper for storing local secret-bearing JSON files.

use std::ffi::OsString;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use base64::Engine;
use serde::{Deserialize, Serialize};

const FORMAT: &str = "codexbar.secure-file";
const VERSION: u32 = 1;
const WINDOWS_DPAPI_USER: &str = "windows-dpapi-user";
const WINDOWS_DPAPI_MACHINE: &str = "windows-dpapi-machine";
static TEMP_FILE_COUNTER: AtomicU64 = AtomicU64::new(0);

const STATE_LOCK_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10);
const STATE_LOCK_RETRY: std::time::Duration = std::time::Duration::from_millis(20);

/// Serialize read-modify-write transactions across Ceiling processes.
///
/// Every settings and credential store shares one lock so operations spanning
/// multiple files cannot interleave with a writer for any one of those files.
pub fn with_state_write_lock<T>(operation: impl FnOnce() -> io::Result<T>) -> io::Result<T> {
    let lock_path = dirs::config_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("Ceiling")
        .join("state-write.lock");
    with_state_write_lock_at(&lock_path, operation)
}

pub(crate) fn with_state_write_lock_at<T>(
    lock_path: &Path,
    operation: impl FnOnce() -> io::Result<T>,
) -> io::Result<T> {
    if let Some(parent) = lock_path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let _lock = StateWriteLock::acquire(lock_path)?;
    operation()
}

/// Serialize writes to a third-party state file without folding unrelated
/// credential paths into Ceiling's global state lock.
pub(crate) fn with_file_write_lock<T>(
    file_path: &Path,
    operation: impl FnOnce() -> io::Result<T>,
) -> io::Result<T> {
    let parent = file_path.parent().ok_or_else(|| {
        io::Error::new(io::ErrorKind::InvalidInput, "state file path has no parent")
    })?;
    let file_name = file_path.file_name().ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            "state file path has no file name",
        )
    })?;
    let mut lock_name = OsString::from(".");
    lock_name.push(file_name);
    lock_name.push(".ceiling-write.lock");
    with_state_write_lock_at(&parent.join(lock_name), operation)
}

struct StateWriteLock {
    #[cfg(windows)]
    handle: windows::Win32::Foundation::HANDLE,
    #[cfg(not(windows))]
    path: PathBuf,
}

impl StateWriteLock {
    fn acquire(path: &Path) -> io::Result<Self> {
        let deadline = std::time::Instant::now() + STATE_LOCK_TIMEOUT;
        loop {
            match Self::try_acquire(path) {
                Ok(lock) => return Ok(lock),
                Err(error)
                    if matches!(
                        error.kind(),
                        io::ErrorKind::WouldBlock | io::ErrorKind::PermissionDenied
                    ) && std::time::Instant::now() < deadline =>
                {
                    std::thread::sleep(STATE_LOCK_RETRY);
                }
                Err(error) => return Err(error),
            }
        }
    }

    #[cfg(windows)]
    fn try_acquire(path: &Path) -> io::Result<Self> {
        use std::os::windows::ffi::OsStrExt;
        use windows::Win32::Foundation::{GENERIC_READ, GENERIC_WRITE};
        use windows::Win32::Storage::FileSystem::{
            CreateFileW, FILE_ATTRIBUTE_NORMAL, FILE_SHARE_MODE, OPEN_ALWAYS,
        };
        use windows::core::PCWSTR;

        let wide: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();
        let handle = unsafe {
            CreateFileW(
                PCWSTR(wide.as_ptr()),
                GENERIC_READ.0 | GENERIC_WRITE.0,
                FILE_SHARE_MODE(0),
                None,
                OPEN_ALWAYS,
                FILE_ATTRIBUTE_NORMAL,
                None,
            )
        }
        .map_err(|error| {
            let code = error.code().0 as u32;
            let win32_code = code & 0xffff;
            if win32_code == 32 || win32_code == 33 {
                io::Error::new(io::ErrorKind::WouldBlock, "state store is locked")
            } else {
                io::Error::other(format!("could not acquire state lock: {error}"))
            }
        })?;
        Ok(Self { handle })
    }

    #[cfg(not(windows))]
    fn try_acquire(path: &Path) -> io::Result<Self> {
        std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(path)
            .map(|_| Self {
                path: path.to_path_buf(),
            })
    }
}

impl Drop for StateWriteLock {
    fn drop(&mut self) {
        #[cfg(windows)]
        unsafe {
            let _ = windows::Win32::Foundation::CloseHandle(self.handle);
        }
        #[cfg(not(windows))]
        {
            let _ = std::fs::remove_file(&self.path);
        }
    }
}

#[derive(Debug, Serialize, Deserialize)]
struct ProtectedFile {
    format: String,
    version: u32,
    protection: String,
    payload: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SecureFileStatus {
    Missing,
    Plaintext,
    Protected(String),
    Unreadable(String),
}

/// Return a non-secret storage status for diagnostics/UI surfaces.
pub fn status(path: &Path) -> SecureFileStatus {
    if !path.exists() {
        return SecureFileStatus::Missing;
    }

    let raw = match std::fs::read_to_string(path) {
        Ok(raw) => raw,
        Err(e) => return SecureFileStatus::Unreadable(e.to_string()),
    };

    let Ok(file) = serde_json::from_str::<ProtectedFile>(&raw) else {
        return SecureFileStatus::Plaintext;
    };

    if file.format != FORMAT {
        return SecureFileStatus::Plaintext;
    }
    if file.version != VERSION {
        return SecureFileStatus::Unreadable(format!(
            "unsupported secure file version {}",
            file.version
        ));
    }

    match file.protection.as_str() {
        WINDOWS_DPAPI_USER | WINDOWS_DPAPI_MACHINE => SecureFileStatus::Protected(file.protection),
        other => {
            SecureFileStatus::Unreadable(format!("unsupported secure file protection {other}"))
        }
    }
}

/// Read a UTF-8 file that may be protected by this module.
pub fn read_string(path: &Path) -> io::Result<String> {
    let raw = std::fs::read_to_string(path)?;
    let Ok(file) = serde_json::from_str::<ProtectedFile>(&raw) else {
        return Ok(raw);
    };

    if file.format != FORMAT {
        return Ok(raw);
    }
    if file.version != VERSION {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("unsupported secure file version {}", file.version),
        ));
    }

    match file.protection.as_str() {
        WINDOWS_DPAPI_USER | WINDOWS_DPAPI_MACHINE => {
            let encrypted = base64::engine::general_purpose::STANDARD
                .decode(file.payload.as_bytes())
                .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
            let plain = unprotect(&encrypted)?;
            String::from_utf8(plain).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))
        }
        other => Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("unsupported secure file protection {other}"),
        )),
    }
}

/// Write a UTF-8 file, protecting it with Windows DPAPI when available.
pub fn write_string(path: &Path, contents: &str) -> io::Result<()> {
    let bytes = protected_file_bytes(contents)?;
    atomic_write(path, &bytes)
}

/// Replace a local state file atomically with private permissions.
///
/// The temporary file lives beside the destination so the final rename stays
/// on one filesystem. A crash can leave a harmless temp file, but it cannot
/// truncate the last known-good settings or credential file.
pub fn atomic_write(path: &Path, bytes: &[u8]) -> io::Result<()> {
    atomic_write_with_permissions(path, bytes, AtomicWritePermissions::Private)
}

/// Atomically replace a file owned by another application while retaining its
/// existing permission boundary.
pub(crate) fn atomic_write_preserving_permissions(path: &Path, bytes: &[u8]) -> io::Result<()> {
    atomic_write_with_permissions(path, bytes, AtomicWritePermissions::PreserveExisting)
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum AtomicWritePermissions {
    Private,
    PreserveExisting,
}

fn atomic_write_with_permissions(
    path: &Path,
    bytes: &[u8],
    permission_mode: AtomicWritePermissions,
) -> io::Result<()> {
    let parent = path.parent().ok_or_else(|| {
        io::Error::new(io::ErrorKind::InvalidInput, "state file path has no parent")
    })?;
    let file_name = path.file_name().ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            "state file path has no file name",
        )
    })?;

    let mut temp_path = None;
    let mut temp_file = None;
    for _ in 0..16 {
        let sequence = TEMP_FILE_COUNTER.fetch_add(1, Ordering::Relaxed);
        let mut temp_name = OsString::from(".");
        temp_name.push(file_name);
        temp_name.push(format!(".ceiling-tmp-{}-{sequence}", std::process::id()));
        let candidate = parent.join(temp_name);

        let mut options = std::fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        match options.open(&candidate) {
            Ok(file) => {
                temp_path = Some(candidate);
                temp_file = Some(file);
                break;
            }
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(error),
        }
    }

    let temp_path = temp_path.ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::AlreadyExists,
            "could not allocate a unique state-file temp path",
        )
    })?;
    let mut temp_file = temp_file.expect("temp path and file are assigned together");

    let existing_permissions = if permission_mode == AtomicWritePermissions::PreserveExisting {
        std::fs::metadata(path)
            .ok()
            .map(|metadata| metadata.permissions())
    } else {
        None
    };

    let result = (|| {
        prepare_temp_permissions(&temp_path, permission_mode, existing_permissions.as_ref())?;
        temp_file.write_all(bytes)?;
        temp_file.sync_all()?;
        drop(temp_file);
        atomic_replace(&temp_path, path, permission_mode)?;
        sync_parent_directory(parent)?;
        Ok(())
    })();

    if result.is_err() {
        let _ = std::fs::remove_file(&temp_path);
    }
    result
}

#[cfg(windows)]
fn atomic_replace(
    from: &Path,
    to: &Path,
    permission_mode: AtomicWritePermissions,
) -> io::Result<()> {
    use std::os::windows::ffi::OsStrExt;
    use windows::Win32::Storage::FileSystem::{
        MOVEFILE_REPLACE_EXISTING, MOVEFILE_WRITE_THROUGH, MoveFileExW, REPLACE_FILE_FLAGS,
        ReplaceFileW,
    };
    use windows::core::PCWSTR;

    let destination_exists = to.exists();
    let wide_from: Vec<u16> = from
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();
    let wide_to: Vec<u16> = to
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();
    unsafe {
        if permission_mode == AtomicWritePermissions::PreserveExisting && destination_exists {
            ReplaceFileW(
                PCWSTR(wide_to.as_ptr()),
                PCWSTR(wide_from.as_ptr()),
                PCWSTR::null(),
                REPLACE_FILE_FLAGS(0),
                None,
                None,
            )
            .map_err(|error| {
                io::Error::other(format!(
                    "metadata-preserving file replacement failed: {error}"
                ))
            })
        } else {
            MoveFileExW(
                PCWSTR(wide_from.as_ptr()),
                PCWSTR(wide_to.as_ptr()),
                MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
            )
            .map_err(|error| io::Error::other(format!("atomic file replacement failed: {error}")))
        }
    }
}

#[cfg(not(windows))]
fn atomic_replace(
    from: &Path,
    to: &Path,
    _permission_mode: AtomicWritePermissions,
) -> io::Result<()> {
    std::fs::rename(from, to)
}

#[cfg(windows)]
fn prepare_temp_permissions(
    path: &Path,
    _permission_mode: AtomicWritePermissions,
    _existing_permissions: Option<&std::fs::Permissions>,
) -> io::Result<()> {
    // ReplaceFileW transfers the destination's metadata to the replacement.
    // Keep the temporary secret current-user-only until that atomic swap.
    restrict_file_permissions(path)
}

#[cfg(not(windows))]
fn prepare_temp_permissions(
    path: &Path,
    permission_mode: AtomicWritePermissions,
    existing_permissions: Option<&std::fs::Permissions>,
) -> io::Result<()> {
    if permission_mode == AtomicWritePermissions::PreserveExisting
        && let Some(permissions) = existing_permissions
    {
        return std::fs::set_permissions(path, permissions.clone());
    }
    restrict_file_permissions(path)
}

#[cfg(unix)]
fn sync_parent_directory(parent: &Path) -> io::Result<()> {
    std::fs::File::open(parent)?.sync_all()
}

#[cfg(not(unix))]
fn sync_parent_directory(_parent: &Path) -> io::Result<()> {
    Ok(())
}

#[cfg(windows)]
fn protected_file_bytes(contents: &str) -> io::Result<Vec<u8>> {
    let (protection, encrypted) = protect(contents.as_bytes())?;
    let file = ProtectedFile {
        format: FORMAT.to_string(),
        version: VERSION,
        protection: protection.to_string(),
        payload: base64::engine::general_purpose::STANDARD.encode(encrypted),
    };
    serde_json::to_vec_pretty(&file).map_err(io::Error::other)
}

#[cfg(not(windows))]
fn protected_file_bytes(contents: &str) -> io::Result<Vec<u8>> {
    Ok(contents.as_bytes().to_vec())
}

#[cfg(windows)]
fn protect(plain: &[u8]) -> io::Result<(&'static str, Vec<u8>)> {
    use windows::Win32::Security::Cryptography::CRYPTPROTECT_UI_FORBIDDEN;

    protect_with_flags(plain, CRYPTPROTECT_UI_FORBIDDEN)
        .map(|encrypted| (WINDOWS_DPAPI_USER, encrypted))
        .map_err(|error| io::Error::other(format!("user-scoped DPAPI protection failed: {error}")))
}

#[cfg(windows)]
fn protect_with_flags(plain: &[u8], flags: u32) -> io::Result<Vec<u8>> {
    use windows::Win32::Foundation::{HLOCAL, LocalFree};
    use windows::Win32::Security::Cryptography::{CRYPT_INTEGER_BLOB, CryptProtectData};

    unsafe {
        let input_blob = CRYPT_INTEGER_BLOB {
            cbData: plain.len() as u32,
            pbData: plain.as_ptr() as *mut u8,
        };
        let mut output_blob = CRYPT_INTEGER_BLOB {
            cbData: 0,
            pbData: std::ptr::null_mut(),
        };

        CryptProtectData(&input_blob, None, None, None, None, flags, &mut output_blob)
            .map_err(|e| io::Error::other(format!("CryptProtectData failed: {e:?}")))?;

        if output_blob.pbData.is_null() {
            return Err(io::Error::other("CryptProtectData returned null output"));
        }

        let encrypted =
            std::slice::from_raw_parts(output_blob.pbData, output_blob.cbData as usize).to_vec();
        let _ = LocalFree(HLOCAL(output_blob.pbData as *mut _));
        Ok(encrypted)
    }
}

#[cfg(windows)]
fn unprotect(encrypted: &[u8]) -> io::Result<Vec<u8>> {
    use windows::Win32::Foundation::{HLOCAL, LocalFree};
    use windows::Win32::Security::Cryptography::{
        CRYPT_INTEGER_BLOB, CRYPTPROTECT_UI_FORBIDDEN, CryptUnprotectData,
    };

    unsafe {
        let input_blob = CRYPT_INTEGER_BLOB {
            cbData: encrypted.len() as u32,
            pbData: encrypted.as_ptr() as *mut u8,
        };
        let mut output_blob = CRYPT_INTEGER_BLOB {
            cbData: 0,
            pbData: std::ptr::null_mut(),
        };

        CryptUnprotectData(
            &input_blob,
            None,
            None,
            None,
            None,
            CRYPTPROTECT_UI_FORBIDDEN,
            &mut output_blob,
        )
        .map_err(|e| io::Error::other(format!("CryptUnprotectData failed: {e:?}")))?;

        if output_blob.pbData.is_null() {
            return Err(io::Error::other("CryptUnprotectData returned null output"));
        }

        let plain =
            std::slice::from_raw_parts(output_blob.pbData, output_blob.cbData as usize).to_vec();
        let _ = LocalFree(HLOCAL(output_blob.pbData as *mut _));
        Ok(plain)
    }
}

#[cfg(not(windows))]
fn unprotect(_encrypted: &[u8]) -> io::Result<Vec<u8>> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "Windows DPAPI-protected files can only be read on Windows by the same user",
    ))
}

#[cfg(unix)]
fn restrict_file_permissions(path: &Path) -> io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    let mut perms = std::fs::metadata(path)?.permissions();
    perms.set_mode(0o600);
    std::fs::set_permissions(path, perms)
}

#[cfg(windows)]
fn restrict_file_permissions(path: &Path) -> io::Result<()> {
    crate::windows_security::restrict_path_to_current_user(path)
}

#[cfg(not(any(unix, windows)))]
fn restrict_file_permissions(_path: &Path) -> io::Result<()> {
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_plaintext_json_without_wrapper() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("plain.json");
        std::fs::write(&path, r#"{"hello":"world"}"#).unwrap();

        assert_eq!(read_string(&path).unwrap(), r#"{"hello":"world"}"#);
    }

    #[test]
    fn write_roundtrips_on_this_platform() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("secure.json");
        write_string(&path, r#"{"secret":"value"}"#).unwrap();

        assert_eq!(read_string(&path).unwrap(), r#"{"secret":"value"}"#);
    }

    #[test]
    fn write_atomically_replaces_an_existing_file_without_temp_artifacts() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("secure.json");
        write_string(&path, r#"{"secret":"first"}"#).unwrap();
        write_string(&path, r#"{"secret":"second"}"#).unwrap();

        assert_eq!(read_string(&path).unwrap(), r#"{"secret":"second"}"#);
        let names = std::fs::read_dir(dir.path())
            .unwrap()
            .map(|entry| entry.unwrap().file_name())
            .collect::<Vec<_>>();
        assert_eq!(names, vec![OsString::from("secure.json")]);
    }

    #[test]
    fn failed_metadata_preserving_replacement_keeps_destination_and_cleans_temp() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("credentials.json");
        std::fs::create_dir(&path).unwrap();

        assert!(atomic_write_preserving_permissions(&path, b"replacement").is_err());
        assert!(path.is_dir());
        let temp_artifacts = std::fs::read_dir(dir.path())
            .unwrap()
            .filter_map(Result::ok)
            .filter(|entry| entry.file_name().to_string_lossy().contains("ceiling-tmp"))
            .count();
        assert_eq!(temp_artifacts, 0);
    }

    #[cfg(unix)]
    #[test]
    fn metadata_preserving_write_keeps_unix_file_mode() {
        use std::os::unix::fs::PermissionsExt;

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("credentials.json");
        std::fs::write(&path, b"original").unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o640)).unwrap();

        atomic_write_preserving_permissions(&path, b"replacement").unwrap();

        assert_eq!(
            std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o640
        );
    }

    #[cfg(windows)]
    fn windows_dacl(path: &Path) -> Vec<u8> {
        use std::mem::size_of;
        use std::os::windows::ffi::OsStrExt;

        use windows::Win32::Security::{
            DACL_SECURITY_INFORMATION, GetFileSecurityW, PSECURITY_DESCRIPTOR,
        };
        use windows::core::PCWSTR;

        let wide_path: Vec<u16> = path
            .as_os_str()
            .encode_wide()
            .chain(std::iter::once(0))
            .collect();
        let mut descriptor_bytes = 0u32;
        unsafe {
            let _ = GetFileSecurityW(
                PCWSTR(wide_path.as_ptr()),
                DACL_SECURITY_INFORMATION.0,
                PSECURITY_DESCRIPTOR(std::ptr::null_mut()),
                0,
                &mut descriptor_bytes,
            );
        }
        assert!(descriptor_bytes > 0);

        let descriptor_words = (descriptor_bytes as usize).div_ceil(size_of::<usize>());
        let mut descriptor = vec![0usize; descriptor_words];
        unsafe {
            GetFileSecurityW(
                PCWSTR(wide_path.as_ptr()),
                DACL_SECURITY_INFORMATION.0,
                PSECURITY_DESCRIPTOR(descriptor.as_mut_ptr().cast()),
                descriptor_bytes,
                &mut descriptor_bytes,
            )
            .ok()
            .unwrap();
        }
        unsafe {
            std::slice::from_raw_parts(descriptor.as_ptr().cast::<u8>(), descriptor_bytes as usize)
                .to_vec()
        }
    }

    #[cfg(windows)]
    #[test]
    fn metadata_preserving_write_keeps_windows_dacl() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("credentials.json");
        std::fs::write(&path, b"original").unwrap();
        let before = windows_dacl(&path);

        atomic_write_preserving_permissions(&path, b"replacement").unwrap();

        assert_eq!(windows_dacl(&path), before);
    }

    #[cfg(windows)]
    #[test]
    fn windows_write_uses_user_dpapi_and_a_protected_single_entry_dacl() {
        use std::mem::size_of;
        use std::os::windows::ffi::OsStrExt;

        use windows::Win32::Foundation::{BOOL, CloseHandle, HANDLE};
        use windows::Win32::Security::{
            ACCESS_ALLOWED_ACE, ACL, ACL_SIZE_INFORMATION, AclSizeInformation,
            DACL_SECURITY_INFORMATION, EqualSid, GetAce, GetAclInformation, GetFileSecurityW,
            GetSecurityDescriptorControl, GetSecurityDescriptorDacl, GetTokenInformation,
            PSECURITY_DESCRIPTOR, PSID, SE_DACL_PROTECTED, TOKEN_QUERY, TOKEN_USER, TokenUser,
        };
        use windows::Win32::Storage::FileSystem::FILE_ALL_ACCESS;
        use windows::Win32::System::SystemServices::ACCESS_ALLOWED_ACE_TYPE;
        use windows::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};
        use windows::core::PCWSTR;

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("secure.json");
        write_string(&path, r#"{"secret":"value"}"#).unwrap();

        let raw = std::fs::read_to_string(&path).unwrap();
        let protected: ProtectedFile = serde_json::from_str(&raw).unwrap();
        assert_eq!(protected.protection, WINDOWS_DPAPI_USER);

        let wide_path: Vec<u16> = path
            .as_os_str()
            .encode_wide()
            .chain(std::iter::once(0))
            .collect();
        let mut descriptor_bytes = 0u32;
        unsafe {
            let _ = GetFileSecurityW(
                PCWSTR(wide_path.as_ptr()),
                DACL_SECURITY_INFORMATION.0,
                PSECURITY_DESCRIPTOR(std::ptr::null_mut()),
                0,
                &mut descriptor_bytes,
            );
        }
        assert!(descriptor_bytes > 0);

        let descriptor_words = (descriptor_bytes as usize).div_ceil(size_of::<usize>());
        let mut descriptor_buffer = vec![0usize; descriptor_words];
        let descriptor = PSECURITY_DESCRIPTOR(descriptor_buffer.as_mut_ptr().cast());

        unsafe {
            GetFileSecurityW(
                PCWSTR(wide_path.as_ptr()),
                DACL_SECURITY_INFORMATION.0,
                descriptor,
                descriptor_bytes,
                &mut descriptor_bytes,
            )
            .ok()
            .unwrap();

            let mut control = 0u16;
            let mut revision = 0u32;
            GetSecurityDescriptorControl(descriptor, &mut control, &mut revision).unwrap();
            assert_ne!(control & SE_DACL_PROTECTED.0, 0);

            let mut present = BOOL::default();
            let mut defaulted = BOOL::default();
            let mut dacl: *mut ACL = std::ptr::null_mut();
            GetSecurityDescriptorDacl(descriptor, &mut present, &mut dacl, &mut defaulted).unwrap();
            assert!(present.as_bool());
            assert!(!dacl.is_null());
            assert!(!defaulted.as_bool());

            let mut info = ACL_SIZE_INFORMATION::default();
            GetAclInformation(
                dacl,
                (&mut info as *mut ACL_SIZE_INFORMATION).cast(),
                size_of::<ACL_SIZE_INFORMATION>() as u32,
                AclSizeInformation,
            )
            .unwrap();
            assert_eq!(info.AceCount, 1);

            let mut ace_ptr = std::ptr::null_mut();
            GetAce(dacl, 0, &mut ace_ptr).unwrap();
            let ace = &*ace_ptr.cast::<ACCESS_ALLOWED_ACE>();
            assert_eq!(u32::from(ace.Header.AceType), ACCESS_ALLOWED_ACE_TYPE);
            assert_eq!(ace.Mask, FILE_ALL_ACCESS.0);

            let mut token = HANDLE::default();
            OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token).unwrap();
            let mut token_bytes = 0u32;
            let _ = GetTokenInformation(token, TokenUser, None, 0, &mut token_bytes);
            assert!(token_bytes >= size_of::<TOKEN_USER>() as u32);
            let token_words = (token_bytes as usize).div_ceil(size_of::<usize>());
            let mut token_buffer = vec![0usize; token_words];
            GetTokenInformation(
                token,
                TokenUser,
                Some(token_buffer.as_mut_ptr().cast()),
                token_bytes,
                &mut token_bytes,
            )
            .unwrap();
            CloseHandle(token).unwrap();

            let token_user = &*token_buffer.as_ptr().cast::<TOKEN_USER>();
            let ace_sid = PSID((&ace.SidStart as *const u32).cast_mut().cast());
            EqualSid(ace_sid, token_user.User.Sid).unwrap();
        }
    }

    #[test]
    fn status_reports_missing_plaintext_and_protected_files() {
        let dir = tempfile::tempdir().unwrap();
        let missing = dir.path().join("missing.json");
        assert_eq!(status(&missing), SecureFileStatus::Missing);

        let plain = dir.path().join("plain.json");
        std::fs::write(&plain, r#"{"secret":"value"}"#).unwrap();
        assert_eq!(status(&plain), SecureFileStatus::Plaintext);

        let protected = dir.path().join("protected.json");
        std::fs::write(
            &protected,
            serde_json::to_string(&ProtectedFile {
                format: FORMAT.to_string(),
                version: VERSION,
                protection: WINDOWS_DPAPI_USER.to_string(),
                payload: "AA==".to_string(),
            })
            .unwrap(),
        )
        .unwrap();
        assert_eq!(
            status(&protected),
            SecureFileStatus::Protected(WINDOWS_DPAPI_USER.to_string())
        );
    }

    #[test]
    fn status_reports_unsupported_wrappers_as_unreadable() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("protected.json");
        std::fs::write(
            &path,
            serde_json::to_string(&ProtectedFile {
                format: FORMAT.to_string(),
                version: VERSION + 1,
                protection: WINDOWS_DPAPI_USER.to_string(),
                payload: "AA==".to_string(),
            })
            .unwrap(),
        )
        .unwrap();

        assert!(matches!(status(&path), SecureFileStatus::Unreadable(_)));
    }

    #[cfg(windows)]
    #[test]
    fn windows_write_uses_protected_wrapper() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("secure.json");
        write_string(&path, r#"{"secret":"value"}"#).unwrap();

        let raw = std::fs::read_to_string(&path).unwrap();
        let file: ProtectedFile = serde_json::from_str(&raw).unwrap();

        assert_eq!(file.format, FORMAT);
        assert_eq!(file.version, VERSION);
        assert!(matches!(
            file.protection.as_str(),
            WINDOWS_DPAPI_USER | WINDOWS_DPAPI_MACHINE
        ));
        assert!(
            !raw.contains("secret") && !raw.contains("value"),
            "protected Windows file must not contain plaintext JSON"
        );
    }
}
