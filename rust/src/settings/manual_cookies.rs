use super::*;

/// Manual cookie storage
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ManualCookies {
    /// Provider ID -> cookie header mapping
    pub cookies: HashMap<String, ManualCookieEntry>,
}

/// A single manual cookie entry
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ManualCookieEntry {
    pub cookie_header: String,
    pub saved_at: String,
}

impl ManualCookies {
    /// Get the cookies file path
    pub fn cookies_path() -> Option<PathBuf> {
        dirs::config_dir().map(|p| p.join("Ceiling").join("manual_cookies.json"))
    }

    /// Load manual cookies from disk
    pub fn load() -> Self {
        Self::try_load().unwrap_or_default()
    }

    pub(super) fn try_load() -> anyhow::Result<Self> {
        let Some(path) = Self::cookies_path() else {
            return Ok(Self::default());
        };
        if !path.exists() {
            return Ok(Self::default());
        }
        let content = crate::secure_file::read_string(&path)?;
        Ok(serde_json::from_str(&content)?)
    }

    /// Save manual cookies to disk
    pub fn save(&self) -> anyhow::Result<()> {
        let path = Self::cookies_path()
            .ok_or_else(|| anyhow::anyhow!("Could not determine cookies path"))?;

        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }

        let json = serde_json::to_string_pretty(self)?;
        crate::secure_file::write_string(&path, &json)?;

        Ok(())
    }

    /// Get cookie for a provider
    pub fn get(&self, provider_id: &str) -> Option<&str> {
        self.cookies
            .get(provider_id)
            .map(|e| e.cookie_header.as_str())
    }

    /// Set cookie for a provider
    pub fn set(&mut self, provider_id: &str, cookie_header: &str) {
        let now = chrono::Utc::now().format("%Y-%m-%d %H:%M").to_string();
        self.cookies.insert(
            provider_id.to_string(),
            ManualCookieEntry {
                cookie_header: cookie_header.to_string(),
                saved_at: now,
            },
        );
    }

    /// Remove cookie for a provider
    pub fn remove(&mut self, provider_id: &str) {
        self.cookies.remove(provider_id);
    }

    /// Get all saved cookies for UI display
    pub fn get_all_for_display(&self) -> Vec<SavedCookieInfo> {
        self.cookies
            .iter()
            .map(|(id, entry)| {
                let provider_name = ProviderId::from_cli_name(id)
                    .map(|p| p.display_name().to_string())
                    .unwrap_or_else(|| id.clone());

                SavedCookieInfo {
                    provider_id: id.clone(),
                    provider: provider_name,
                    saved_at: entry.saved_at.clone(),
                }
            })
            .collect()
    }
}

/// Info about a saved cookie for UI display
#[derive(Debug, Clone, Serialize)]
pub struct SavedCookieInfo {
    pub provider_id: String,
    pub provider: String,
    pub saved_at: String,
}
