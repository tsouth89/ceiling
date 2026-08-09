import { useCallback, useEffect, useState } from "react";
import {
  getManualCookies,
  removeManualCookie,
  setManualCookie,
} from "../../../lib/tauri";
import { Select } from "../../../components/FormControls";
import type {
  CookieInfoBridge,
  ProviderCatalogEntry,
} from "../../../types/bridge";

export default function CookiesTab({ providers }: { providers: ProviderCatalogEntry[] }) {
  const [cookies, setCookies] = useState<CookieInfoBridge[]>([]);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  // Add-cookie form state
  const [addProviderId, setAddProviderId] = useState("");
  const [addCookieValue, setAddCookieValue] = useState("");

  const reload = useCallback(async () => {
    try {
      setCookies(await getManualCookies());
    } catch (err: unknown) {
      setError(err instanceof Error ? err.message : String(err));
    }
  }, []);

  useEffect(() => {
    void reload();
  }, [reload]);

  // Only show providers with a cookie domain
  const cookieProviders = providers.filter((p) => p.cookieDomain !== null);

  const handleAdd = async () => {
    if (!addProviderId || !addCookieValue.trim()) return;
    setBusy(true);
    setError(null);
    try {
      const next = await setManualCookie(addProviderId, addCookieValue.trim());
      setCookies(next);
      setAddProviderId("");
      setAddCookieValue("");
    } catch (err: unknown) {
      setError(err instanceof Error ? err.message : String(err));
    } finally {
      setBusy(false);
    }
  };

  const handleRemove = async (providerId: string) => {
    setBusy(true);
    setError(null);
    try {
      const next = await removeManualCookie(providerId);
      setCookies(next);
    } catch (err: unknown) {
      setError(err instanceof Error ? err.message : String(err));
    } finally {
      setBusy(false);
    }
  };

  return (
    <section className="settings-section">
      <h3 className="settings-section__title">Saved Cookies</h3>
      <p className="settings-section__hint">
        Saved cookie headers for browser-authenticated providers. Ceiling does
        not scan browser databases; you stay in control of exactly what is saved.
      </p>

      {error && (
        <div className="settings-status settings-status--error">{error}</div>
      )}

      {cookies.length > 0 ? (
        <ul className="credential-list">
          {cookies.map((c) => (
            <li key={c.providerId} className="credential-card">
              <div className="credential-card__header">
                <div className="credential-card__info">
                  <strong>{c.provider}</strong>
                  <span className="credential-card__meta">
                    <span className="credential-card__badge credential-card__badge--set">
                      Saved
                    </span>
                    <span className="credential-card__date">
                      {c.savedAt}
                    </span>
                  </span>
                </div>
                <div className="credential-card__actions">
                  <button
                    className="credential-btn credential-btn--danger"
                    disabled={busy}
                    onClick={() => void handleRemove(c.providerId)}
                  >
                    Remove
                  </button>
                </div>
              </div>
            </li>
          ))}
        </ul>
      ) : (
        <p className="credential-empty">No manual cookies saved.</p>
      )}

      <h3 className="settings-section__title">Copy a cookie header</h3>
      <ol className="settings-section__hint">
        <li>Sign in to the provider in your normal browser.</li>
        <li>Press F12, open Network, then refresh the provider page.</li>
        <li>Select an authenticated request and copy the Cookie request-header value.</li>
        <li>Paste the value below and save it. Never share it—it grants account access.</li>
      </ol>
      <div className="credential-add-form">
        <Select
          value={addProviderId}
          options={[
            { value: "", label: "Select provider…" },
            ...cookieProviders.map((p) => ({
              value: p.id,
              label: p.displayName,
            })),
          ]}
          onChange={setAddProviderId}
          disabled={busy}
        />
        <textarea
          className="text-input credential-textarea"
          placeholder="Paste cookie header value…"
          rows={3}
          value={addCookieValue}
          onChange={(e) => setAddCookieValue(e.target.value)}
          disabled={busy}
        />
        <button
          className="credential-btn credential-btn--primary"
          disabled={busy || !addProviderId || !addCookieValue.trim()}
          onClick={() => void handleAdd()}
        >
          Save Cookie
        </button>
      </div>
    </section>
  );
}
