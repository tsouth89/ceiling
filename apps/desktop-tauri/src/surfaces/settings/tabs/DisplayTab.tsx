import { useCallback, useEffect, useState } from "react";
import { useLocale } from "../../../hooks/useLocale";
import { Field, Toggle } from "../../../components/FormControls";
import type { TabProps } from "../../Settings";
import { FloatBarSettingsSection } from "../../../floatbar";

function clampWindowScalePercent(value: number): number {
  return Math.min(250, Math.max(100, Number.isFinite(value) ? value : 100));
}

/**
 * Displays the menu-related display settings and controls.
 *
 * @param mode - Determines whether the menu settings are displayed.
 * @returns The display settings interface.
 */
export default function DisplayTab({
  mode = "menu",
  settings,
  set,
  saving,
}: TabProps & { mode?: "menu" }) {
  const { t } = useLocale();
  const [windowScaleDraft, setWindowScaleDraft] = useState(() =>
    clampWindowScalePercent(settings.windowScalePercent),
  );

  useEffect(() => {
    setWindowScaleDraft(clampWindowScalePercent(settings.windowScalePercent));
  }, [settings.windowScalePercent]);

  const commitWindowScale = useCallback(() => {
    const next = clampWindowScalePercent(windowScaleDraft);
    if (next !== settings.windowScalePercent) {
      set({ windowScalePercent: next });
    }
  }, [set, settings.windowScalePercent, windowScaleDraft]);
  return (
    <>
      {/* ── Menu content ─────────────────────────────────────────── */}
      {mode === "menu" && <section className="settings-section">
        <h3 className="settings-section__title">{t("TabMenu")}</h3>
        <div className="settings-section__group">
          <Field
            label={`${t("WindowScaleLabel")} (${windowScaleDraft}%)`}
            description={t("WindowScaleHelper")}
          >
            <input
              type="range"
              min={100}
              max={250}
              step={5}
              value={windowScaleDraft}
              disabled={saving}
              onChange={(e) =>
                setWindowScaleDraft(
                  clampWindowScalePercent(Number(e.target.value)),
                )
              }
              onPointerUp={commitWindowScale}
              onTouchEnd={commitWindowScale}
              onBlur={commitWindowScale}
              onKeyUp={commitWindowScale}
              aria-label={t("WindowScaleAriaLabel")}
            />
          </Field>
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
