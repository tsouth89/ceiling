import { useLocale } from "../../../hooks/useLocale";
import { Field, Toggle } from "../../../components/FormControls";
import type { TabProps } from "../../Settings";
import { FloatBarSettingsSection } from "../../../floatbar";

export default function DisplayTab({
  mode = "menu",
  settings,
  set,
  saving,
}: TabProps & { mode?: "menu" }) {
  const { t } = useLocale();
  return (
    <>
      {/* ── Menu content ─────────────────────────────────────────── */}
      {mode === "menu" && <section className="settings-section">
        <h3 className="settings-section__title">{t("TabMenu")}</h3>
        <div className="settings-section__group">
          <Field
            label={t("ShowAsUsedLabel")}
            description={t("ShowAsUsedHelper")}
            leading
          >
            <Toggle
              checked={settings.showAsUsed}
              disabled={saving}
              onChange={(v) => set({ showAsUsed: v })}
            />
          </Field>
          <Field
            label={t("ResetTimeRelative")}
            description={t("ResetTimeRelativeHelper")}
            leading
          >
            <Toggle
              checked={settings.resetTimeRelative}
              disabled={saving}
              onChange={(v) => set({ resetTimeRelative: v })}
            />
          </Field>
          <Field
            label={t("ShowResetWhenExhausted")}
            description={t("ShowResetWhenExhaustedHelper")}
            leading
          >
            <Toggle
              checked={settings.showResetWhenExhausted}
              ariaLabel={t("ShowResetWhenExhausted")}
              disabled={saving}
              onChange={(v) => set({ showResetWhenExhausted: v })}
            />
          </Field>
        </div>
      </section>}

      {mode === "menu" && (
        <FloatBarSettingsSection settings={settings} saving={saving} set={set} />
      )}
    </>
  );
}
