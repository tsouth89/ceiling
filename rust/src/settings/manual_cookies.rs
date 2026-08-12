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
    /// Apply a mutation while holding the shared state lock across load and
    /// save, so another process cannot overwrite this update with stale data.
    pub fn update(operation: impl FnOnce(&mut Self)) -> anyhow::Result<Self> {
        let path = Self::cookies_path()
            .ok_or_else(|| anyhow::anyhow!("Could not determine cookies path"))?;
        crate::secure_file::with_state_write_lock(|| {
            let mut cookies = Self::load_update_from(&path)?;
            operation(&mut cookies);
            cookies.save_to(&path).map_err(std::io::Error::other)?;
            Ok(cookies)
        })
        .map_err(Into::into)
    }

    /// Get the cookies file path
    pub fn cookies_path() -> Option<PathBuf> {
        dirs::config_dir().map(|p| p.join("Ceiling").join("manual_cookies.json"))
    }

    /// Load manual cookies from disk, treating an unreadable store as empty.
    ///
    /// Read-only callers only. Anything that mutates and then calls
    /// [`save`](Self::save) must use [`load_for_update`](Self::load_for_update).
    pub fn load() -> Self {
        Self::try_load().unwrap_or_default()
    }

    /// Load the store fallibly for read-only status/diagnostic callers.
    ///
    /// Unlike [`load`](Self::load), this preserves an unreadable result so a
    /// caller cannot mistake decode failure for provider credential absence.
    pub fn try_load_for_read() -> anyhow::Result<Self> {
        Self::try_load()
    }

    /// Load manual cookies for a read-modify-write cycle, failing closed.
    ///
    /// See [`ApiKeys::load_for_update`] — `save` replaces the whole file, so a
    /// store that silently decoded as empty would take every other provider's
    /// cookie with it on the next write (SBS-623).
    pub fn load_for_update() -> anyhow::Result<Self> {
        Self::try_load().map_err(|error| {
            // Generic for the same reason as `ApiKeys::load_for_update`: the
            // decode error can quote decrypted cookie material and this string
            // reaches the frontend.
            tracing::warn!(%error, "Saved manual cookies could not be decoded");
            anyhow::anyhow!(
                "Saved manual cookies could not be read. Refusing to write, which \
                 would replace the stored cookies with only this change. See the \
                 log for details."
            )
        })
    }

    pub(super) fn try_load() -> anyhow::Result<Self> {
        let Some(path) = Self::cookies_path() else {
            return Ok(Self::default());
        };
        Self::try_load_from(&path)
    }

    pub(super) fn try_load_from(path: &std::path::Path) -> anyhow::Result<Self> {
        if !path.exists() {
            return Ok(Self::default());
        }
        let content = crate::secure_file::read_string(path)?;
        Ok(serde_json::from_str(&content)?)
    }

    /// Save manual cookies to disk
    pub fn save(&self) -> anyhow::Result<()> {
        let path = Self::cookies_path()
            .ok_or_else(|| anyhow::anyhow!("Could not determine cookies path"))?;

        self.save_to(&path)
    }

    fn load_update_from(path: &std::path::Path) -> std::io::Result<Self> {
        Self::try_load_from(path).map_err(|error| {
            tracing::warn!(%error, "Saved manual cookies could not be decoded");
            std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "Saved manual cookies could not be read; refusing to replace them",
            )
        })
    }

    pub(super) fn save_to(&self, path: &std::path::Path) -> anyhow::Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }

        let json = serde_json::to_string_pretty(self)?;
        crate::secure_file::write_string(path, &json)?;

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
