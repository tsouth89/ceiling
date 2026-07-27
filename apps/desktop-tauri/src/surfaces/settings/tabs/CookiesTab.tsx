import { useCallback, useEffect, useState } from "react";
import {
  getManualCookies,
  importBrowserCookies,
  listDetectedBrowsers,
  removeManualCookie,
  setManualCookie,
} from "../../../lib/tauri";
import { Select } from "../../../components/FormControls";
import type {
  CookieInfoBridge,
  DetectedBrowserBridge,
  ProviderCatalogEntry,
} from "../../../types/bridge";

export default function CookiesTab({ providers }: { providers: ProviderCatalogEntry[] }) {
  const [cookies, setCookies] = useState<CookieInfoBridge[]>([]);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  // Browser import state
  const [browsers, setBrowsers] = useState<DetectedBrowserBridge[]>([]);
  const [browsersLoaded, setBrowsersLoaded] = useState(false);
  const [importProviderId, setImportProviderId] = useState("");
  const [importBrowserType, setImportBrowserType] = useState("");
  const [importStatus, setImportStatus] = useState<string | null>(null);
  const [importError, setImportError] = useState<string | null>(null);

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

  // Lazy-load browser list on first render
  useEffect(() => {
    listDetectedBrowsers()
      .then((list) => {
        setBrowsers(list);
        setBrowsersLoaded(true);
        if (list.length > 0) setImportBrowserType(list[0].browserType);
      })
      .catch(() => {
        setBrowsersLoaded(true);
      });
  }, []);

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

  const handleBrowserImport = async () => {
    if (!importProviderId || !importBrowserType) return;
    setBusy(true);
    setImportError(null);
    setImportStatus(null);
    try {
      const next = await importBrowserCookies(importProviderId, importBrowserType);
      setCookies(next);
      setImportStatus("Cookies imported successfully.");
      setImportProviderId("");
    } catch (err: unknown) {
      setImportError(err instanceof Error ? err.message : String(err));
    } finally {
      setBusy(false);
    }
  };

  return (
    <section className="settings-section">
      <h3 className="settings-section__title">Saved Cookies</h3>
      <p className="settings-section__hint">
        Paste a Cookie header from the provider site (recommended on Chrome,
        Edge, and Brave). Open DevTools (F12) → Network → refresh → click any
        request → copy the value of the Cookie request header only (not the
        word Cookie:). Per-provider steps live under Providers → provider
        detail.
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

      {/* ── Browser import ── */}
      {browsersLoaded && browsers.length > 0 && (
        <>
          <h3 className="settings-section__title">Import from Browser</h3>
          <p className="settings-section__hint">
            Extract cookies automatically from a signed-in browser.
            The browser must be installed on this machine and you must be
            signed in to the provider in that browser.
          </p>

          {importError && (
            <div className="settings-status settings-status--error">{importError}</div>
          )}
          {importStatus && (
            <div className="settings-status settings-status--ok">{importStatus}</div>
          )}

          <div className="credential-add-form">
            <Select
              value={importProviderId}
              options={[
                { value: "", label: "Select provider…" },
                ...cookieProviders.map((p) => ({
                  value: p.id,
                  label: p.displayName,
                })),
              ]}
              onChange={setImportProviderId}
              disabled={busy}
            />
            <Select
              value={importBrowserType}
              options={browsers.map((b) => ({
                value: b.browserType,
                label: `${b.displayName} (${b.profileCount} profile${b.profileCount !== 1 ? "s" : ""})`,
              }))}
              onChange={setImportBrowserType}
              disabled={busy}
            />
            <button
              className="credential-btn credential-btn--primary"
              disabled={busy || !importProviderId || !importBrowserType}
              onClick={() => void handleBrowserImport()}
            >
              Import Cookies
            </button>
          </div>
        </>
      )}

      {browsersLoaded && browsers.length === 0 && (
        <>
          <h3 className="settings-section__title">Import from Browser</h3>
          <p className="settings-section__hint">
            No supported browsers detected on this machine, or automatic cookie
            extraction is unavailable (requires Windows with Chrome, Edge, Brave,
            or Firefox installed). Use the manual paste form below instead.
          </p>
        </>
      )}

      <h3 className="settings-section__title">Add Cookie Manually</h3>
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
