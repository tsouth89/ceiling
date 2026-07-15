import { useCallback, useState } from "react";
import { useLocale } from "../../../hooks/useLocale";
import { playNotificationSound } from "../../../lib/tauri";
import { Field, NumberInput, Toggle } from "../../../components/FormControls";
import type { TabProps } from "../../Settings";

/**
 * Renders startup, notification, and usage-threshold settings based on the selected mode.
 *
 * @param mode - The settings category to display.
 * @param settings - The current application settings.
 * @param set - Updates one or more application settings.
 * @param saving - Whether settings are currently being saved.
 */
export default function GeneralTab({
  mode = "general",
  settings,
  set,
  saving,
}: TabProps & { mode?: "general" | "notifications" }) {
  const { t } = useLocale();
  const [playingSound, setPlayingSound] = useState(false);

  const handleTestSound = useCallback(() => {
    setPlayingSound(true);
    void playNotificationSound().catch(() => {});
    window.setTimeout(() => setPlayingSound(false), 1500);
  }, []);

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
              max={100}
              step={5}
              disabled={saving}
              onChange={(v) => set({ highUsageThreshold: v })}
            />
          </Field>
        </div>
      </section>}

    </>
  );
}
