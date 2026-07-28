//! Browser detection for Windows and WSL
//!
//! On native Windows, uses standard AppData paths for Chrome, Edge, Brave,
//! Chromium variants, and Firefox-family browsers. On WSL, resolves browser
//! paths via /mnt/c/ to access Windows browser data.

#![allow(dead_code)]

use std::path::{Path, PathBuf};

use crate::wsl;

/// Supported browser types
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum BrowserType {
    /// Prefer Firefox-family first: cookies are plaintext and work under ABE.
    Firefox,
    FirefoxDeveloper,
    LibreWolf,
    Floorp,
    Waterfox,
    Chrome,
    ChromeBeta,
    ChromeCanary,
    Edge,
    EdgeBeta,
    EdgeDev,
    Brave,
    Arc,
    Chromium,
}

impl BrowserType {
    /// Preferred scan order: Firefox-family first (works without ABE), then
    /// Chromium derivatives. Within Chromium, Edge/Brave sometimes lag Chrome
    /// on ABE rollout, so they are still worth trying.
    pub fn all() -> &'static [BrowserType] {
        &[
            BrowserType::Firefox,
            BrowserType::FirefoxDeveloper,
            BrowserType::LibreWolf,
            BrowserType::Floorp,
            BrowserType::Waterfox,
            BrowserType::Edge,
            BrowserType::EdgeBeta,
            BrowserType::EdgeDev,
            BrowserType::Brave,
            BrowserType::Chrome,
            BrowserType::ChromeBeta,
            BrowserType::ChromeCanary,
            BrowserType::Arc,
            BrowserType::Chromium,
        ]
    }

    /// Check if this is a Chromium-based browser
    pub fn is_chromium_based(&self) -> bool {
        !matches!(
            self,
            BrowserType::Firefox
                | BrowserType::FirefoxDeveloper
                | BrowserType::LibreWolf
                | BrowserType::Floorp
                | BrowserType::Waterfox
        )
    }

    /// Stable IPC key used by the Tauri bridge.
    pub fn key(&self) -> &'static str {
        match self {
            BrowserType::Firefox => "firefox",
            BrowserType::FirefoxDeveloper => "firefox-developer",
            BrowserType::LibreWolf => "librewolf",
            BrowserType::Floorp => "floorp",
            BrowserType::Waterfox => "waterfox",
            BrowserType::Chrome => "chrome",
            BrowserType::ChromeBeta => "chrome-beta",
            BrowserType::ChromeCanary => "chrome-canary",
            BrowserType::Edge => "edge",
            BrowserType::EdgeBeta => "edge-beta",
            BrowserType::EdgeDev => "edge-dev",
            BrowserType::Brave => "brave",
            BrowserType::Arc => "arc",
            BrowserType::Chromium => "chromium",
        }
    }

    pub fn from_key(key: &str) -> Option<Self> {
        match key.trim().to_ascii_lowercase().as_str() {
            "firefox" => Some(Self::Firefox),
            "firefox-developer" | "firefoxdev" | "firefox-dev" => Some(Self::FirefoxDeveloper),
            "librewolf" => Some(Self::LibreWolf),
            "floorp" => Some(Self::Floorp),
            "waterfox" => Some(Self::Waterfox),
            "chrome" => Some(Self::Chrome),
            "chrome-beta" | "chromebeta" => Some(Self::ChromeBeta),
            "chrome-canary" | "chromecanary" | "canary" => Some(Self::ChromeCanary),
            "edge" => Some(Self::Edge),
            "edge-beta" | "edgebeta" => Some(Self::EdgeBeta),
            "edge-dev" | "edgedev" => Some(Self::EdgeDev),
            "brave" => Some(Self::Brave),
            "arc" => Some(Self::Arc),
            "chromium" => Some(Self::Chromium),
            _ => None,
        }
    }

    /// Get the display name
    pub fn display_name(&self) -> &'static str {
        match self {
            BrowserType::Firefox => "Firefox",
            BrowserType::FirefoxDeveloper => "Firefox Developer Edition",
            BrowserType::LibreWolf => "LibreWolf",
            BrowserType::Floorp => "Floorp",
            BrowserType::Waterfox => "Waterfox",
            BrowserType::Chrome => "Google Chrome",
            BrowserType::ChromeBeta => "Google Chrome Beta",
            BrowserType::ChromeCanary => "Google Chrome Canary",
            BrowserType::Edge => "Microsoft Edge",
            BrowserType::EdgeBeta => "Microsoft Edge Beta",
            BrowserType::EdgeDev => "Microsoft Edge Dev",
            BrowserType::Brave => "Brave",
            BrowserType::Arc => "Arc",
            BrowserType::Chromium => "Chromium",
        }
    }
}

/// A detected browser installation
#[derive(Debug, Clone)]
pub struct DetectedBrowser {
    pub browser_type: BrowserType,
    pub user_data_dir: PathBuf,
    pub profiles: Vec<BrowserProfile>,
}

/// A browser profile
#[derive(Debug, Clone)]
pub struct BrowserProfile {
    pub name: String,
    pub path: PathBuf,
    pub is_default: bool,
}

impl BrowserProfile {
    /// Candidate cookie database paths for Chromium browsers (newest first).
    pub fn chromium_cookie_db_candidates(&self) -> Vec<PathBuf> {
        vec![
            self.path.join("Network").join("Cookies"),
            self.path.join("Cookies"),
        ]
    }

    /// Primary cookies database path for Chromium browsers.
    pub fn cookies_db_path(&self) -> PathBuf {
        self.chromium_cookie_db_candidates()
            .into_iter()
            .find(|p| p.is_file())
            .unwrap_or_else(|| self.path.join("Network").join("Cookies"))
    }

    /// Firefox cookies.sqlite path.
    pub fn firefox_cookies_db_path(&self) -> PathBuf {
        self.path.join("cookies.sqlite")
    }

    /// Get the Local State file path (contains encryption key)
    pub fn local_state_path(&self, user_data_dir: &Path) -> PathBuf {
        user_data_dir.join("Local State")
    }
}

/// Browser detector for Windows and WSL
pub struct BrowserDetector;

impl BrowserDetector {
    /// Detect all installed browsers.
    ///
    /// On native Windows, scans standard AppData directories.
    /// On WSL, also scans Windows browser paths via /mnt/c/.
    pub fn detect_all() -> Vec<DetectedBrowser> {
        let mut browsers = Vec::new();

        for browser_type in BrowserType::all() {
            if let Some(browser) = Self::detect(*browser_type) {
                browsers.push(browser);
            }
        }

        if wsl::is_wsl() {
            let wsl_browsers = super::wsl_paths::WslBrowserDetector::detect_all();
            for wsl_browser in wsl_browsers {
                let already_found = browsers
                    .iter()
                    .any(|b| b.browser_type == wsl_browser.browser_type);
                if !already_found {
                    browsers.push(wsl_browser);
                }
            }
        }

        browsers
    }

    /// Detect a specific browser
    pub fn detect(browser_type: BrowserType) -> Option<DetectedBrowser> {
        let user_data_dir = Self::get_user_data_dir(browser_type)?;

        if !user_data_dir.exists() {
            return None;
        }

        let profiles = Self::detect_profiles(browser_type, &user_data_dir);

        if profiles.is_empty() {
            return None;
        }

        Some(DetectedBrowser {
            browser_type,
            user_data_dir,
            profiles,
        })
    }

    /// Get the user data directory for a browser
    fn get_user_data_dir(browser_type: BrowserType) -> Option<PathBuf> {
        // In WSL, prefer Windows AppData paths when available
        if wsl::is_wsl()
            && let Some(appdata_local) = wsl::windows_appdata_local()
            && let Some(path) = Self::path_for(
                browser_type,
                &appdata_local,
                wsl::windows_appdata_roaming().as_deref(),
            )
            && path.exists()
        {
            return Some(path);
        }

        let local_app_data = dirs::data_local_dir()?;
        let app_data = dirs::data_dir()?;
        Self::path_for(browser_type, &local_app_data, Some(&app_data))
    }

    fn path_for(
        browser_type: BrowserType,
        local_app_data: &Path,
        app_data: Option<&Path>,
    ) -> Option<PathBuf> {
        let path = match browser_type {
            BrowserType::Chrome => local_app_data
                .join("Google")
                .join("Chrome")
                .join("User Data"),
            BrowserType::ChromeBeta => local_app_data
                .join("Google")
                .join("Chrome Beta")
                .join("User Data"),
            BrowserType::ChromeCanary => local_app_data
                .join("Google")
                .join("Chrome SxS")
                .join("User Data"),
            BrowserType::Edge => local_app_data
                .join("Microsoft")
                .join("Edge")
                .join("User Data"),
            BrowserType::EdgeBeta => local_app_data
                .join("Microsoft")
                .join("Edge Beta")
                .join("User Data"),
            BrowserType::EdgeDev => local_app_data
                .join("Microsoft")
                .join("Edge Dev")
                .join("User Data"),
            BrowserType::Brave => local_app_data
                .join("BraveSoftware")
                .join("Brave-Browser")
                .join("User Data"),
            BrowserType::Arc => local_app_data.join("Arc").join("User Data"),
            BrowserType::Chromium => local_app_data.join("Chromium").join("User Data"),
            BrowserType::Firefox => {
                let roaming = app_data?;
                roaming.join("Mozilla").join("Firefox").join("Profiles")
            }
            BrowserType::FirefoxDeveloper => {
                // Separate install dir when present; otherwise skip so we do not
                // double-report the main Firefox Profiles folder.
                let roaming = app_data?;
                let dedicated = roaming
                    .join("Mozilla")
                    .join("Firefox Developer Edition")
                    .join("Profiles");
                if dedicated.exists() {
                    dedicated
                } else {
                    return None;
                }
            }
            BrowserType::LibreWolf => {
                let roaming = app_data?;
                let from_roaming = roaming.join("librewolf").join("Profiles");
                if from_roaming.exists() {
                    from_roaming
                } else {
                    local_app_data.join("librewolf").join("Profiles")
                }
            }
            BrowserType::Floorp => {
                let roaming = app_data?;
                roaming.join("Floorp").join("Profiles")
            }
            BrowserType::Waterfox => {
                let roaming = app_data?;
                roaming.join("Waterfox").join("Profiles")
            }
        };
        Some(path)
    }

    /// Detect profiles within a browser's user data directory
    fn detect_profiles(browser_type: BrowserType, user_data_dir: &PathBuf) -> Vec<BrowserProfile> {
        if !browser_type.is_chromium_based() {
            return Self::detect_firefox_profiles(user_data_dir);
        }

        Self::detect_chromium_profiles(user_data_dir)
    }

    /// Detect Chromium-based browser profiles via directory scan + Local State.
    fn detect_chromium_profiles(user_data_dir: &PathBuf) -> Vec<BrowserProfile> {
        let mut profiles = Vec::new();
        let mut seen = std::collections::HashSet::new();

        // Local State profile.info_cache lists named profiles (including non
        // "Profile N" folders on some installs).
        if let Some(from_state) = Self::profiles_from_local_state(user_data_dir) {
            for profile in from_state {
                if seen.insert(profile.path.clone()) {
                    profiles.push(profile);
                }
            }
        }

        // Always scan the filesystem so we do not miss profiles when Local
        // State is incomplete or locked.
        let default_path = user_data_dir.join("Default");
        if default_path.is_dir() && seen.insert(default_path.clone()) {
            profiles.push(BrowserProfile {
                name: "Default".to_string(),
                path: default_path,
                is_default: true,
            });
        }

        if let Ok(entries) = std::fs::read_dir(user_data_dir) {
            for entry in entries.flatten() {
                let name = entry.file_name().to_string_lossy().to_string();
                let path = entry.path();
                if !path.is_dir() {
                    continue;
                }
                let looks_like_profile = name == "Default"
                    || name.starts_with("Profile ")
                    || name.starts_with("Person ")
                    || (name != "System Profile"
                        && name != "Guest Profile"
                        && name != "Crashpad"
                        && name != "ShaderCache"
                        && name != "GrShaderCache"
                        && name != "GraphiteDawnCache"
                        && name != "component_crx_cache"
                        && name != "extensions_crx_cache"
                        && name != "Safe Browsing"
                        && (path.join("Network").join("Cookies").is_file()
                            || path.join("Cookies").is_file()
                            || path.join("Preferences").is_file()));
                if looks_like_profile && seen.insert(path.clone()) {
                    profiles.push(BrowserProfile {
                        name: name.clone(),
                        path,
                        is_default: name == "Default",
                    });
                }
            }
        }

        // Prefer Default / is_default first so the primary seat is tried early.
        profiles.sort_by(|a, b| {
            b.is_default
                .cmp(&a.is_default)
                .then_with(|| a.name.cmp(&b.name))
        });
        profiles
    }

    fn profiles_from_local_state(user_data_dir: &Path) -> Option<Vec<BrowserProfile>> {
        let local_state = user_data_dir.join("Local State");
        let content = std::fs::read_to_string(&local_state).ok()?;
        let json: serde_json::Value = serde_json::from_str(&content).ok()?;
        let info = json.get("profile")?.get("info_cache")?.as_object()?;
        let last_used = json
            .get("profile")
            .and_then(|p| p.get("last_used"))
            .and_then(|v| v.as_str())
            .unwrap_or("Default");

        let mut profiles = Vec::new();
        for (dir_name, meta) in info {
            let path = user_data_dir.join(dir_name);
            if !path.is_dir() {
                continue;
            }
            let display = meta
                .get("name")
                .and_then(|v| v.as_str())
                .filter(|s| !s.trim().is_empty())
                .map(|s| format!("{dir_name} ({s})"))
                .unwrap_or_else(|| dir_name.clone());
            profiles.push(BrowserProfile {
                name: display,
                path,
                is_default: dir_name == last_used || dir_name == "Default",
            });
        }
        Some(profiles)
    }

    /// Detect Firefox-family profiles
    fn detect_firefox_profiles(profiles_dir: &PathBuf) -> Vec<BrowserProfile> {
        let mut profiles = Vec::new();

        if let Ok(entries) = std::fs::read_dir(profiles_dir) {
            for entry in entries.flatten() {
                let name = entry.file_name().to_string_lossy().to_string();
                let path = entry.path();

                // Firefox profiles are named like "abcd1234.default" or
                // "abcd1234.default-release".
                if path.is_dir() && name.contains('.') {
                    let is_default = name.contains("default");
                    // Skip empty profiles without a cookies DB.
                    if !path.join("cookies.sqlite").is_file() && !path.join("prefs.js").is_file() {
                        continue;
                    }
                    profiles.push(BrowserProfile {
                        name,
                        path,
                        is_default,
                    });
                }
            }
        }

        profiles.sort_by(|a, b| {
            b.is_default
                .cmp(&a.is_default)
                .then_with(|| a.name.cmp(&b.name))
        });
        profiles
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_browser_detection() {
        let browsers = BrowserDetector::detect_all();
        println!("Detected {} browsers", browsers.len());
        for browser in &browsers {
            println!(
                "  {} ({}) at {:?} ({} profiles)",
                browser.browser_type.display_name(),
                browser.browser_type.key(),
                browser.user_data_dir,
                browser.profiles.len()
            );
            for profile in &browser.profiles {
                println!("    - {} @ {:?}", profile.name, profile.path);
            }
        }
    }

    #[test]
    fn browser_keys_roundtrip() {
        for bt in BrowserType::all() {
            assert_eq!(BrowserType::from_key(bt.key()), Some(*bt));
        }
    }

    #[test]
    fn firefox_is_scanned_before_chromium() {
        let all = BrowserType::all();
        let firefox_idx = all.iter().position(|b| *b == BrowserType::Firefox).unwrap();
        let chrome_idx = all.iter().position(|b| *b == BrowserType::Chrome).unwrap();
        assert!(firefox_idx < chrome_idx);
    }
}
