import { useCallback, useEffect, useState } from "react";
import {
  getManualCookies,
  importBrowserCookies,
  listDetectedBrowsers,
  openProviderDashboard,
  removeManualCookie,
  setManualCookie,
} from "../../../lib/tauri";
import { Select } from "../../../components/FormControls";
import { useLocale } from "../../../hooks/useLocale";
import type {
  CookieInfoBridge,
  DetectedBrowserBridge,
} from "../../../types/bridge";

interface Props {
  providerId: string;
  cookieDomain: string | null;
}

function cookiePlaceholder(
  providerId: string,
  cookieDomain: string,
  t: ReturnType<typeof useLocale>["t"],
): string {
  if (providerId === "ollama") {
    return t("BrowserCookiePlaceholderOllama");
  }
  if (providerId === "t3chat") {
    return t("BrowserCookiePlaceholderCurl");
  }
  // Show a domain-specific example of the raw header value, not the "Cookie:" label.
  return `session=…; other=${cookieDomain}…`;
}

/**
 * Per-provider browser cookie management. Renders nothing for providers
 * that do not have a cookieDomain (i.e. don't authenticate via web cookies).
 *
 * Product stance: manual paste is the recommended path on Windows Chromium
 * (App-Bound Encryption blocks automatic decrypt). Auto-import is offered as
 * a best-effort secondary action, especially useful for Firefox.
 */
export function CookieSection({ providerId, cookieDomain }: Props) {
  const { t } = useLocale();
  const [saved, setSaved] = useState<CookieInfoBridge | null>(null);
  const [loaded, setLoaded] = useState(false);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const [browsers, setBrowsers] = useState<DetectedBrowserBridge[]>([]);
  const [browsersLoaded, setBrowsersLoaded] = useState(false);
  const [browserType, setBrowserType] = useState("");
  const [importStatus, setImportStatus] = useState<string | null>(null);
  const [importError, setImportError] = useState<string | null>(null);
  const [showAutoImport, setShowAutoImport] = useState(false);

  const [pasteValue, setPasteValue] = useState("");

  const reload = useCallback(
    async (signal: { stale: boolean }) => {
      try {
        const cookies = await getManualCookies();
        if (signal.stale) return;
        setSaved(cookies.find((c) => c.providerId === providerId) ?? null);
      } catch (err: unknown) {
        if (signal.stale) return;
        setError(err instanceof Error ? err.message : String(err));
      } finally {
        if (!signal.stale) setLoaded(true);
      }
    },
    [providerId],
  );

  useEffect(() => {
    if (cookieDomain === null) return;
    const signal = { stale: false };
    setLoaded(false);
    setError(null);
    setImportError(null);
    setImportStatus(null);
    setPasteValue("");
    setSaved(null);
    setShowAutoImport(false);
    void reload(signal);
    return () => {
      signal.stale = true;
    };
  }, [reload, cookieDomain]);

  useEffect(() => {
    if (cookieDomain === null) return;
    listDetectedBrowsers()
      .then((list) => {
        setBrowsers(list);
        setBrowsersLoaded(true);
        // Prefer Firefox-family first when present (automatic works there).
        const preferred =
          list.find((b) => b.browserType.startsWith("firefox")) ??
          list.find((b) => b.browserType === "brave") ??
          list[0];
        if (preferred) setBrowserType(preferred.browserType);
      })
      .catch(() => {
        setBrowsersLoaded(true);
      });
  }, [cookieDomain]);

  if (cookieDomain === null) return null;
  if (!loaded) return null;

  const handleRemove = async () => {
    setBusy(true);
    setError(null);
    try {
      const next = await removeManualCookie(providerId);
      setSaved(next.find((c) => c.providerId === providerId) ?? null);
    } catch (err: unknown) {
      setError(err instanceof Error ? err.message : String(err));
    } finally {
      setBusy(false);
    }
  };

  const handleImport = async () => {
    if (!browserType) return;
    setBusy(true);
    setImportError(null);
    setImportStatus(null);
    try {
      const next = await importBrowserCookies(providerId, browserType);
      setSaved(next.find((c) => c.providerId === providerId) ?? null);
      setImportStatus(t("BrowserCookieImportSuccess"));
    } catch (err: unknown) {
      setImportError(err instanceof Error ? err.message : String(err));
    } finally {
      setBusy(false);
    }
  };

  const handlePaste = async () => {
    if (!pasteValue.trim()) return;
    setBusy(true);
    setError(null);
    try {
      const next = await setManualCookie(providerId, pasteValue.trim());
      setSaved(next.find((c) => c.providerId === providerId) ?? null);
      setPasteValue("");
    } catch (err: unknown) {
      setError(err instanceof Error ? err.message : String(err));
    } finally {
      setBusy(false);
    }
  };

  const handlePasteFromClipboard = async () => {
    try {
      const text = await navigator.clipboard.readText();
      if (text.trim()) {
        // Strip a leading "Cookie: " label if someone copied the whole line.
        setPasteValue(text.replace(/^\s*cookie\s*:\s*/i, "").trim());
      }
    } catch {
      setError(t("BrowserCookieClipboardDenied"));
    }
  };

  const handleOpenSite = () => {
    void openProviderDashboard(providerId).catch((e) =>
      setError(e instanceof Error ? e.message : String(e)),
    );
  };

  return (
    <section className="provider-detail-section cookie-section">
      <h4>{t("BrowserCookiesSectionTitle")}</h4>

      {error && (
        <div className="settings-status settings-status--error">{error}</div>
      )}

      {saved ? (
        <ul className="credential-list">
          <li className="credential-card">
            <div className="credential-card__header">
              <div className="credential-card__info">
                <span className="credential-card__meta">
                  <span className="credential-card__badge credential-card__badge--set">
                    {t("BrowserCookieSavedBadge")}
                  </span>
                  <span className="credential-card__date">{saved.savedAt}</span>
                </span>
              </div>
              <div className="credential-card__actions">
                <button
                  className="credential-btn credential-btn--danger"
                  disabled={busy}
                  onClick={() => void handleRemove()}
                >
                  {t("BrowserCookieRemove")}
                </button>
              </div>
            </div>
          </li>
        </ul>
      ) : (
        <p className="credential-empty">{t("BrowserCookieNoneSaved")}</p>
      )}

      {/* ── Recommended: paste Cookie header ── */}
      <div className="cookie-paste-guide">
        <div className="cookie-paste-guide__header">
          <strong>{t("BrowserCookiePasteRecommended")}</strong>
          <span className="cookie-paste-guide__badge">
            {t("BrowserCookieRecommendedBadge")}
          </span>
        </div>
        <p className="settings-section__hint cookie-paste-guide__why">
          {t("BrowserCookiePasteWhy")}
        </p>
        <ol className="cookie-paste-guide__steps">
          <li>
            {t("BrowserCookieStep1Before")}{" "}
            <code className="cookie-paste-guide__domain">{cookieDomain}</code>
            {t("BrowserCookieStep1After")}
            <div className="cookie-paste-guide__step-actions">
              <button
                type="button"
                className="credential-btn credential-btn--secondary"
                disabled={busy}
                onClick={handleOpenSite}
              >
                {t("BrowserCookieOpenSite")}
              </button>
            </div>
          </li>
          <li>{t("BrowserCookieStep2")}</li>
          <li>
            {t("BrowserCookieStep3Before")}{" "}
            <strong>{t("BrowserCookieStep3Strong")}</strong>
            {t("BrowserCookieStep3After")}
          </li>
          <li>
            {t("BrowserCookieStep4Before")}{" "}
            <code className="cookie-paste-guide__sample">
              name=value; other=value
            </code>
            {t("BrowserCookieStep4After")}
          </li>
        </ol>

        <div className="credential-add-form cookie-paste-guide__form">
          <textarea
            className="text-input credential-textarea"
            placeholder={cookiePlaceholder(providerId, cookieDomain, t)}
            rows={3}
            value={pasteValue}
            onChange={(e) => setPasteValue(e.target.value)}
            disabled={busy}
            aria-label={t("BrowserCookiePasteAreaLabel")}
          />
          <div className="cookie-paste-guide__form-actions">
            <button
              type="button"
              className="credential-btn credential-btn--secondary"
              disabled={busy}
              onClick={() => void handlePasteFromClipboard()}
            >
              {t("BrowserCookiePasteClipboard")}
            </button>
            <button
              type="button"
              className="credential-btn credential-btn--primary"
              disabled={busy || !pasteValue.trim()}
              onClick={() => void handlePaste()}
            >
              {t("BrowserCookieSave")}
            </button>
          </div>
        </div>
      </div>

      {/* ── Secondary: automatic browser import ── */}
      {browsersLoaded && browsers.length > 0 && (
        <div className="cookie-auto-import">
          <button
            type="button"
            className="cookie-auto-import__toggle"
            onClick={() => setShowAutoImport((v) => !v)}
            aria-expanded={showAutoImport}
          >
            {showAutoImport
              ? t("BrowserCookieAutoHide")
              : t("BrowserCookieAutoShow")}
          </button>
          {showAutoImport && (
            <div className="cookie-auto-import__body">
              <p className="settings-section__hint">
                {t("BrowserCookieAutoHint")}
              </p>
              {importError && (
                <div className="settings-status settings-status--error">
                  {importError}
                </div>
              )}
              {importStatus && (
                <div className="settings-status settings-status--ok">
                  {importStatus}
                </div>
              )}
              <div className="credential-add-form">
                <Select
                  value={browserType}
                  options={browsers.map((b) => ({
                    value: b.browserType,
                    label: `${b.displayName} (${b.profileCount} ${
                      b.profileCount === 1
                        ? t("BrowserCookieProfileSingular")
                        : t("BrowserCookieProfilePlural")
                    })`,
                  }))}
                  onChange={setBrowserType}
                  disabled={busy}
                />
                <button
                  className="credential-btn credential-btn--secondary"
                  disabled={busy || !browserType}
                  onClick={() => void handleImport()}
                >
                  {t("BrowserCookieImportFromBrowser")}
                </button>
              </div>
            </div>
          )}
        </div>
      )}
    </section>
  );
}
