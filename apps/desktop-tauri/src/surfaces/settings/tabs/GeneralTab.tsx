import { useCallback, useState } from "react";
import { useLocale } from "../../../hooks/useLocale";
import { playNotificationSound, sendTestNotification } from "../../../lib/tauri";
import { Field, NumberInput, Select, Toggle } from "../../../components/FormControls";
import type { Language } from "../../../types/bridge";
import type { TabProps } from "../../Settings";

// Languages we ship a translation bundle for. Others fall back to English on
// the backend, so we only surface the ones that actually change the UI.
const LANGUAGE_OPTIONS: { value: Language; label: string }[] = [
  { value: "english", label: "English" },
  { value: "chinese", label: "中文" },
];

type TestNotificationStatus = "idle" | "sending" | "sent" | "failed";

export default function GeneralTab({
  mode = "general",
  settings,
  set,
  saving,
}: TabProps & { mode?: "general" | "notifications" }) {
  const { t, setLanguage } = useLocale();
  const spendBudgetAlertsEnabled = settings.spendBudgetAlertsEnabled ?? false;
  const spendBudgetPeriod = settings.spendBudgetPeriod ?? "daily";
  const spendBudgetWarningUsd = settings.spendBudgetWarningUsd ?? 5;
  const spendBudgetLimitUsd = settings.spendBudgetLimitUsd ?? 15;
  const [playingSound, setPlayingSound] = useState(false);
  const [testStatus, setTestStatus] = useState<TestNotificationStatus>("idle");
  const [testError, setTestError] = useState<string | null>(null);

  const handleTestSound = useCallback(() => {
    setPlayingSound(true);
    void playNotificationSound().catch(() => {});
    window.setTimeout(() => setPlayingSound(false), 1500);
  }, []);

  const handleTestNotification = useCallback(() => {
    setTestStatus("sending");
    setTestError(null);
    void sendTestNotification()
      .then(() => setTestStatus("sent"))
      .catch((error: unknown) => {
        setTestStatus("failed");
        setTestError(
          typeof error === "string" ? error : t("NotificationTestFailed"),
        );
      })
      .finally(() => {
        window.setTimeout(() => setTestStatus("idle"), 4000);
      });
  }, [t]);

  return (
    <>
      {mode === "general" && <section className="settings-section">
        <h3 className="settings-section__title">{t("StartupSettings")}</h3>
        <div className="settings-section__group">
          <Field label={t("StartAtLogin")} description={t("StartAtLoginHelper")} leading>
            <Toggle
              checked={settings.startAtLogin}
              disabled={saving}
              onChange={(v) => set({ startAtLogin: v })}
            />
          </Field>
          <Field
            label={t("StartMinimized")}
            description={t("StartMinimizedHelper")}
            leading
          >
            <Toggle
              checked={settings.startMinimized}
              disabled={saving}
              onChange={(v) => set({ startMinimized: v })}
            />
          </Field>
        </div>
      </section>}

      {mode === "general" && <section className="settings-section">
        <h3 className="settings-section__title">{t("SectionLanguage")}</h3>
        <div className="settings-section__group">
          <Field label={t("InterfaceLanguage")}>
            <Select
              value={settings.uiLanguage}
              options={LANGUAGE_OPTIONS}
              ariaLabel={t("InterfaceLanguage")}
              disabled={saving}
              onChange={(v) => {
                void setLanguage(v as Language);
              }}
            />
          </Field>
        </div>
      </section>}

      {mode === "notifications" && <section className="settings-section">
        <h3 className="settings-section__title">
          {t("SectionNotifications")}
        </h3>
        <div className="settings-section__group">
          <Field
            label={t("ShowNotifications")}
            description={t("ShowNotificationsHelper")}
            leading
          >
            <Toggle
              checked={settings.showNotifications}
              disabled={saving}
              onChange={(v) => set({ showNotifications: v })}
            />
          </Field>
          <Field
            label={t("CapacityEventNotifications")}
            description={t("CapacityEventNotificationsHelper")}
            leading
          >
            <Toggle
              checked={settings.capacityEventNotificationsEnabled}
              ariaLabel={t("CapacityEventNotifications")}
              disabled={saving || !settings.showNotifications}
              onChange={(v) => set({ capacityEventNotificationsEnabled: v })}
            />
          </Field>
          <Field
            label={t("NotificationTest")}
            description={t("NotificationTestHelper")}
            leading
          >
            <div className="sound-enabled-row">
              <button
                type="button"
                className="shortcut-capture__button shortcut-capture__button--ghost"
                disabled={
                  saving ||
                  !settings.showNotifications ||
                  testStatus === "sending"
                }
                onClick={handleTestNotification}
              >
                {testStatus === "sending"
                  ? t("NotificationTestSending")
                  : testStatus === "sent"
                    ? t("NotificationTestSent")
                    : testStatus === "failed"
                      ? t("NotificationTestFailed")
                      : t("NotificationTestButton")}
              </button>
            </div>
            {testError && (
              <div className="settings-status settings-status--error" role="alert">
                {testError}
              </div>
            )}
          </Field>
          <Field label={t("SoundEnabled")} description={t("SoundEnabledHelper")} leading>
            <div className="sound-enabled-row">
              <Toggle
                checked={settings.soundEnabled}
                disabled={saving}
                onChange={(v) => set({ soundEnabled: v })}
              />
              <button
                type="button"
                className="shortcut-capture__button shortcut-capture__button--ghost"
                disabled={saving || !settings.soundEnabled || playingSound}
                onClick={handleTestSound}
              >
                {playingSound
                  ? t("NotificationTestSoundPlaying")
                  : t("NotificationTestSound")}
              </button>
            </div>
          </Field>
        </div>
      </section>}

      {mode === "notifications" && <section className="settings-section">
        <h3 className="settings-section__title">
          {t("SectionUsageThresholds")}
        </h3>
        <div className="settings-section__group">
          <Field
            label={t("HighUsageAlert")}
            description={t("HighUsageWarningHelper")}
          >
            <NumberInput
              value={settings.highUsageThreshold}
              min={0}
              max={settings.criticalUsageThreshold}
              step={5}
              disabled={saving}
              onChange={(v) => set({
                highUsageThreshold: Math.min(v, settings.criticalUsageThreshold),
              })}
            />
          </Field>
        </div>
      </section>}

      {mode === "notifications" && <section className="settings-section">
        <h3 className="settings-section__title">{t("SectionSpendBudget")}</h3>
        <div className="settings-section__group">
          <Field
            label={t("SpendBudgetAlerts")}
            description={t("SpendBudgetAlertsHelper")}
            leading
          >
            <Toggle
              checked={spendBudgetAlertsEnabled}
              ariaLabel={t("SpendBudgetAlerts")}
              disabled={saving || !settings.showNotifications}
              onChange={(v) => set({ spendBudgetAlertsEnabled: v })}
            />
          </Field>
          <Field label={t("SpendBudgetPeriod")}>
            <Select
              value={spendBudgetPeriod}
              options={[
                { value: "daily", label: t("SpendBudgetDaily") },
                { value: "monthly", label: t("SpendBudgetMonthly") },
              ]}
              ariaLabel={t("SpendBudgetPeriod")}
              disabled={saving || !settings.showNotifications || !spendBudgetAlertsEnabled}
              onChange={(v) => set({ spendBudgetPeriod: v as "daily" | "monthly" })}
            />
          </Field>
          <Field label={t("SpendBudgetWarning")} description={t("SpendBudgetThresholdsHelper")}>
            <NumberInput
              value={spendBudgetWarningUsd}
              min={0}
              max={spendBudgetLimitUsd}
              step={1}
              ariaLabel={t("SpendBudgetWarning")}
              disabled={saving || !settings.showNotifications || !spendBudgetAlertsEnabled}
              onChange={(v) => set({
                spendBudgetWarningUsd: Math.min(v, spendBudgetLimitUsd),
              })}
            />
          </Field>
          <Field label={t("SpendBudgetCap")}>
            <NumberInput
              value={spendBudgetLimitUsd}
              min={spendBudgetWarningUsd}
              step={1}
              ariaLabel={t("SpendBudgetCap")}
              disabled={saving || !settings.showNotifications || !spendBudgetAlertsEnabled}
              onChange={(v) => set({
                spendBudgetLimitUsd: Math.max(v, spendBudgetWarningUsd),
              })}
            />
          </Field>
        </div>
      </section>}

    </>
  );
}
