import { useCallback, useEffect, useState } from "react";
import {
  getManualCookies,
  removeManualCookie,
  setManualCookie,
} from "../../../lib/tauri";
import { useLocale } from "../../../hooks/useLocale";
import type { CookieInfoBridge } from "../../../types/bridge";

interface Props {
  providerId: string;
  cookieDomain: string | null;
}

function cookiePlaceholder(
  providerId: string,
  t: ReturnType<typeof useLocale>["t"],
): string {
  if (providerId === "ollama") {
    return t("BrowserCookiePlaceholderOllama");
  }
  if (providerId === "t3chat") {
    return t("BrowserCookiePlaceholderCurl");
  }
  return t("BrowserCookiePlaceholderDefault");
}

/**
 * Per-provider browser cookie management. Renders nothing for providers
 * that do not have a cookieDomain (i.e. don't authenticate via web cookies).
 */
export function CookieSection({ providerId, cookieDomain }: Props) {
  const { t } = useLocale();
  const [saved, setSaved] = useState<CookieInfoBridge | null>(null);
  const [loaded, setLoaded] = useState(false);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const [pasteValue, setPasteValue] = useState("");

  const reload = useCallback(async (signal: { stale: boolean }) => {
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
  }, [providerId]);

  useEffect(() => {
    if (cookieDomain === null) return;
    const signal = { stale: false };
    setLoaded(false);
    setError(null);
    setPasteValue("");
    setSaved(null);
    void reload(signal);
    return () => { signal.stale = true; };
  }, [reload, cookieDomain]);

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

  return (
    <section className="provider-detail-section">
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

      <div className="provider-detail-helper">
        <strong>{t("BrowserCookiePasteGuideTitle")}</strong>
        <ol>
          <li>{t("BrowserCookiePasteGuideSignIn")}</li>
          <li>{t("BrowserCookiePasteGuideDevTools")}</li>
          <li>{t("BrowserCookiePasteGuideCopy")}</li>
          <li>{t("BrowserCookiePasteGuideSave")}</li>
        </ol>
        <p>{t("BrowserCookiePasteGuidePrivacy")}</p>
      </div>

      <div className="credential-add-form">
        <textarea
          className="text-input credential-textarea"
          placeholder={cookiePlaceholder(providerId, t)}
          rows={3}
          value={pasteValue}
          onChange={(e) => setPasteValue(e.target.value)}
          disabled={busy}
        />
        <button
          className="credential-btn credential-btn--primary"
          disabled={busy || !pasteValue.trim()}
          onClick={() => void handlePaste()}
        >
          {t("BrowserCookieSave")}
        </button>
      </div>
    </section>
  );
}
