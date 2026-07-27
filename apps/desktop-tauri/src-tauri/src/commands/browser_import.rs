use super::*;

// ── Browser cookie import commands ────────────────────────────────────

/// Bridge-friendly detected browser entry.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DetectedBrowserBridge {
    /// Stable key used when calling `import_browser_cookies`.
    pub browser_type: String,
    pub display_name: String,
    pub profile_count: usize,
}

/// Per-browser cookie probe for Settings diagnostics.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BrowserCookieProbeBridge {
    pub browser_type: String,
    pub display_name: String,
    pub profile_count: usize,
    pub status: String,
    pub detail: String,
    pub cookie_count: usize,
}

/// List all browsers detected on this machine that Ceiling can read cookies from.
///
/// On non-Windows platforms (e.g. Linux CI) this returns an empty list because
/// DPAPI is unavailable; the UI should hide/disable the import button in that case.
#[tauri::command]
pub fn list_detected_browsers() -> Vec<DetectedBrowserBridge> {
    use codexbar::browser::detection::BrowserDetector;

    BrowserDetector::detect_all()
        .into_iter()
        .map(|b| DetectedBrowserBridge {
            browser_type: b.browser_type.key().to_string(),
            display_name: b.browser_type.display_name().to_string(),
            profile_count: b.profiles.len(),
        })
        .collect()
}

/// Diagnose automatic cookie availability for a provider across every browser.
#[tauri::command]
pub fn diagnose_browser_cookies(provider_id: String) -> Result<Vec<BrowserCookieProbeBridge>, String> {
    use codexbar::browser::cookies::{BrowserCookieProbeStatus, diagnose_cookies_for_domain};

    let pid = parse_provider_arg(&provider_id)?;
    let settings = Settings::load();
    let domain = cookie_domain_for_provider(pid, &settings)?;

    Ok(diagnose_cookies_for_domain(domain)
        .into_iter()
        .map(|probe| BrowserCookieProbeBridge {
            browser_type: probe.browser_key,
            display_name: probe.browser_name,
            profile_count: probe.profile_count,
            status: match probe.status {
                BrowserCookieProbeStatus::Ready => "ready",
                BrowserCookieProbeStatus::Empty => "empty",
                BrowserCookieProbeStatus::AppBoundEncryption => "abe",
                BrowserCookieProbeStatus::LockedOrUnreadable => "locked",
                BrowserCookieProbeStatus::MissingDatabase => "missing",
                BrowserCookieProbeStatus::Error => "error",
            }
            .to_string(),
            detail: probe.detail,
            cookie_count: probe.cookie_count,
        })
        .collect())
}

/// Import cookies for `provider_id` from the named browser and persist them as
/// a manual-cookie override, replacing any existing entry for that provider.
///
/// `browser_type` must be one of the keys returned by `list_detected_browsers`
/// (e.g. `"chrome"`, `"edge"`, `"brave"`, `"firefox"`). Pass `"auto"` to try
/// every detected browser in preferred order (Firefox first).
///
/// Returns the updated manual-cookies list on success.
#[tauri::command]
pub fn import_browser_cookies(
    provider_id: String,
    browser_type: String,
) -> Result<Vec<CookieInfoBridge>, String> {
    use codexbar::browser::cookies::{CookieExtractor, get_cookies_for_domain};
    use codexbar::browser::detection::{BrowserDetector, BrowserType};

    // Resolve the provider to get its cookie domain.
    let pid = parse_provider_arg(&provider_id)?;
    let settings = Settings::load();
    let domain = cookie_domain_for_provider(pid, &settings)?;

    let cookies = if browser_type.eq_ignore_ascii_case("auto") {
        get_cookies_for_domain(domain).map_err(format_cookie_error)?
    } else {
        let wanted = BrowserType::from_key(&browser_type)
            .ok_or_else(|| format!("Unknown browser '{browser_type}'"))?;
        let browsers = BrowserDetector::detect_all();
        let browser = browsers
            .into_iter()
            .find(|b| b.browser_type == wanted)
            .ok_or_else(|| format!("Browser '{browser_type}' not found or not installed"))?;
        CookieExtractor::extract_for_domain(&browser, domain).map_err(format_cookie_error)?
    };

    if cookies.is_empty() {
        return Err(format!(
            "No cookies found for {domain}. Chrome/Edge/Brave often block automatic import (App-Bound Encryption). Sign in with Firefox, or paste the Cookie header from DevTools. For Cursor, Automatic can also read the IDE session on disk."
        ));
    }

    let mut cookie_header = CookieExtractor::build_cookie_header(&cookies);
    if pid == codexbar::core::ProviderId::Cursor {
        cookie_header = codexbar::providers::cursor::normalize_cookie_header(&cookie_header)
            .filter(|header| {
                header
                    .to_ascii_lowercase()
                    .contains("workoscursorsessiontoken=")
            })
            .ok_or_else(|| {
                "Found cookies for cursor.com, but no WorkosCursorSessionToken. Sign in at cursor.com (Google SSO or email), then import again — or set Automatic to use the Cursor IDE session.".to_string()
            })?;
    }
    validate_single_line_secret(&cookie_header, "Cookie header", MAX_COOKIE_HEADER_LEN)?;

    // Persist as manual cookie.
    let mut manual = ManualCookies::load();
    manual.set(pid.cli_name(), &cookie_header);
    manual.save().map_err(|e| e.to_string())?;

    Ok(get_manual_cookies())
}

fn cookie_domain_for_provider(
    pid: codexbar::core::ProviderId,
    settings: &Settings,
) -> Result<&'static str, String> {
    if pid == codexbar::core::ProviderId::MiniMax {
        return Ok(
            codexbar::providers::MiniMaxProvider::cookie_domain_for_region(Some(
                settings.api_region(pid),
            )),
        );
    }
    pid.cookie_domain()
        .ok_or_else(|| format!("Provider '{}' does not use cookie authentication", pid.cli_name()))
}

fn format_cookie_error(error: codexbar::browser::cookies::CookieError) -> String {
    use codexbar::browser::cookies::CookieError;
    match error {
        CookieError::Dpapi(msg) => format!("DPAPI error: {msg}"),
        other => other.to_string(),
    }
}
