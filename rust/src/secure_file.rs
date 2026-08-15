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
///
/// Where the lock cannot be enforced at all - a filesystem without `flock`, a
/// lock file this user can never open - the operation still runs, unserialized,
/// and a warning names the lock path. Blocking every write would be worse than
/// the interleaving risk, and the old lock protocol worked on those mounts.
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
    /// `None` once the lock was found to be unenforceable; see [`LockAttempt`].
    #[cfg(windows)]
    handle: Option<windows::Win32::Foundation::HANDLE>,
    /// Open lock file whose exclusive flock is released when this is dropped.
    /// `None` once the lock was found to be unenforceable; see [`LockAttempt`].
    #[cfg(not(windows))]
    _file: Option<std::fs::File>,
}

/// Outcome of one attempt to take the state-write lock.
enum LockAttempt {
    Acquired(StateWriteLock),
    /// Another live holder has the lock, so the attempt is worth repeating.
    Contended,
    /// The lock can never be taken through this path: the filesystem does not
    /// implement `flock`, or the lock file itself is unopenable (left by a
    /// privileged run, replaced by a directory, read-only mount). Retrying
    /// would only stall every write until the timeout and then fail it.
    Unenforceable(io::Error),
    /// Something unrelated to locking went wrong; the caller sees the error.
    Failed(io::Error),
}

impl StateWriteLock {
    fn acquire(path: &Path) -> io::Result<Self> {
        Self::acquire_with(path, Self::try_acquire)
    }

    fn acquire_with(
        path: &Path,
        mut attempt: impl FnMut(&Path) -> LockAttempt,
    ) -> io::Result<Self> {
        let deadline = std::time::Instant::now() + STATE_LOCK_TIMEOUT;
        loop {
            match attempt(path) {
                LockAttempt::Acquired(lock) => return Ok(lock),
                LockAttempt::Unenforceable(error) => {
                    // Degrade instead of blocking a legitimate write, but say
                    // so: this write is not serialized against other processes.
                    tracing::warn!(
                        lock_path = %path.display(),
                        %error,
                        "state write lock cannot be enforced here; writing without cross-process serialization"
                    );
                    return Ok(Self::unenforced());
                }
                LockAttempt::Failed(error) => return Err(error),
                LockAttempt::Contended => {
                    if std::time::Instant::now() >= deadline {
                        return Err(io::Error::new(
                            io::ErrorKind::WouldBlock,
                            "state store is locked",
                        ));
                    }
                    std::thread::sleep(STATE_LOCK_RETRY);
                }
            }
        }
    }

    /// A lock object that owns nothing, for filesystems and lock paths where no
    /// lock can be taken.
    fn unenforced() -> Self {
        Self {
            #[cfg(windows)]
            handle: None,
            #[cfg(not(windows))]
            _file: None,
        }
    }

    #[cfg(windows)]
    fn try_acquire(path: &Path) -> LockAttempt {
        use std::os::windows::ffi::OsStrExt;
        use windows::Win32::Foundation::{GENERIC_READ, GENERIC_WRITE};
        use windows::Win32::Storage::FileSystem::{
            CreateFileW, FILE_ATTRIBUTE_NORMAL, FILE_SHARE_MODE, OPEN_ALWAYS,
        };
        use windows::core::PCWSTR;

        let wide: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();
        match unsafe {
            CreateFileW(
                PCWSTR(wide.as_ptr()),
                GENERIC_READ.0 | GENERIC_WRITE.0,
                FILE_SHARE_MODE(0),
                None,
                OPEN_ALWAYS,
                FILE_ATTRIBUTE_NORMAL,
                None,
            )
        } {
            Ok(handle) => LockAttempt::Acquired(Self {
                handle: Some(handle),
            }),
            Err(error) => classify_open_failure(&error),
        }
    }

    #[cfg(not(windows))]
    fn try_acquire(path: &Path) -> LockAttempt {
        let mut options = std::fs::OpenOptions::new();
        options.read(true).write(true).create(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let file = match options.open(path) {
            Ok(file) => file,
            Err(error) => return classify_open_failure(&error),
        };
        match file.try_lock() {
            Ok(()) => LockAttempt::Acquired(Self { _file: Some(file) }),
            Err(error) => classify_lock_failure(error),
        }
    }
}

/// Classify a Win32 failure to open the lock file.
///
/// A sharing or lock violation is a live holder. `ERROR_ACCESS_DENIED` is not:
/// the lock file exists but this user can never open it, so treating it as
/// contention would stall every settings write for the full timeout and then
/// fail it.
#[cfg(windows)]
fn classify_open_failure(error: &windows::core::Error) -> LockAttempt {
    let io_error = || io::Error::other(format!("could not acquire state lock: {error}"));
    match win32_error_code(error) {
        WIN32_ERROR_SHARING_VIOLATION | WIN32_ERROR_LOCK_VIOLATION => LockAttempt::Contended,
        WIN32_ERROR_ACCESS_DENIED => LockAttempt::Unenforceable(io_error()),
        _ => LockAttempt::Failed(io_error()),
    }
}

/// Classify a failure to open the lock file.
///
/// Nothing here means "someone else holds the lock" - `open` does not block on
/// `flock`. A lock file this process can never open (left behind by a
/// privileged run, shadowed by a directory, on a read-only mount) would
/// otherwise stall every settings write for the full timeout and then fail it.
#[cfg(not(windows))]
fn classify_open_failure(error: &io::Error) -> LockAttempt {
    let unopenable = matches!(
        error.kind(),
        io::ErrorKind::PermissionDenied
            | io::ErrorKind::IsADirectory
            | io::ErrorKind::ReadOnlyFilesystem
    );
    let error = io::Error::new(
        error.kind(),
        format!("could not open the state lock file: {error}"),
    );
    if unopenable {
        LockAttempt::Unenforceable(error)
    } else {
        LockAttempt::Failed(error)
    }
}

/// Classify a `flock` failure on the opened lock file.
///
/// `WouldBlock` is the only answer that means another live holder has the lock.
/// A signal is worth another attempt. Every other errno says this filesystem
/// cannot enforce `flock` at all (NFS without lockd, some FUSE and SMB mounts),
/// and the old `create_new` protocol used to work there, so the write must not
/// be blocked by it.
#[cfg(not(windows))]
fn classify_lock_failure(error: std::fs::TryLockError) -> LockAttempt {
    match error {
        std::fs::TryLockError::WouldBlock => LockAttempt::Contended,
        std::fs::TryLockError::Error(error) if error.kind() == io::ErrorKind::Interrupted => {
            LockAttempt::Contended
        }
        std::fs::TryLockError::Error(error) => LockAttempt::Unenforceable(io::Error::new(
            error.kind(),
            format!("this filesystem cannot lock the state lock file: {error}"),
        )),
    }
}

impl Drop for StateWriteLock {
    fn drop(&mut self) {
        #[cfg(windows)]
        if let Some(handle) = self.handle.take() {
            unsafe {
                let _ = windows::Win32::Foundation::CloseHandle(handle);
            }
        }
        // Non-Windows: closing `_file` releases the flock. Leave the lock file
        // so a leftover after crash is not treated as a live holder, and so
        // unlinking cannot create a second lock inode while another holder
        // still has the original file open. A leftover this process cannot open
        // no longer blocks writes; see `classify_open_failure`.
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
    // Follow a symlinked credential path (dotfile managers, WSL shared targets)
    // so the atomic replace updates the target instead of replacing the link.
    let resolved = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    atomic_write_with_permissions(&resolved, bytes, AtomicWritePermissions::PreserveExisting)
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
        #[cfg(unix)]
        if permission_mode == AtomicWritePermissions::PreserveExisting {
            copy_unix_owner(path, &temp_path)?;
        }
        temp_file.write_all(bytes)?;
        temp_file.sync_all()?;
        drop(temp_file);
        atomic_replace(&temp_path, path, permission_mode)?;
        sync_parent_directory(parent)?;
        Ok(())
    })();

    if result.is_err() && !keep_replacement_temp(&result, path) {
        let _ = std::fs::remove_file(&temp_path);
    }
    result
}

/// `ERROR_UNABLE_TO_MOVE_REPLACEMENT` (1176). `ReplaceFileW` has already
/// removed the destination; the new bytes exist only at the replacement path.
#[cfg(any(windows, test))]
const WIN32_ERROR_UNABLE_TO_MOVE_REPLACEMENT: u32 = 0x498;
#[cfg(test)]
const WIN32_ERROR_UNABLE_TO_MOVE_REPLACEMENT_2: u32 = 0x499;
#[cfg(any(windows, test))]
const WIN32_ERROR_ACCESS_DENIED: u32 = 5;
#[cfg(any(windows, test))]
const WIN32_ERROR_SHARING_VIOLATION: u32 = 32;
#[cfg(any(windows, test))]
const WIN32_ERROR_LOCK_VIOLATION: u32 = 33;

/// Whether the temp file is the only remaining copy of the new bytes.
///
/// `ERROR_UNABLE_TO_MOVE_REPLACEMENT` means the destination is already gone,
/// so deleting the temp would destroy the credential file. Trust that error
/// code even if a later `exists()` check races. Any other failure that leaves
/// no destination also keeps the temp.
#[cfg(any(windows, test))]
fn keep_temp_after_replace_failure(win32_code: u32, destination_still_exists: bool) -> bool {
    win32_code == WIN32_ERROR_UNABLE_TO_MOVE_REPLACEMENT || !destination_still_exists
}

#[cfg(any(windows, test))]
fn is_sharing_or_access_failure(win32_code: u32) -> bool {
    matches!(
        win32_code,
        WIN32_ERROR_ACCESS_DENIED | WIN32_ERROR_SHARING_VIOLATION | WIN32_ERROR_LOCK_VIOLATION
    )
}

fn keep_replacement_temp(result: &io::Result<()>, dest: &Path) -> bool {
    let Err(error) = result else {
        return false;
    };
    #[cfg(windows)]
    {
        if let Some(raw) = error.raw_os_error() {
            return keep_temp_after_replace_failure(raw as u32, dest.exists());
        }
        !dest.exists()
    }
    #[cfg(not(windows))]
    {
        let _ = dest;
        false
    }
}

#[cfg(windows)]
fn win32_error_code(error: &windows::core::Error) -> u32 {
    (error.code().0 as u32) & 0xffff
}

/// Build an `io::Error` that still reports the Win32 code via `raw_os_error()`.
///
/// `Error::new` drops that code, which would make `keep_replacement_temp` miss
/// `ERROR_UNABLE_TO_MOVE_REPLACEMENT` if the dest name is recreated.
#[cfg(any(windows, test))]
fn io_error_from_win32_code(code: u32) -> io::Error {
    io::Error::from_raw_os_error(code as i32)
}

#[cfg(windows)]
fn win32_io_error(_context: &str, error: windows::core::Error) -> io::Error {
    io_error_from_win32_code(win32_error_code(&error))
}

#[cfg(windows)]
struct CapturedFileSecurity {
    descriptor: Vec<usize>,
    control: u16,
}

#[cfg(windows)]
fn capture_file_security(path: &Path) -> io::Result<CapturedFileSecurity> {
    use std::mem::size_of;
    use std::os::windows::ffi::OsStrExt;

    use windows::Win32::Security::{
        DACL_SECURITY_INFORMATION, GROUP_SECURITY_INFORMATION, GetFileSecurityW,
        GetSecurityDescriptorControl, OWNER_SECURITY_INFORMATION, PSECURITY_DESCRIPTOR,
    };
    use windows::core::PCWSTR;

    let wide: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();
    let requested =
        OWNER_SECURITY_INFORMATION.0 | GROUP_SECURITY_INFORMATION.0 | DACL_SECURITY_INFORMATION.0;
    let mut needed = 0u32;
    unsafe {
        let _ = GetFileSecurityW(
            PCWSTR(wide.as_ptr()),
            requested,
            PSECURITY_DESCRIPTOR(std::ptr::null_mut()),
            0,
            &mut needed,
        );
    }
    if needed == 0 {
        return Err(io::Error::other(
            "GetFileSecurityW returned an empty descriptor",
        ));
    }

    let words = (needed as usize).div_ceil(size_of::<usize>());
    let mut descriptor = vec![0usize; words];
    unsafe {
        GetFileSecurityW(
            PCWSTR(wide.as_ptr()),
            requested,
            PSECURITY_DESCRIPTOR(descriptor.as_mut_ptr().cast()),
            needed,
            &mut needed,
        )
        .ok()
        .map_err(|error| io::Error::other(format!("GetFileSecurityW failed: {error}")))?;

        let mut control = 0u16;
        let mut revision = 0u32;
        GetSecurityDescriptorControl(
            PSECURITY_DESCRIPTOR(descriptor.as_mut_ptr().cast()),
            &mut control,
            &mut revision,
        )
        .map_err(|error| {
            io::Error::other(format!("GetSecurityDescriptorControl failed: {error}"))
        })?;
        Ok(CapturedFileSecurity {
            descriptor,
            control,
        })
    }
}

#[cfg(windows)]
fn captured_descriptor(
    captured: &CapturedFileSecurity,
) -> windows::Win32::Security::PSECURITY_DESCRIPTOR {
    windows::Win32::Security::PSECURITY_DESCRIPTOR(captured.descriptor.as_ptr() as *mut _)
}

#[cfg(windows)]
fn dacl_security_info(
    captured: &CapturedFileSecurity,
) -> windows::Win32::Security::OBJECT_SECURITY_INFORMATION {
    use windows::Win32::Security::{
        DACL_SECURITY_INFORMATION, OBJECT_SECURITY_INFORMATION,
        PROTECTED_DACL_SECURITY_INFORMATION, SE_DACL_PROTECTED,
        UNPROTECTED_DACL_SECURITY_INFORMATION,
    };

    let mut info = DACL_SECURITY_INFORMATION.0;
    if captured.control & SE_DACL_PROTECTED.0 != 0 {
        info |= PROTECTED_DACL_SECURITY_INFORMATION.0;
    } else {
        info |= UNPROTECTED_DACL_SECURITY_INFORMATION.0;
    }
    OBJECT_SECURITY_INFORMATION(info)
}

#[cfg(windows)]
fn apply_captured_dacl(path: &Path, captured: &CapturedFileSecurity) -> io::Result<()> {
    use std::os::windows::ffi::OsStrExt;

    use windows::Win32::Foundation::BOOL;
    use windows::Win32::Security::Authorization::{SE_FILE_OBJECT, SetNamedSecurityInfoW};
    use windows::Win32::Security::{ACL, GetSecurityDescriptorDacl, PSID};
    use windows::core::PCWSTR;

    let wide: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();
    let descriptor = captured_descriptor(captured);
    let mut present = BOOL::default();
    let mut defaulted = BOOL::default();
    let mut dacl: *mut ACL = std::ptr::null_mut();
    unsafe {
        GetSecurityDescriptorDacl(descriptor, &mut present, &mut dacl, &mut defaulted).map_err(
            |error| io::Error::other(format!("GetSecurityDescriptorDacl failed: {error}")),
        )?;
        if !present.as_bool() {
            return Ok(());
        }
        let status = SetNamedSecurityInfoW(
            PCWSTR(wide.as_ptr()),
            SE_FILE_OBJECT,
            dacl_security_info(captured),
            PSID::default(),
            PSID::default(),
            (!dacl.is_null()).then_some(dacl.cast_const()),
            None,
        );
        if status.is_err() {
            return Err(io::Error::other(format!(
                "SetNamedSecurityInfoW DACL failed: {status:?}"
            )));
        }
    }
    Ok(())
}

#[cfg(windows)]
fn apply_captured_owner(path: &Path, captured: &CapturedFileSecurity) -> io::Result<()> {
    use std::mem::ManuallyDrop;
    use std::os::windows::ffi::OsStrExt;

    use windows::Win32::Foundation::BOOL;
    use windows::Win32::Security::Authorization::{SE_FILE_OBJECT, SetNamedSecurityInfoW};
    use windows::Win32::Security::{
        GetSecurityDescriptorOwner, OBJECT_SECURITY_INFORMATION, OWNER_SECURITY_INFORMATION, PSID,
    };
    use windows::core::PCWSTR;

    let wide: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();
    let descriptor = captured_descriptor(captured);
    let mut owner = ManuallyDrop::new(PSID::default());
    let mut defaulted = BOOL::default();
    unsafe {
        GetSecurityDescriptorOwner(descriptor, &mut *owner, &mut defaulted).map_err(|error| {
            io::Error::other(format!("GetSecurityDescriptorOwner failed: {error}"))
        })?;
        if owner.is_invalid() {
            return Ok(());
        }
        let status = SetNamedSecurityInfoW(
            PCWSTR(wide.as_ptr()),
            SE_FILE_OBJECT,
            OBJECT_SECURITY_INFORMATION(OWNER_SECURITY_INFORMATION.0),
            *owner,
            PSID::default(),
            None,
            None,
        );
        if status.is_err() {
            return Err(io::Error::other(format!(
                "SetNamedSecurityInfoW owner failed: {status:?}"
            )));
        }
    }
    Ok(())
}

#[cfg(any(windows, test))]
fn finish_preserved_security_restore(
    dacl: io::Result<()>,
    owner: io::Result<()>,
) -> io::Result<()> {
    // WRITE_OWNER / SE_RESTORE is not guaranteed. Keep the replaced bytes
    // and the restored DACL if only the owner rewrite is denied.
    let _ = owner;
    dacl
}

#[cfg(windows)]
fn apply_file_security(path: &Path, captured: &CapturedFileSecurity) -> io::Result<()> {
    finish_preserved_security_restore(
        apply_captured_dacl(path, captured),
        apply_captured_owner(path, captured),
    )
}

#[cfg(windows)]
fn restore_captured_security(
    path: &Path,
    captured: Option<&CapturedFileSecurity>,
) -> io::Result<()> {
    match captured {
        Some(security) => apply_file_security(path, security),
        None => Ok(()),
    }
}

#[cfg(windows)]
fn move_file_replace(from: &Path, to: &Path) -> Result<(), windows::core::Error> {
    use std::os::windows::ffi::OsStrExt;
    use windows::Win32::Storage::FileSystem::{
        MOVEFILE_REPLACE_EXISTING, MOVEFILE_WRITE_THROUGH, MoveFileExW,
    };
    use windows::core::PCWSTR;

    let wide_from: Vec<u16> = from.as_os_str().encode_wide().chain(Some(0)).collect();
    let wide_to: Vec<u16> = to.as_os_str().encode_wide().chain(Some(0)).collect();
    unsafe {
        MoveFileExW(
            PCWSTR(wide_from.as_ptr()),
            PCWSTR(wide_to.as_ptr()),
            MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
        )
    }
}

#[cfg(windows)]
fn retry_move_file_replace(from: &Path, to: &Path) -> Result<(), windows::core::Error> {
    const ATTEMPTS: u32 = 5;
    const RETRY: std::time::Duration = std::time::Duration::from_millis(20);
    let mut last = None;
    for attempt in 0..ATTEMPTS {
        match move_file_replace(from, to) {
            Ok(()) => return Ok(()),
            Err(error)
                if is_sharing_or_access_failure(win32_error_code(&error))
                    && attempt + 1 < ATTEMPTS =>
            {
                last = Some(error);
                std::thread::sleep(RETRY);
            }
            Err(error) => return Err(error),
        }
    }
    Err(last.expect("sharing retry records the last error"))
}

#[cfg(windows)]
fn sharing_replace_error(error: windows::core::Error) -> io::Error {
    let os = io_error_from_win32_code(win32_error_code(&error));
    io::Error::new(
        os.kind(),
        format!(
            "could not replace the file because another process has it open without delete sharing: {os}"
        ),
    )
}

#[cfg(windows)]
fn atomic_replace(
    from: &Path,
    to: &Path,
    permission_mode: AtomicWritePermissions,
) -> io::Result<()> {
    use std::os::windows::ffi::OsStrExt;
    use windows::Win32::Storage::FileSystem::{REPLACE_FILE_FLAGS, ReplaceFileW};
    use windows::core::PCWSTR;

    let destination_exists = to.exists();
    if permission_mode != AtomicWritePermissions::PreserveExisting || !destination_exists {
        return move_file_replace(from, to)
            .map_err(|error| win32_io_error("atomic file replacement failed", error));
    }

    let captured = capture_file_security(to).ok();
    let wide_from: Vec<u16> = from.as_os_str().encode_wide().chain(Some(0)).collect();
    let wide_to: Vec<u16> = to.as_os_str().encode_wide().chain(Some(0)).collect();
    let replace_result = unsafe {
        ReplaceFileW(
            PCWSTR(wide_to.as_ptr()),
            PCWSTR(wide_from.as_ptr()),
            PCWSTR::null(),
            REPLACE_FILE_FLAGS(0),
            None,
            None,
        )
    };
    match replace_result {
        Ok(()) => restore_captured_security(to, captured.as_ref()),
        Err(error) if win32_error_code(&error) == WIN32_ERROR_UNABLE_TO_MOVE_REPLACEMENT => {
            // Destination is already gone; new bytes live only at `from`.
            match retry_move_file_replace(from, to) {
                Ok(()) => restore_captured_security(to, captured.as_ref()),
                Err(_move_error) => {
                    // Keep the original 0x498 so cleanup will not delete the
                    // temp even if something recreates the dest name, and log
                    // the temp path so the remaining copy of the tokens stays
                    // recoverable.
                    let code = win32_error_code(&error);
                    tracing::warn!(
                        temp_path = %from.display(),
                        win32_code = code,
                        "metadata-preserving replacement removed the destination and the recovery move failed"
                    );
                    Err(io_error_from_win32_code(code))
                }
            }
        }
        Err(error) if is_sharing_or_access_failure(win32_error_code(&error)) => {
            match retry_move_file_replace(from, to) {
                Ok(()) => restore_captured_security(to, captured.as_ref()),
                Err(move_error) => Err(sharing_replace_error(move_error)),
            }
        }
        Err(error) => Err(win32_io_error(
            "metadata-preserving file replacement failed",
            error,
        )),
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
    // Keep the temporary secret current-user-only until the swap. Destination
    // owner/DACL are copied back onto the replaced file afterwards.
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
fn copy_unix_owner(from: &Path, to: &Path) -> io::Result<()> {
    use std::os::unix::fs::{MetadataExt, chown};

    let metadata = match std::fs::metadata(from) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error),
    };
    chown(to, Some(metadata.uid()), Some(metadata.gid()))
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
        use std::os::unix::fs::{MetadataExt, PermissionsExt, chown};

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("credentials.json");
        std::fs::write(&path, b"original").unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o640)).unwrap();
        let before = std::fs::metadata(&path).unwrap();
        chown(&path, Some(before.uid()), Some(before.gid())).unwrap();

        atomic_write_preserving_permissions(&path, b"replacement").unwrap();

        let after = std::fs::metadata(&path).unwrap();
        assert_eq!(after.permissions().mode() & 0o777, 0o640);
        assert_eq!(after.uid(), before.uid());
        assert_eq!(after.gid(), before.gid());
    }

    #[cfg(unix)]
    #[test]
    fn metadata_preserving_write_follows_a_symlinked_credential_path() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("real.json");
        std::fs::write(&target, b"original").unwrap();
        let link = dir.path().join("link.json");
        std::os::unix::fs::symlink(&target, &link).unwrap();

        atomic_write_preserving_permissions(&link, b"replacement").unwrap();

        assert!(
            std::fs::symlink_metadata(&link)
                .unwrap()
                .file_type()
                .is_symlink()
        );
        assert_eq!(std::fs::read(&target).unwrap(), b"replacement");
    }

    #[test]
    fn keep_temp_when_replace_already_removed_destination() {
        assert!(keep_temp_after_replace_failure(
            WIN32_ERROR_UNABLE_TO_MOVE_REPLACEMENT,
            false
        ));
        assert!(keep_temp_after_replace_failure(
            WIN32_ERROR_UNABLE_TO_MOVE_REPLACEMENT,
            true
        ));
        assert!(!keep_temp_after_replace_failure(
            WIN32_ERROR_UNABLE_TO_MOVE_REPLACEMENT_2,
            true
        ));
        assert!(keep_temp_after_replace_failure(
            WIN32_ERROR_UNABLE_TO_MOVE_REPLACEMENT_2,
            false
        ));
        assert!(!keep_temp_after_replace_failure(
            WIN32_ERROR_SHARING_VIOLATION,
            true
        ));
        assert!(keep_temp_after_replace_failure(
            WIN32_ERROR_SHARING_VIOLATION,
            false
        ));
        assert!(is_sharing_or_access_failure(WIN32_ERROR_ACCESS_DENIED));
        assert!(is_sharing_or_access_failure(WIN32_ERROR_SHARING_VIOLATION));
        assert!(is_sharing_or_access_failure(WIN32_ERROR_LOCK_VIOLATION));
        assert!(!is_sharing_or_access_failure(
            WIN32_ERROR_UNABLE_TO_MOVE_REPLACEMENT
        ));
    }

    #[test]
    fn win32_replace_error_keeps_unable_to_move_replacement_os_code() {
        let error = io_error_from_win32_code(WIN32_ERROR_UNABLE_TO_MOVE_REPLACEMENT);
        assert_eq!(
            error.raw_os_error(),
            Some(WIN32_ERROR_UNABLE_TO_MOVE_REPLACEMENT as i32)
        );

        #[cfg(windows)]
        {
            let dir = tempfile::tempdir().unwrap();
            let dest = dir.path().join("credentials.json");
            std::fs::write(&dest, b"reappeared").unwrap();
            let result: io::Result<()> = Err(error);
            assert!(
                keep_replacement_temp(&result, &dest),
                "0x498 must keep the temp even if the dest name was recreated"
            );
        }
    }

    #[cfg(windows)]
    fn current_user_sid() -> (Vec<usize>, windows::Win32::Security::PSID) {
        use std::mem::size_of;

        use windows::Win32::Foundation::{CloseHandle, HANDLE};
        use windows::Win32::Security::{GetTokenInformation, TOKEN_QUERY, TOKEN_USER, TokenUser};
        use windows::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};

        unsafe {
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
            let sid = token_user.User.Sid;
            (token_buffer, sid)
        }
    }

    #[cfg(windows)]
    fn dacl_has_noninherited_user_full_control(path: &Path) -> bool {
        use std::mem::size_of;
        use std::os::windows::ffi::OsStrExt;

        use windows::Win32::Foundation::BOOL;
        use windows::Win32::Security::{
            ACCESS_ALLOWED_ACE, ACL, ACL_SIZE_INFORMATION, AclSizeInformation,
            DACL_SECURITY_INFORMATION, EqualSid, GetAce, GetAclInformation, GetFileSecurityW,
            GetSecurityDescriptorControl, GetSecurityDescriptorDacl, INHERITED_ACE,
            PSECURITY_DESCRIPTOR, PSID, SE_DACL_PROTECTED,
        };
        use windows::Win32::Storage::FileSystem::FILE_ALL_ACCESS;
        use windows::Win32::System::SystemServices::ACCESS_ALLOWED_ACE_TYPE;
        use windows::core::PCWSTR;

        let (_token_buffer, user_sid) = current_user_sid();
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
            if control & SE_DACL_PROTECTED.0 == 0 {
                return false;
            }

            let mut present = BOOL::default();
            let mut defaulted = BOOL::default();
            let mut dacl: *mut ACL = std::ptr::null_mut();
            GetSecurityDescriptorDacl(descriptor, &mut present, &mut dacl, &mut defaulted).unwrap();
            if !present.as_bool() || dacl.is_null() {
                return false;
            }

            let mut info = ACL_SIZE_INFORMATION::default();
            GetAclInformation(
                dacl,
                (&mut info as *mut ACL_SIZE_INFORMATION).cast(),
                size_of::<ACL_SIZE_INFORMATION>() as u32,
                AclSizeInformation,
            )
            .unwrap();

            for index in 0..info.AceCount {
                let mut ace_ptr = std::ptr::null_mut();
                GetAce(dacl, index, &mut ace_ptr).unwrap();
                let ace = &*ace_ptr.cast::<ACCESS_ALLOWED_ACE>();
                if u32::from(ace.Header.AceType) != ACCESS_ALLOWED_ACE_TYPE {
                    continue;
                }
                if u32::from(ace.Header.AceFlags) & INHERITED_ACE.0 != 0 {
                    continue;
                }
                if ace.Mask != FILE_ALL_ACCESS.0 {
                    continue;
                }
                let ace_sid = PSID((&ace.SidStart as *const u32).cast_mut().cast());
                if EqualSid(ace_sid, user_sid).is_ok() {
                    return true;
                }
            }
            false
        }
    }

    #[cfg(windows)]
    #[test]
    fn metadata_preserving_write_keeps_windows_dacl() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("credentials.json");
        std::fs::write(&path, b"original").unwrap();
        crate::windows_security::restrict_path_to_current_user(&path).unwrap();
        assert!(dacl_has_noninherited_user_full_control(&path));

        atomic_write_preserving_permissions(&path, b"replacement").unwrap();

        assert_eq!(std::fs::read(&path).unwrap(), b"replacement");
        assert!(
            dacl_has_noninherited_user_full_control(&path),
            "destination DACL lost the explicit current-user ACE"
        );
    }

    #[test]
    fn metadata_preserving_write_keeps_dacl_when_owner_cannot_be_rewritten() {
        let dacl_ok: io::Result<()> = Ok(());
        let owner_denied = Err(io::Error::from_raw_os_error(
            WIN32_ERROR_ACCESS_DENIED as i32,
        ));
        finish_preserved_security_restore(dacl_ok, owner_denied)
            .expect("owner restore denial must not fail a successful DACL restore");

        let dacl_err = Err(io::Error::other("SetNamedSecurityInfoW DACL failed"));
        assert!(
            finish_preserved_security_restore(dacl_err, Ok(())).is_err(),
            "a failed DACL restore must still fail the persist"
        );
    }

    #[cfg(windows)]
    #[test]
    fn metadata_preserving_write_errors_when_destination_has_no_delete_share() {
        use std::os::windows::ffi::OsStrExt;

        use windows::Win32::Foundation::{CloseHandle, GENERIC_READ};
        use windows::Win32::Storage::FileSystem::{
            CreateFileW, FILE_ATTRIBUTE_NORMAL, FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_EXISTING,
        };
        use windows::core::PCWSTR;

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("credentials.json");
        std::fs::write(&path, b"original").unwrap();

        let wide: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();
        let handle = unsafe {
            CreateFileW(
                PCWSTR(wide.as_ptr()),
                GENERIC_READ.0,
                FILE_SHARE_READ | FILE_SHARE_WRITE,
                None,
                OPEN_EXISTING,
                FILE_ATTRIBUTE_NORMAL,
                None,
            )
        }
        .expect("open dest without delete sharing");

        let error = atomic_write_preserving_permissions(&path, b"replacement").unwrap_err();
        unsafe {
            CloseHandle(handle).ok();
        }

        assert_eq!(std::fs::read(&path).unwrap(), b"original");
        let message = error.to_string();
        assert!(
            message.contains("delete sharing"),
            "expected a clear sharing error, got {message}"
        );
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

    #[cfg(unix)]
    fn take_lock(path: &Path) -> StateWriteLock {
        match StateWriteLock::try_acquire(path) {
            LockAttempt::Acquired(lock) => lock,
            _ => panic!("the lock must be free"),
        }
    }

    #[cfg(unix)]
    #[test]
    fn state_write_lock_try_acquire_reports_contention_while_held() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("state-write.lock");

        let held = take_lock(&path);
        assert!(
            matches!(StateWriteLock::try_acquire(&path), LockAttempt::Contended),
            "second acquire must report contention while the first holder is alive"
        );
        assert!(path.exists(), "lock file stays while a holder is alive");

        drop(held);
        assert!(
            matches!(StateWriteLock::try_acquire(&path), LockAttempt::Acquired(_)),
            "a dropped holder must release the flock for the next acquirer"
        );
    }

    #[cfg(unix)]
    #[test]
    fn state_write_lock_recovers_stale_lock_file_after_crash() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("state-write.lock");
        std::fs::write(&path, b"leftover after SIGKILL").unwrap();

        let started = std::time::Instant::now();
        let mut ran = false;
        with_state_write_lock_at(&path, || {
            ran = true;
            Ok(())
        })
        .unwrap();

        assert!(ran);
        assert!(
            started.elapsed() < std::time::Duration::from_secs(1),
            "a leftover lock file with no live holder must not wait out the acquire timeout"
        );
    }

    #[cfg(unix)]
    #[test]
    fn state_write_lock_retries_until_the_live_holder_releases() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("state-write.lock");
        let (ready_tx, ready_rx) = std::sync::mpsc::channel();
        let (done_tx, done_rx) = std::sync::mpsc::channel();

        let held = take_lock(&path);
        let waiter_path = path.clone();
        std::thread::spawn(move || {
            ready_tx.send(()).unwrap();
            let result = with_state_write_lock_at(&waiter_path, || Ok(42));
            let _ = done_tx.send(result);
        });
        ready_rx.recv().unwrap();

        assert!(
            done_rx
                .recv_timeout(std::time::Duration::from_millis(80))
                .is_err(),
            "a live holder must not have its lock stolen"
        );

        drop(held);
        let result = done_rx
            .recv_timeout(std::time::Duration::from_secs(2))
            .expect("waiter should acquire after the holder drops");
        assert_eq!(result.unwrap(), 42);
    }

    #[test]
    fn an_unenforceable_lock_lets_the_write_through_once() {
        let mut attempts = 0;
        let started = std::time::Instant::now();
        let lock = StateWriteLock::acquire_with(Path::new("state-write.lock"), |_| {
            attempts += 1;
            LockAttempt::Unenforceable(io::Error::from(io::ErrorKind::Unsupported))
        })
        .expect("a lock that cannot be enforced must not block a legitimate write");

        drop(lock);
        assert_eq!(attempts, 1, "an unenforceable lock must not be retried");
        assert!(started.elapsed() < std::time::Duration::from_secs(1));
    }

    #[test]
    fn a_contended_lock_is_retried_and_a_failed_attempt_is_reported() {
        let mut attempts = 0;
        StateWriteLock::acquire_with(Path::new("state-write.lock"), |_| {
            attempts += 1;
            if attempts < 3 {
                LockAttempt::Contended
            } else {
                LockAttempt::Acquired(StateWriteLock::unenforced())
            }
        })
        .expect("contention must be retried until the holder releases");
        assert_eq!(attempts, 3);

        let failure = StateWriteLock::acquire_with(Path::new("state-write.lock"), |_| {
            LockAttempt::Failed(io::Error::from(io::ErrorKind::NotFound))
        });
        match failure {
            Ok(_) => panic!("an unrelated failure must reach the caller"),
            Err(error) => assert_eq!(error.kind(), io::ErrorKind::NotFound),
        }
    }

    #[cfg(unix)]
    #[test]
    fn a_filesystem_without_flock_support_degrades_instead_of_failing() {
        assert!(
            matches!(
                classify_lock_failure(std::fs::TryLockError::WouldBlock),
                LockAttempt::Contended
            ),
            "a live holder must still be waited for"
        );
        assert!(
            matches!(
                classify_lock_failure(std::fs::TryLockError::Error(io::Error::from(
                    io::ErrorKind::Interrupted
                ))),
                LockAttempt::Contended
            ),
            "a signal must be retried, not treated as a broken filesystem"
        );

        // ENOTSUP / ENOLCK from NFS, SMB or FUSE mounts.
        match classify_lock_failure(std::fs::TryLockError::Error(io::Error::from(
            io::ErrorKind::Unsupported,
        ))) {
            LockAttempt::Unenforceable(error) => assert!(
                error.to_string().contains("cannot lock"),
                "the degraded path must name the reason, got {error}"
            ),
            _ => panic!("an flock-less filesystem must degrade, not fail the write"),
        }
    }

    #[test]
    fn a_lock_path_that_cannot_be_opened_does_not_block_the_write() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("state-write.lock");
        // A directory in the lock file's place can never be opened as a lock
        // file, the same dead end as a leftover owned by another user.
        std::fs::create_dir(&path).unwrap();

        let started = std::time::Instant::now();
        let mut ran = false;
        with_state_write_lock_at(&path, || {
            ran = true;
            Ok(())
        })
        .expect("an unopenable lock path must not fail a legitimate write");

        assert!(ran);
        assert!(
            started.elapsed() < std::time::Duration::from_secs(1),
            "an unopenable lock path must not wait out the acquire timeout"
        );
    }

    #[cfg(unix)]
    #[test]
    fn an_unopenable_leftover_lock_file_does_not_block_the_write() {
        use std::os::unix::fs::PermissionsExt;

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("state-write.lock");
        std::fs::write(&path, b"leftover from a privileged run").unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o000)).unwrap();
        if std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(&path)
            .is_ok()
        {
            // Running as root, where no file is unopenable.
            return;
        }

        let started = std::time::Instant::now();
        let mut ran = false;
        with_state_write_lock_at(&path, || {
            ran = true;
            Ok(())
        })
        .expect("a leftover lock file this user cannot open must not fail the write");

        assert!(ran);
        assert!(
            started.elapsed() < std::time::Duration::from_secs(1),
            "an unopenable leftover must not wait out the acquire timeout"
        );
    }
}
