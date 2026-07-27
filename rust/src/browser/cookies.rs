//! Cookie extraction for Windows browsers
//!
//! Chromium browsers store cookies in an SQLite database encrypted with DPAPI.
//! Firefox stores cookies in an unencrypted SQLite database.

#![allow(dead_code)]

use std::path::Path;

use aes_gcm::{
    Aes256Gcm, Nonce,
    aead::{Aead, KeyInit},
};
use base64::Engine;
use rusqlite::Connection;
use thiserror::Error;

use super::detection::{BrowserProfile, DetectedBrowser};

/// Errors that can occur during cookie extraction
#[derive(Debug, Error)]
pub enum CookieError {
    #[error("Database error: {0}")]
    Database(#[from] rusqlite::Error),

    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    #[error("Decryption error: {0}")]
    Decryption(String),

    #[error("No encryption key found")]
    NoEncryptionKey,

    #[error("Cookie not found for domain: {0}")]
    NotFound(String),

    #[error("Browser not installed")]
    BrowserNotInstalled,

    #[error("DPAPI error: {0}")]
    Dpapi(String),

    /// Chromium App-Bound Encryption (ABE) is protecting cookie values.
    /// The user-level DPAPI key in Local State can no longer decrypt cookies encrypted
    /// after the ABE migration.  Modern Chrome and Edge write these cookies with a
    /// `v20` prefix; older migrated profiles can also fail every AES-GCM decrypt while
    /// exposing `app_bound_encrypted_key` in Local State.
    #[error(
        "Chrome/Edge/Brave App-Bound Encryption is blocking automatic browser import. \
             Sign in with Firefox (automatic works there), or paste the Cookie header \
             from DevTools → Network → any request → Request Headers → Cookie."
    )]
    AppBoundEncryption,

    /// Aggregated multi-browser scan failure with actionable detail.
    #[error("{0}")]
    ScanFailed(String),
}

/// A browser cookie
#[derive(Debug, Clone)]
pub struct Cookie {
    pub name: String,
    pub value: String,
    pub domain: String,
    pub path: String,
    pub expires: Option<i64>,
    pub is_secure: bool,
    pub is_http_only: bool,
}

impl Cookie {
    /// Format as a cookie header value
    pub fn to_header_value(&self) -> String {
        format!("{}={}", self.name, self.value)
    }
}

/// A private, short-lived copy of a browser cookie database.
///
/// The guard removes the copy on every return path, including SQLite query
/// failures. On Windows the destination ACL is restricted before any cookie
/// bytes are written.
struct TemporaryCookieDatabase {
    path: std::path::PathBuf,
}

impl TemporaryCookieDatabase {
    fn create(source_path: &Path) -> Result<Self, CookieError> {
        let file_name = source_path
            .file_name()
            .unwrap_or_default()
            .to_string_lossy();
        let path =
            std::env::temp_dir().join(format!("ceiling_{}_{}", uuid::Uuid::new_v4(), file_name));

        let mut destination_options = std::fs::OpenOptions::new();
        destination_options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            destination_options.mode(0o600);
        }

        let create_result = (|| -> Result<(), CookieError> {
            let mut destination = destination_options.open(&path)?;
            #[cfg(windows)]
            crate::windows_security::restrict_path_to_current_user(&path)?;

            copy_file_shared(source_path, &mut destination)?;
            destination.sync_all()?;
            drop(destination);

            // Chromium/Firefox often use WAL journaling. Copying only the main
            // file can yield an empty or stale snapshot while the browser is
            // running. Sidecars share the same base name with -wal / -shm.
            for suffix in ["-wal", "-shm"] {
                let sidecar = {
                    let mut s = source_path.as_os_str().to_owned();
                    s.push(suffix);
                    std::path::PathBuf::from(s)
                };
                if !sidecar.is_file() {
                    continue;
                }
                let dest_sidecar = {
                    let mut s = path.as_os_str().to_owned();
                    s.push(suffix);
                    std::path::PathBuf::from(s)
                };
                let mut options = std::fs::OpenOptions::new();
                options.write(true).create(true).truncate(true);
                #[cfg(unix)]
                {
                    use std::os::unix::fs::OpenOptionsExt;
                    options.mode(0o600);
                }
                match options.open(&dest_sidecar) {
                    Ok(mut out) => {
                        #[cfg(windows)]
                        crate::windows_security::restrict_path_to_current_user(&dest_sidecar)?;
                        if let Err(error) = copy_file_shared(&sidecar, &mut out) {
                            tracing::debug!(
                                path = %sidecar.display(),
                                %error,
                                "cookie DB sidecar copy failed; continuing with main file"
                            );
                            let _ = std::fs::remove_file(&dest_sidecar);
                        } else {
                            let _ = out.sync_all();
                        }
                    }
                    Err(error) => {
                        tracing::debug!(
                            path = %sidecar.display(),
                            %error,
                            "could not create cookie DB sidecar copy"
                        );
                    }
                }
            }
            Ok(())
        })();

        if let Err(error) = create_result {
            cleanup_temp_cookie_files(&path);
            return Err(error);
        }
        Ok(Self { path })
    }

    fn path(&self) -> &Path {
        &self.path
    }
}

fn cleanup_temp_cookie_files(path: &Path) {
    let _ = std::fs::remove_file(path);
    for suffix in ["-wal", "-shm"] {
        let mut sidecar = path.as_os_str().to_owned();
        sidecar.push(suffix);
        let _ = std::fs::remove_file(std::path::PathBuf::from(sidecar));
    }
}

fn copy_file_shared(source_path: &Path, destination: &mut std::fs::File) -> Result<(), CookieError> {
    use std::io::{Read, Write};

    #[cfg(windows)]
    let mut source = {
        use std::os::windows::fs::OpenOptionsExt;

        const FILE_SHARE_READ: u32 = 0x00000001;
        const FILE_SHARE_WRITE: u32 = 0x00000002;
        const FILE_SHARE_DELETE: u32 = 0x00000004;
        std::fs::OpenOptions::new()
            .read(true)
            .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE)
            .open(source_path)?
    };
    #[cfg(not(windows))]
    let mut source = std::fs::File::open(source_path)?;

    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let read = source.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        destination.write_all(&buffer[..read])?;
    }
    Ok(())
}

impl Drop for TemporaryCookieDatabase {
    fn drop(&mut self) {
        for path in [
            self.path.clone(),
            {
                let mut s = self.path.as_os_str().to_owned();
                s.push("-wal");
                std::path::PathBuf::from(s)
            },
            {
                let mut s = self.path.as_os_str().to_owned();
                s.push("-shm");
                std::path::PathBuf::from(s)
            },
        ] {
            if let Ok(file) = std::fs::OpenOptions::new().write(true).open(&path) {
                let _ = file.set_len(0);
                let _ = file.sync_all();
            }
            if let Err(error) = std::fs::remove_file(&path)
                && error.kind() != std::io::ErrorKind::NotFound
            {
                tracing::warn!(%error, path = %path.display(), "Failed to remove temporary browser cookie database");
            }
        }
    }
}

/// Cookie extractor for browsers
pub struct CookieExtractor;

impl CookieExtractor {
    /// Extract cookies for a domain from a browser
    pub fn extract_for_domain(
        browser: &DetectedBrowser,
        domain: &str,
    ) -> Result<Vec<Cookie>, CookieError> {
        let mut all_cookies = Vec::new();
        // Preserve the first ABE error seen so it can be surfaced when no cookies
        // were recovered from any profile of this browser.
        let mut abe_error: Option<CookieError> = None;

        for profile in &browser.profiles {
            match Self::extract_profile_cookies(browser, profile, domain) {
                Ok(cookies) => all_cookies.extend(cookies),
                Err(CookieError::AppBoundEncryption) => {
                    tracing::debug!(
                        "App-Bound Encryption blocked profile {}: {}",
                        profile.name,
                        browser.browser_type.display_name()
                    );
                    if abe_error.is_none() {
                        abe_error = Some(CookieError::AppBoundEncryption);
                    }
                }
                Err(e) => {
                    tracing::debug!(
                        "Failed to extract cookies from profile {}: {}",
                        profile.name,
                        e
                    );
                }
            }
        }

        // If we obtained no cookies at all and every failure was ABE, surface that
        // specific error so callers can try alternative browsers or manual import.
        if all_cookies.is_empty()
            && let Some(abe_err) = abe_error
        {
            return Err(abe_err);
        }

        Ok(all_cookies)
    }

    /// Extract cookies from a specific profile
    fn extract_profile_cookies(
        browser: &DetectedBrowser,
        profile: &BrowserProfile,
        domain: &str,
    ) -> Result<Vec<Cookie>, CookieError> {
        if browser.browser_type.is_chromium_based() {
            Self::extract_chromium_cookies(browser, profile, domain)
        } else {
            Self::extract_firefox_cookies(profile, domain)
        }
    }

    /// Detect whether Chrome App-Bound Encryption (ABE, Chrome 127+) is active for
    /// this browser profile by checking for the `app_bound_encrypted_key` field in
    /// the Local State JSON.  The field is written by Chrome when it migrates the
    /// cookie-encryption key to the ABE system; its presence means the user-level
    /// DPAPI key stored in `encrypted_key` will no longer decrypt newly written
    /// cookies.
    fn detect_app_bound_encryption(local_state_path: &Path) -> bool {
        let Ok(content) = Self::read_file_shared(local_state_path) else {
            return false;
        };
        let Ok(json) = serde_json::from_str::<serde_json::Value>(&content) else {
            return false;
        };
        let present = json
            .get("os_crypt")
            .and_then(|v| v.get("app_bound_encrypted_key"))
            .is_some();
        if present {
            tracing::debug!("Chrome App-Bound Encryption detected in Local State");
        }
        present
    }

    /// Extract cookies from a Chromium-based browser
    fn extract_chromium_cookies(
        browser: &DetectedBrowser,
        profile: &BrowserProfile,
        domain: &str,
    ) -> Result<Vec<Cookie>, CookieError> {
        tracing::debug!(
            "Reading Chromium cookies for {} profile {}",
            browser.browser_type.display_name(),
            profile.name
        );

        let cookies_db = profile
            .chromium_cookie_db_candidates()
            .into_iter()
            .find(|path| path.is_file())
            .ok_or_else(|| {
                CookieError::NotFound(format!(
                    "Cookies database not found for {} profile {}",
                    browser.browser_type.display_name(),
                    profile.name
                ))
            })?;

        // Get the encryption key from Local State
        let local_state_path = profile.local_state_path(&browser.user_data_dir);
        let encryption_key = Self::get_chromium_encryption_key(&local_state_path).map_err(|e| {
            tracing::debug!("Failed to get encryption key: {}", e);
            e
        })?;
        tracing::debug!("Got encryption key ({} bytes)", encryption_key.len());

        // Copy the database to a temp file (browser may have it locked)
        tracing::debug!(path = %cookies_db.display(), "Copying cookies DB to temp...");
        let temp_db = TemporaryCookieDatabase::create(&cookies_db).map_err(|e| {
            tracing::debug!("Failed to copy cookies DB: {}", e);
            e
        })?;

        let mut cookies = Vec::new();
        let mut decrypt_failures: u32 = 0;
        let mut abe_decrypt_failures: u32 = 0;
        let mut candidate_rows: u32 = 0;
        {
            // Keep SQLite handles scoped so Windows can delete the temp DB afterward.
            let conn = Connection::open(temp_db.path())?;

            // Prefer an equality-friendly LIKE that still matches subdomains:
            // host_key is stored as "example.com" or ".example.com".
            let mut stmt = conn.prepare(
                "SELECT name, encrypted_value, host_key, path, expires_utc, is_secure, is_httponly
                 FROM cookies
                 WHERE host_key = ?1
                    OR host_key = ?2
                    OR host_key LIKE ?3
                    OR host_key LIKE ?4",
            )?;

            let bare = domain.trim().trim_start_matches('.').to_string();
            let dotted = format!(".{bare}");
            let like_suffix = format!("%.{bare}");
            let like_any = format!("%{bare}");

            let rows = stmt.query_map(
                rusqlite::params![bare, dotted, like_suffix, like_any],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,   // name
                        row.get::<_, Vec<u8>>(1)?,  // encrypted_value
                        row.get::<_, String>(2)?,   // host_key
                        row.get::<_, String>(3)?,   // path
                        row.get::<_, i64>(4)?,      // expires_utc
                        row.get::<_, i32>(5)? != 0, // is_secure
                        row.get::<_, i32>(6)? != 0, // is_httponly
                    ))
                },
            )?;

            for row in rows {
                let (name, encrypted_value, host_key, path, expires_utc, is_secure, is_http_only) =
                    row?;

                if !domain_matches(&host_key, domain) {
                    continue;
                }
                candidate_rows += 1;

                // Decrypt the cookie value
                let value = match Self::decrypt_chromium_cookie(&encrypted_value, &encryption_key) {
                    Ok(v) => v,
                    Err(CookieError::AppBoundEncryption) => {
                        tracing::debug!("Candidate cookie uses Chromium App-Bound Encryption");
                        decrypt_failures += 1;
                        abe_decrypt_failures += 1;
                        continue;
                    }
                    Err(e) => {
                        tracing::debug!("Failed to decrypt a candidate cookie: {}", e);
                        decrypt_failures += 1;
                        continue;
                    }
                };

                cookies.push(Cookie {
                    name,
                    value,
                    domain: host_key,
                    path,
                    expires: if expires_utc > 0 {
                        Some(expires_utc)
                    } else {
                        None
                    },
                    is_secure,
                    is_http_only,
                });
            }
        }

        tracing::debug!(
            "Found {} cookies for {} ({} candidates, {} failed to decrypt)",
            cookies.len(),
            domain,
            candidate_rows,
            decrypt_failures
        );

        // If every candidate cookie failed to decrypt and no cookies were recovered,
        // check whether Chrome App-Bound Encryption (Chrome 127+) is the culprit.
        // ABE replaces the user-level DPAPI cookie key with a system-level key that
        // cannot be read by third-party tools, causing systematic AES-GCM auth failures.
        if cookies.is_empty()
            && candidate_rows > 0
            && (abe_decrypt_failures > 0
                || (decrypt_failures > 0 && Self::detect_app_bound_encryption(&local_state_path)))
        {
            tracing::warn!(
                browser = %browser.browser_type.display_name(),
                profile = %profile.name,
                decrypt_failures,
                abe_decrypt_failures,
                candidates = candidate_rows,
                "Chromium App-Bound Encryption (ABE) detected: all cookies failed to decrypt"
            );
            return Err(CookieError::AppBoundEncryption);
        }

        Ok(cookies)
    }

    /// Get the Chromium encryption key from Local State
    fn get_chromium_encryption_key(local_state_path: &Path) -> Result<Vec<u8>, CookieError> {
        let content = Self::read_file_shared(local_state_path)?;
        let json: serde_json::Value =
            serde_json::from_str(&content).map_err(|e| CookieError::Decryption(e.to_string()))?;

        let encrypted_key_b64 = json
            .get("os_crypt")
            .and_then(|v| v.get("encrypted_key"))
            .and_then(|v| v.as_str())
            .ok_or(CookieError::NoEncryptionKey)?;

        // Decode base64
        let encrypted_key = base64::engine::general_purpose::STANDARD
            .decode(encrypted_key_b64)
            .map_err(|e| CookieError::Decryption(e.to_string()))?;

        // Remove "DPAPI" prefix (first 5 bytes)
        if encrypted_key.len() < 5 || &encrypted_key[0..5] != b"DPAPI" {
            return Err(CookieError::Decryption(
                "Invalid encrypted key format".to_string(),
            ));
        }

        let encrypted_key = &encrypted_key[5..];

        // Decrypt with DPAPI
        Self::dpapi_decrypt(encrypted_key)
    }

    /// Decrypt data using Windows DPAPI
    #[cfg(windows)]
    fn dpapi_decrypt(encrypted_data: &[u8]) -> Result<Vec<u8>, CookieError> {
        use windows::Win32::Foundation::{HLOCAL, LocalFree};
        use windows::Win32::Security::Cryptography::{CRYPT_INTEGER_BLOB, CryptUnprotectData};

        unsafe {
            let input_blob = CRYPT_INTEGER_BLOB {
                cbData: encrypted_data.len() as u32,
                pbData: encrypted_data.as_ptr() as *mut u8,
            };

            let mut output_blob = CRYPT_INTEGER_BLOB {
                cbData: 0,
                pbData: std::ptr::null_mut(),
            };

            let result =
                CryptUnprotectData(&input_blob, None, None, None, None, 0, &mut output_blob);

            if result.is_err() {
                return Err(CookieError::Dpapi(format!(
                    "CryptUnprotectData failed: {:?}",
                    result
                )));
            }

            if output_blob.pbData.is_null() {
                return Err(CookieError::Dpapi("Output is null".to_string()));
            }

            let decrypted =
                std::slice::from_raw_parts(output_blob.pbData, output_blob.cbData as usize)
                    .to_vec();

            // Free the DPAPI-allocated buffer to prevent memory leaks
            let _ = LocalFree(HLOCAL(output_blob.pbData as *mut _));

            Ok(decrypted)
        }
    }

    #[cfg(not(windows))]
    fn dpapi_decrypt(_encrypted_data: &[u8]) -> Result<Vec<u8>, CookieError> {
        if crate::wsl::is_wsl() {
            Err(CookieError::Dpapi(
                "DPAPI is not available in WSL. Chromium cookies cannot be automatically \
                 extracted. Use manual cookies (Settings → Cookies) or CLI-based authentication \
                 instead. Run Ceiling natively on Windows for automatic cookie extraction."
                    .to_string(),
            ))
        } else {
            Err(CookieError::Dpapi(
                "DPAPI is only available on Windows".to_string(),
            ))
        }
    }

    /// Decrypt a Chromium cookie value
    fn decrypt_chromium_cookie(encrypted_value: &[u8], key: &[u8]) -> Result<String, CookieError> {
        if encrypted_value.is_empty() {
            return Ok(String::new());
        }

        // Check for v10/v11 prefix (AES-256-GCM)
        // Need at least: 3 (prefix) + 12 (nonce) + 16 (tag) = 31 bytes minimum
        let has_v10_prefix = encrypted_value.len() >= 31 && &encrypted_value[0..3] == b"v10";
        let has_v11_prefix = encrypted_value.len() >= 31 && &encrypted_value[0..3] == b"v11";
        let has_v20_prefix = encrypted_value.len() >= 3 && &encrypted_value[0..3] == b"v20";

        if has_v20_prefix {
            // Chrome/Edge 127+ on Windows use App-Bound Encryption for these
            // cookies. Treating the blob as old DPAPI data produces a misleading
            // DPAPI or "no cookies found" error even though the user is signed in.
            return Err(CookieError::AppBoundEncryption);
        }

        if has_v10_prefix || has_v11_prefix {
            let prefix = &encrypted_value[0..3];
            tracing::debug!(
                "Decrypting cookie with {} prefix, {} bytes total",
                String::from_utf8_lossy(prefix),
                encrypted_value.len(),
            );

            // v10/v11: 3 byte prefix + 12 byte nonce + ciphertext + 16 byte tag
            let nonce = &encrypted_value[3..15];
            let ciphertext = &encrypted_value[15..];

            let cipher = Aes256Gcm::new_from_slice(key)
                .map_err(|e| CookieError::Decryption(format!("cipher init: {}", e)))?;

            let nonce_obj = Nonce::from_slice(nonce);

            let plaintext = cipher.decrypt(nonce_obj, ciphertext).map_err(|e| {
                tracing::debug!("AES-GCM decrypt failed: {}", e);
                CookieError::Decryption(format!("decrypt: {}", e))
            })?;

            tracing::debug!("Decrypted {} bytes successfully", plaintext.len(),);

            // Some Chromium versions prepend metadata bytes before the actual
            // cookie value in the AES-GCM plaintext.  If the leading bytes look
            // non-ASCII, skip up to a 32-byte internal header to find the start
            // of the cookie string.  This is distinct from App-Bound Encryption
            // (ABE): ABE failures are caught upstream as AES-GCM authentication
            // errors and never reach this point.
            let value_bytes = if plaintext.len() > 32 {
                // Check if first 32 bytes are garbage (non-ASCII)
                let has_garbage_prefix = plaintext[..32].iter().any(|&b| !(32..=127).contains(&b));
                if has_garbage_prefix {
                    // Find where ASCII text starts (skip prefix)
                    let start = plaintext
                        .iter()
                        .position(|&b| {
                            // Look for common cookie value start chars
                            b.is_ascii_alphanumeric() || b == b'"' || b == b'{'
                        })
                        .unwrap_or(0);

                    // But use a minimum of 32 bytes prefix for App-Bound Encryption
                    let actual_start = if start < 32 && plaintext.len() > 32 {
                        32
                    } else {
                        start
                    };

                    tracing::debug!(
                        "Skipping {} byte prefix (App-Bound Encryption)",
                        actual_start
                    );
                    &plaintext[actual_start..]
                } else {
                    &plaintext[..]
                }
            } else {
                &plaintext[..]
            };

            String::from_utf8(value_bytes.to_vec()).map_err(|e| {
                tracing::debug!("UTF-8 conversion failed after prefix strip: {}", e);
                CookieError::Decryption(e.to_string())
            })
        } else {
            tracing::debug!(
                "Cookie not v10/v11 format, total {} bytes",
                encrypted_value.len()
            );
            // Old format: DPAPI encrypted directly
            let decrypted = Self::dpapi_decrypt(encrypted_value)?;
            String::from_utf8(decrypted).map_err(|e| CookieError::Decryption(e.to_string()))
        }
    }

    /// Extract cookies from Firefox
    fn extract_firefox_cookies(
        profile: &BrowserProfile,
        domain: &str,
    ) -> Result<Vec<Cookie>, CookieError> {
        let cookies_db = profile.firefox_cookies_db_path();

        if !cookies_db.exists() {
            return Err(CookieError::NotFound(format!(
                "Cookies database not found for Firefox profile {}",
                profile.name
            )));
        }

        // Copy to temp (browser may have it locked), including -wal/-shm.
        let temp_db = TemporaryCookieDatabase::create(&cookies_db)?;

        let bare = domain.trim().trim_start_matches('.').to_string();
        let dotted = format!(".{bare}");
        let like_suffix = format!("%.{bare}");
        let like_any = format!("%{bare}");

        let mut cookies = Vec::new();
        {
            // Keep SQLite handles scoped so Windows can delete the temp DB afterward.
            let conn = Connection::open(temp_db.path())?;

            let mut stmt = conn.prepare(
                "SELECT name, value, host, path, expiry, isSecure, isHttpOnly
                 FROM moz_cookies
                 WHERE host = ?1
                    OR host = ?2
                    OR host LIKE ?3
                    OR host LIKE ?4",
            )?;

            let rows = stmt.query_map(
                rusqlite::params![bare, dotted, like_suffix, like_any],
                |row| {
                    Ok(Cookie {
                        name: row.get(0)?,
                        value: row.get(1)?,
                        domain: row.get(2)?,
                        path: row.get(3)?,
                        expires: row.get(4).ok(),
                        is_secure: row.get::<_, i32>(5)? != 0,
                        is_http_only: row.get::<_, i32>(6)? != 0,
                    })
                },
            )?;

            for row in rows {
                let cookie = row?;
                if domain_matches(&cookie.domain, domain) {
                    cookies.push(cookie);
                }
            }
        }

        Ok(cookies)
    }

    /// Read a file using shared mode to handle locked files
    #[cfg(windows)]
    fn read_file_shared(path: &Path) -> Result<String, CookieError> {
        use std::io::Read;
        use std::os::windows::fs::OpenOptionsExt;

        const FILE_SHARE_READ: u32 = 0x00000001;
        const FILE_SHARE_WRITE: u32 = 0x00000002;
        const FILE_SHARE_DELETE: u32 = 0x00000004;

        let mut file = std::fs::OpenOptions::new()
            .read(true)
            .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE)
            .open(path)?;

        let mut content = String::new();
        file.read_to_string(&mut content)?;
        Ok(content)
    }

    #[cfg(not(windows))]
    fn read_file_shared(path: &Path) -> Result<String, CookieError> {
        Ok(std::fs::read_to_string(path)?)
    }

    /// Build a cookie header string for HTTP requests
    pub fn build_cookie_header(cookies: &[Cookie]) -> String {
        cookies
            .iter()
            .map(|c| c.to_header_value())
            .collect::<Vec<_>>()
            .join("; ")
    }
}

fn domain_matches(host_key: &str, domain: &str) -> bool {
    let host = host_key.trim().trim_end_matches('.').to_ascii_lowercase();
    let domain = domain
        .trim()
        .trim_start_matches('.')
        .trim_end_matches('.')
        .to_ascii_lowercase();
    host == domain || host == format!(".{domain}") || host.ends_with(&format!(".{domain}"))
}

/// Result of probing one browser for a domain (diagnostics / UI).
#[derive(Debug, Clone)]
pub struct BrowserCookieProbe {
    pub browser_key: String,
    pub browser_name: String,
    pub profile_count: usize,
    pub status: BrowserCookieProbeStatus,
    pub detail: String,
    pub cookie_count: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BrowserCookieProbeStatus {
    Ready,
    Empty,
    AppBoundEncryption,
    LockedOrUnreadable,
    MissingDatabase,
    Error,
}

/// Probe every detected browser for cookies on `domain` without short-circuiting.
pub fn diagnose_cookies_for_domain(domain: &str) -> Vec<BrowserCookieProbe> {
    use super::detection::BrowserDetector;

    let browsers = BrowserDetector::detect_all();
    let mut probes = Vec::with_capacity(browsers.len());
    for browser in browsers {
        probes.push(probe_browser_for_domain(&browser, domain));
    }
    probes
}

fn probe_browser_for_domain(
    browser: &super::detection::DetectedBrowser,
    domain: &str,
) -> BrowserCookieProbe {
    let browser_key = browser.browser_type.key().to_string();
    let browser_name = browser.browser_type.display_name().to_string();
    let profile_count = browser.profiles.len();
    match CookieExtractor::extract_for_domain(browser, domain) {
        Ok(cookies) if !cookies.is_empty() => BrowserCookieProbe {
            browser_key,
            browser_name,
            profile_count,
            status: BrowserCookieProbeStatus::Ready,
            detail: format!(
                "{} readable cookie{}",
                cookies.len(),
                if cookies.len() == 1 { "" } else { "s" }
            ),
            cookie_count: cookies.len(),
        },
        Ok(_) => BrowserCookieProbe {
            browser_key,
            browser_name,
            profile_count,
            status: BrowserCookieProbeStatus::Empty,
            detail: format!("No cookies for {domain} in any profile (signed out?)"),
            cookie_count: 0,
        },
        Err(CookieError::AppBoundEncryption) => BrowserCookieProbe {
            browser_key,
            browser_name,
            profile_count,
            status: BrowserCookieProbeStatus::AppBoundEncryption,
            detail: "App-Bound Encryption blocks automatic decrypt (v20 cookies)".into(),
            cookie_count: 0,
        },
        Err(CookieError::NotFound(msg)) => BrowserCookieProbe {
            browser_key,
            browser_name,
            profile_count,
            status: BrowserCookieProbeStatus::MissingDatabase,
            detail: msg,
            cookie_count: 0,
        },
        Err(CookieError::Io(err))
            if err.kind() == std::io::ErrorKind::PermissionDenied
                || err.raw_os_error() == Some(32) =>
        {
            BrowserCookieProbe {
                browser_key,
                browser_name,
                profile_count,
                status: BrowserCookieProbeStatus::LockedOrUnreadable,
                detail: "Cookie database locked by a running browser; close it and retry".into(),
                cookie_count: 0,
            }
        }
        Err(err) => BrowserCookieProbe {
            browser_key,
            browser_name,
            profile_count,
            status: BrowserCookieProbeStatus::Error,
            detail: err.to_string(),
            cookie_count: 0,
        },
    }
}

/// Helper to get cookies for a specific domain from any available browser
pub fn get_cookies_for_domain(domain: &str) -> Result<Vec<Cookie>, CookieError> {
    use super::detection::BrowserDetector;

    let browsers = BrowserDetector::detect_all();

    if browsers.is_empty() {
        return Err(CookieError::BrowserNotInstalled);
    }

    // Track scan outcomes so the final error is actionable instead of a bare
    // "not found" after every Chromium browser hits App-Bound Encryption.
    let mut abe_browsers: Vec<String> = Vec::new();
    let mut empty_browsers: Vec<String> = Vec::new();
    let mut other_notes: Vec<String> = Vec::new();

    // Try each browser until we find cookies (Firefox-family first).
    for browser in &browsers {
        match CookieExtractor::extract_for_domain(browser, domain) {
            Ok(cookies) if !cookies.is_empty() => {
                tracing::info!(
                    browser = %browser.browser_type.display_name(),
                    count = cookies.len(),
                    %domain,
                    "automatic cookie import succeeded"
                );
                return Ok(cookies);
            }
            Ok(_) => {
                empty_browsers.push(browser.browser_type.display_name().to_string());
            }
            Err(CookieError::AppBoundEncryption) => {
                tracing::warn!(
                    browser = %browser.browser_type.display_name(),
                    "App-Bound Encryption prevents automatic cookie import; \
                     trying remaining browsers"
                );
                abe_browsers.push(browser.browser_type.display_name().to_string());
            }
            Err(e) => {
                tracing::debug!(
                    "Failed to get cookies from {}: {}",
                    browser.browser_type.display_name(),
                    e
                );
                other_notes.push(format!(
                    "{}: {}",
                    browser.browser_type.display_name(),
                    e
                ));
            }
        }
    }

    if !abe_browsers.is_empty() && empty_browsers.is_empty() && other_notes.is_empty() {
        return Err(CookieError::AppBoundEncryption);
    }

    if !abe_browsers.is_empty() {
        let mut parts = vec![format!(
            "Could not read cookies for {domain}. App-Bound Encryption blocked: {}.",
            abe_browsers.join(", ")
        )];
        if !empty_browsers.is_empty() {
            parts.push(format!(
                "No login cookies found in: {}.",
                empty_browsers.join(", ")
            ));
        }
        parts.push(
            "Sign in with Firefox (automatic works there), or paste the Cookie header from DevTools."
                .into(),
        );
        return Err(CookieError::ScanFailed(parts.join(" ")));
    }

    if !empty_browsers.is_empty() {
        return Err(CookieError::ScanFailed(format!(
            "No cookies for {domain} in scanned browsers ({}). Sign in to that site in Firefox, Chrome, Edge, or Brave, then try again — or paste the Cookie header manually.",
            empty_browsers.join(", ")
        )));
    }

    Err(CookieError::NotFound(domain.to_string()))
}

/// Get a cookie header string for a domain
pub fn get_cookie_header(domain: &str) -> Result<String, CookieError> {
    let cookies = get_cookies_for_domain(domain)?;
    Ok(CookieExtractor::build_cookie_header(&cookies))
}

/// Get a cookie header string from the first domain that has readable cookies.
pub fn get_cookie_header_for_domains(domains: &[&str]) -> Result<String, CookieError> {
    let mut app_bound_encryption_seen = false;
    let mut last_error = None;

    for domain in domains {
        match get_cookie_header(domain) {
            Ok(header) if !header.trim().is_empty() => return Ok(header),
            Ok(_) => {}
            Err(CookieError::AppBoundEncryption) => app_bound_encryption_seen = true,
            Err(error) => last_error = Some(error),
        }
    }

    if app_bound_encryption_seen {
        Err(CookieError::AppBoundEncryption)
    } else {
        Err(last_error.unwrap_or_else(|| CookieError::NotFound(domains.join(", "))))
    }
}

/// Get a cookie header string for a domain from a specific browser
pub fn get_cookie_header_from_browser(
    domain: &str,
    browser: &super::detection::DetectedBrowser,
) -> Result<String, CookieError> {
    let cookies = CookieExtractor::extract_for_domain(browser, domain)?;
    if cookies.is_empty() {
        return Err(CookieError::NotFound(domain.to_string()));
    }
    Ok(CookieExtractor::build_cookie_header(&cookies))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn temporary_cookie_database_is_removed_on_drop() {
        let source_path = std::env::temp_dir().join(format!(
            "ceiling_cookie_source_{}.sqlite",
            uuid::Uuid::new_v4()
        ));
        std::fs::write(&source_path, b"cookie database contents").unwrap();

        let temporary = TemporaryCookieDatabase::create(&source_path).unwrap();
        let temporary_path = temporary.path().to_path_buf();
        assert_eq!(
            std::fs::read(&temporary_path).unwrap(),
            b"cookie database contents"
        );
        drop(temporary);

        assert!(!temporary_path.exists());
        std::fs::remove_file(source_path).unwrap();
    }

    #[test]
    fn domain_matching_requires_boundary() {
        assert!(domain_matches("chatgpt.com", "chatgpt.com"));
        assert!(domain_matches(".chatgpt.com", "chatgpt.com"));
        assert!(domain_matches("auth.chatgpt.com", "chatgpt.com"));
        assert!(domain_matches("AUTH.CHATGPT.COM.", "chatgpt.com"));
        assert!(!domain_matches("evilchatgpt.com", "chatgpt.com"));
        assert!(!domain_matches("chatgpt.com.evil.test", "chatgpt.com"));
    }

    #[test]
    fn test_cookie_extraction() {
        // This test will only work on a machine with Chrome installed
        match get_cookies_for_domain("claude.ai") {
            Ok(cookies) => {
                println!("Found {} cookies for claude.ai", cookies.len());
                for cookie in &cookies {
                    println!(
                        "  {}={}",
                        cookie.name,
                        &cookie.value[..20.min(cookie.value.len())]
                    );
                }
            }
            Err(e) => {
                println!("Could not get cookies: {}", e);
            }
        }
    }

    /// Verify that the ABE error variant formats a readable, actionable message.
    #[test]
    fn test_abe_error_display() {
        let err = CookieError::AppBoundEncryption;
        let msg = err.to_string();
        assert!(
            msg.contains("App-Bound Encryption"),
            "ABE error should mention App-Bound Encryption"
        );
        assert!(
            msg.contains("Chrome/Edge/Brave") || msg.contains("Chrome/Edge"),
            "ABE error should identify Chromium browsers"
        );
        assert!(
            msg.contains("Paste") || msg.contains("manual") || msg.contains("Firefox"),
            "ABE error should mention manual import or Firefox fallback"
        );
    }

    #[test]
    fn diagnose_returns_probe_per_detected_browser() {
        let probes = diagnose_cookies_for_domain("example.invalid");
        // On CI without browsers this may be empty; on a real Windows desktop
        // we expect at least one probe with a stable key.
        for probe in &probes {
            assert!(!probe.browser_key.is_empty());
            assert!(!probe.browser_name.is_empty());
        }
    }

    /// Verify that modern Chromium `v20` cookies are recognized as App-Bound
    /// Encryption instead of being misrouted through legacy DPAPI decryption.
    #[test]
    fn test_v20_cookie_reports_app_bound_encryption() {
        let mut encrypted_value = b"v20".to_vec();
        encrypted_value.extend_from_slice(&[0x42; 48]);
        let key = [0_u8; 32];

        let err = CookieExtractor::decrypt_chromium_cookie(&encrypted_value, &key)
            .expect_err("v20 cookies should report App-Bound Encryption");

        assert!(
            matches!(err, CookieError::AppBoundEncryption),
            "expected AppBoundEncryption, got {err:?}"
        );
    }

    /// Verify that ABE detection returns false for a Local State JSON without the field.
    #[test]
    fn test_detect_abe_absent() {
        use std::io::Write;

        let dir = std::env::temp_dir();
        let path = dir.join("codexbar_test_local_state_no_abe.json");
        {
            let mut f = std::fs::File::create(&path).unwrap();
            write!(f, r#"{{"os_crypt":{{"encrypted_key":"QUJDREVGR0g="}}}}"#).unwrap();
        }
        let detected = CookieExtractor::detect_app_bound_encryption(&path);
        let _ = std::fs::remove_file(&path);
        assert!(!detected, "ABE should not be detected when field is absent");
    }

    /// Verify that ABE detection returns true for a Local State JSON with the ABE field.
    #[test]
    fn test_detect_abe_present() {
        use std::io::Write;

        let dir = std::env::temp_dir();
        let path = dir.join("codexbar_test_local_state_abe.json");
        {
            let mut f = std::fs::File::create(&path).unwrap();
            write!(
                f,
                r#"{{"os_crypt":{{"encrypted_key":"QUJDREVGR0g=","app_bound_encrypted_key":"c29tZWtleQ=="}}}}"#
            )
            .unwrap();
        }
        let detected = CookieExtractor::detect_app_bound_encryption(&path);
        let _ = std::fs::remove_file(&path);
        assert!(detected, "ABE should be detected when field is present");
    }
}
