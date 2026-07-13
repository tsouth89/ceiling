//! Cursor session resolution: paste normalization + IDE disk auth.
//!
//! Preferred auth order for Automatic:
//! 1. Manual / pasted session cookie
//! 2. Cursor IDE `state.vscdb` (`cursorAuth/accessToken`)
//! 3. Browser cookies for cursor.com / cursor.sh
//!
//! The web dashboard API expects
//! `Cookie: WorkosCursorSessionToken=<sub>%3A%3A<jwt>`.
//! The IDE stores only the JWT; we rebuild the cookie from the JWT `sub` claim.

use base64::Engine;
use rusqlite::{Connection, OpenFlags, types::Value as SqlValue};
use serde::Deserialize;
use std::path::{Path, PathBuf};

use crate::core::ProviderError;

pub(super) const SESSION_COOKIE_NAME: &str = "WorkosCursorSessionToken";
const ACCESS_TOKEN_KEY: &str = "cursorAuth/accessToken";

#[derive(Debug, Deserialize)]
struct JwtClaims {
    sub: Option<String>,
}

/// Normalize pasted Cursor session material into a Cookie header value.
///
/// Accepts:
/// - `WorkosCursorSessionToken=...` (with or without `Cookie:` prefix)
/// - bare `user_…%3A%3A<jwt>` / `user_…::<jwt>`
/// - bare JWT (rebuilds `sub%3A%3Ajwt` from the payload)
pub fn normalize_cookie_header(input: &str) -> Option<String> {
    let mut header = input.trim();
    if header.is_empty() {
        return None;
    }

    if header
        .get(.."cookie:".len())
        .is_some_and(|prefix| prefix.eq_ignore_ascii_case("cookie:"))
    {
        header = header["cookie:".len()..].trim();
    }

    if header.is_empty() {
        return None;
    }

    // Full Cookie header with one or more pairs.
    if header.contains('=') {
        return Some(prefer_session_cookie_pairs(header));
    }

    // Bare session value: user_…::jwt or user_…%3A%3Ajwt
    if looks_like_session_value(header) {
        return Some(format!(
            "{}={}",
            SESSION_COOKIE_NAME,
            encode_session_separators(header)
        ));
    }

    // Bare JWT from DevTools / IDE — rebuild WorkosCursorSessionToken.
    if let Some(cookie) = cookie_from_access_token(header) {
        return Some(cookie);
    }

    None
}

/// Read a session cookie from the local Cursor IDE state database.
pub fn disk_session_cookie() -> Result<String, ProviderError> {
    let path = default_state_db_path().ok_or_else(|| {
        ProviderError::NotInstalled("Could not resolve Cursor application data path.".to_string())
    })?;
    disk_session_cookie_from_path(&path)
}

pub(super) fn disk_session_cookie_from_path(db_path: &Path) -> Result<String, ProviderError> {
    if !db_path.exists() {
        return Err(ProviderError::NotInstalled(format!(
            "Cursor is not signed in on this machine (missing {}). Open Cursor and sign in, or paste a WorkosCursorSessionToken cookie.",
            db_path.display()
        )));
    }

    let conn = Connection::open_with_flags(db_path, OpenFlags::SQLITE_OPEN_READ_ONLY)
        .map_err(|e| ProviderError::Other(format!("Failed to open Cursor state database: {e}")))?;
    conn.busy_timeout(std::time::Duration::from_millis(250))
        .map_err(|e| {
            ProviderError::Other(format!("Failed to configure Cursor SQLite timeout: {e}"))
        })?;

    let value: SqlValue = conn
        .query_row(
            "SELECT value FROM ItemTable WHERE key = ?1 LIMIT 1",
            [ACCESS_TOKEN_KEY],
            |row| row.get(0),
        )
        .map_err(|e| match e {
            rusqlite::Error::QueryReturnedNoRows => ProviderError::AuthRequired,
            other => ProviderError::Other(format!(
                "Failed to read Cursor access token from IDE state: {other}"
            )),
        })?;

    let token = decode_text_value(value).ok_or_else(|| {
        ProviderError::Parse("Cursor access token in state.vscdb is not valid text".to_string())
    })?;

    cookie_from_access_token(&token).ok_or_else(|| {
        ProviderError::Parse(
            "Cursor IDE access token is not a usable JWT (missing sub claim)".to_string(),
        )
    })
}

fn default_state_db_path() -> Option<PathBuf> {
    if let Ok(path) = std::env::var("CURSOR_STATE_DB")
        && !path.trim().is_empty()
    {
        return Some(PathBuf::from(path));
    }
    dirs::data_dir().map(|base| {
        base.join("Cursor")
            .join("User")
            .join("globalStorage")
            .join("state.vscdb")
    })
}

fn cookie_from_access_token(token: &str) -> Option<String> {
    let token = token.trim();
    if token.is_empty() {
        return None;
    }
    let sub = jwt_sub(token)?;
    Some(format!(
        "{}={}{}{}",
        SESSION_COOKIE_NAME, sub, "%3A%3A", token
    ))
}

fn jwt_sub(token: &str) -> Option<String> {
    let payload = token.split('.').nth(1)?;
    let decoded = decode_jwt_segment(payload)?;
    let claims: JwtClaims = serde_json::from_slice(&decoded).ok()?;
    let sub = claims.sub?.trim().to_string();
    (!sub.is_empty()).then_some(sub)
}

fn decode_jwt_segment(segment: &str) -> Option<Vec<u8>> {
    let padded = match segment.len() % 4 {
        0 => segment.to_string(),
        2 => format!("{segment}=="),
        3 => format!("{segment}="),
        _ => return None,
    };
    base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(segment)
        .or_else(|_| base64::engine::general_purpose::URL_SAFE.decode(padded))
        .ok()
}

fn looks_like_session_value(value: &str) -> bool {
    value.contains("%3A%3A")
        || value.contains("%3a%3a")
        || value.contains("::")
        || value.starts_with("user_")
}

fn encode_session_separators(value: &str) -> String {
    if value.contains("%3A%3A") || value.contains("%3a%3a") {
        return value.to_string();
    }
    value.replacen("::", "%3A%3A", 1)
}

fn prefer_session_cookie_pairs(header: &str) -> String {
    let pairs: Vec<&str> = header
        .split(';')
        .map(str::trim)
        .filter(|part| !part.is_empty())
        .collect();

    if pairs
        .iter()
        .any(|part| cookie_name_eq(part, SESSION_COOKIE_NAME))
    {
        return pairs.join("; ");
    }

    // If the paste used a different casing / name but one pair looks like the session value,
    // rewrite the first matching pair to the canonical cookie name.
    let mut rewritten = false;
    let mut out = Vec::new();
    for part in pairs {
        if !rewritten
            && let Some((_name, value)) = part.split_once('=')
            && looks_like_session_value(value)
        {
            out.push(format!(
                "{}={}",
                SESSION_COOKIE_NAME,
                encode_session_separators(value)
            ));
            rewritten = true;
        } else {
            out.push(part.to_string());
        }
    }
    out.join("; ")
}

fn cookie_name_eq(pair: &str, expected: &str) -> bool {
    pair.split_once('=')
        .is_some_and(|(name, _)| name.eq_ignore_ascii_case(expected))
}

fn decode_text_value(value: SqlValue) -> Option<String> {
    match value {
        SqlValue::Text(text) => {
            let trimmed = text.trim_matches(char::from(0)).trim();
            (!trimmed.is_empty()).then(|| trimmed.to_string())
        }
        SqlValue::Blob(bytes) => {
            if let Ok(text) = std::str::from_utf8(&bytes) {
                let trimmed = text.trim_matches(char::from(0)).trim();
                if !trimmed.is_empty() {
                    return Some(trimmed.to_string());
                }
            }
            None
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use base64::Engine;

    fn make_jwt(sub: &str) -> String {
        let header = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(br#"{"alg":"none"}"#);
        let payload = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .encode(format!(r#"{{"sub":"{sub}"}}"#).as_bytes());
        format!("{header}.{payload}.sig")
    }

    #[test]
    fn wraps_bare_session_value() {
        let value = "user_01ABC%3A%3AeyJhbGciOiJIUzI1NiJ9.payload.sig";
        assert_eq!(
            normalize_cookie_header(value).as_deref(),
            Some("WorkosCursorSessionToken=user_01ABC%3A%3AeyJhbGciOiJIUzI1NiJ9.payload.sig")
        );
    }

    #[test]
    fn encodes_literal_double_colon() {
        let value = "user_01ABC::eyJhbGciOiJIUzI1NiJ9.payload.sig";
        assert_eq!(
            normalize_cookie_header(value).as_deref(),
            Some("WorkosCursorSessionToken=user_01ABC%3A%3AeyJhbGciOiJIUzI1NiJ9.payload.sig")
        );
    }

    #[test]
    fn strips_cookie_prefix() {
        let header = "Cookie: WorkosCursorSessionToken=user_01ABC%3A%3Ajwt";
        assert_eq!(
            normalize_cookie_header(header).as_deref(),
            Some("WorkosCursorSessionToken=user_01ABC%3A%3Ajwt")
        );
    }

    #[test]
    fn rebuilds_cookie_from_bare_jwt() {
        let jwt = make_jwt("user_01TEST");
        assert_eq!(
            normalize_cookie_header(&jwt).as_deref(),
            Some(format!("WorkosCursorSessionToken=user_01TEST%3A%3A{jwt}").as_str())
        );
    }

    #[test]
    fn rejects_empty() {
        assert_eq!(normalize_cookie_header("   "), None);
        assert_eq!(normalize_cookie_header("Cookie:   "), None);
    }

    #[test]
    fn disk_session_from_fixture_db() {
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("state.vscdb");
        let jwt = make_jwt("google-oauth2|123");
        {
            let conn = Connection::open(&db_path).unwrap();
            conn.execute(
                "CREATE TABLE ItemTable (key TEXT PRIMARY KEY, value TEXT)",
                [],
            )
            .unwrap();
            conn.execute(
                "INSERT INTO ItemTable(key, value) VALUES (?1, ?2)",
                [ACCESS_TOKEN_KEY, jwt.as_str()],
            )
            .unwrap();
        }

        let cookie = disk_session_cookie_from_path(&db_path).unwrap();
        assert_eq!(
            cookie,
            format!("WorkosCursorSessionToken=google-oauth2|123%3A%3A{jwt}")
        );
    }
}
