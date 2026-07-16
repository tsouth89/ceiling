//! Small helper for storing local secret-bearing JSON files.

use std::io;
use std::path::Path;

use base64::Engine;
use serde::{Deserialize, Serialize};

const FORMAT: &str = "codexbar.secure-file";
const VERSION: u32 = 1;
const WINDOWS_DPAPI_USER: &str = "windows-dpapi-user";
const WINDOWS_DPAPI_MACHINE: &str = "windows-dpapi-machine";

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
    std::fs::write(path, bytes)?;
    restrict_file_permissions(path)?;
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
