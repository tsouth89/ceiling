import io

def norm(t):
    return t.replace("\r\n", "\n")

def patch(path, pairs, expect=1):
    s = io.open(path, encoding="utf-8", newline="").read()
    nl = "\r\n" if "\r\n" in s[:4000] else "\n"
    for old, new in pairs:
        o = norm(old).replace("\n", nl)
        n = norm(new).replace("\n", nl)
        c = s.count(o)
        assert c == expect, "%d matches for %r in %s" % (c, old[:70], path)
        s = s.replace(o, n)
    io.open(path, "w", encoding="utf-8", newline="").write(s)
    print("patched", path)

# A store that will not decode is not a revoked credential.
patch("rust/src/settings.rs", [
("""/// The body of [`revoke_managed_credentials`], with the paths and the""",
"""/// Whether `provider` still has a credential in Preferences.
///
/// An unreadable store answers `true`. This exists for the background refresh
/// paths, which ask "was this revoked while I was working" before writing a
/// renewed token, and [`ApiKeys::load`] cannot tell an empty store from one
/// that failed to decode. Reading a decode failure as a revoke would drop a
/// renewed token and leave the session on a credential the provider may have
/// already rotated away from.
pub(crate) fn provider_credential_present(provider: ProviderId) -> bool {
    match ApiKeys::try_load() {
        Ok(keys) => keys.has_key(provider.cli_name()),
        Err(error) => {
            tracing::warn!("Could not read stored API keys ({error}); treating as still signed in");
            true
        }
    }
}

/// The body of [`revoke_managed_credentials`], with the paths and the"""),
])

patch("rust/src/settings/api_keys.rs", [
("    pub(super) fn try_load() -> anyhow::Result<Self> {",
 "    pub(crate) fn try_load() -> anyhow::Result<Self> {"),
])

# Inject the check so the production path itself is testable.
patch("rust/src/providers/stepfun/mod.rs", [
("""fn persist_refreshed_token_in(store: &impl TokenSecretStore, token: &str) {
    let locked = crate::secure_file::with_state_write_lock(|| {
        Ok(persist_refreshed_token_when(
            store,
            token,
            stepfun_credential_configured(),
        ))
    });
    if let Err(error) = locked {
        tracing::debug!("Could not take the state lock to persist refreshed StepFun token: {error}");
    }
}""",
"""fn persist_refreshed_token_in(store: &impl TokenSecretStore, token: &str) {
    persist_refreshed_token_checked(store, token, stepfun_credential_configured)
}

/// Persist under the state lock, asking `configured` while holding it.
///
/// The predicate is a parameter so a test can drive the real locked path,
/// rather than only the decision it reaches.
fn persist_refreshed_token_checked(
    store: &impl TokenSecretStore,
    token: &str,
    configured: impl FnOnce() -> bool,
) {
    let locked = crate::secure_file::with_state_write_lock(|| {
        Ok(persist_refreshed_token_when(store, token, configured()))
    });
    if let Err(error) = locked {
        tracing::debug!("Could not take the state lock to persist refreshed StepFun token: {error}");
    }
}"""),
])

patch("rust/src/providers/stepfun/mod.rs", [
("""fn stepfun_credential_configured() -> bool {
    if crate::settings::ApiKeys::load().has_key(crate::core::ProviderId::StepFun.cli_name()) {
        return true;
    }""",
"""fn stepfun_credential_configured() -> bool {
    if crate::settings::provider_credential_present(crate::core::ProviderId::StepFun) {
        return true;
    }"""),
])

patch("rust/src/providers/stepfun/mod.rs", [
("""    #[test]
    fn a_refresh_for_a_live_credential_is_still_persisted() {""",
"""    /// The locked path itself, not just the decision it reaches.
    ///
    /// Without this, dropping the lock or hard-coding the check to true would
    /// leave every other test here green while the race this closes came back.
    #[test]
    fn the_locked_persist_path_writes_nothing_for_a_revoked_credential() {
        let store = MemoryTokenSecretStore::new();

        persist_refreshed_token_checked(&store, "refreshed-after-revoke", || false);

        let error = resolve_token_in(&store, None, STEPFUN_CREDENTIAL_TARGET, &[])
            .expect_err("a revoked credential must leave the keyring empty");
        assert!(matches!(error, ProviderError::NotInstalled(_)));
    }

    #[test]
    fn the_locked_persist_path_writes_for_a_live_credential() {
        let store = MemoryTokenSecretStore::new();

        persist_refreshed_token_checked(&store, "fresh-token", || true);

        assert_eq!(
            resolve_token_in(&store, None, STEPFUN_CREDENTIAL_TARGET, &[]).unwrap(),
            "fresh-token"
        );
    }

    #[test]
    fn a_refresh_for_a_live_credential_is_still_persisted() {"""),
])
