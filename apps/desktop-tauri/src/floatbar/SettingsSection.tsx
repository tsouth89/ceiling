import { useCallback, useEffect, useState } from "react";
import { Field, Select, Toggle } from "../components/FormControls";
import type {
  FloatBarOrientation,
  FloatBarContrast,
  FloatBarDensity,
  SettingsSnapshot,
  SettingsUpdate,
} from "../types/bridge";

interface Props {
  settings: SettingsSnapshot;
  saving: boolean;
  set: (patch: SettingsUpdate) => void;
}

function useDraftNumber(value: number) {
  const [draft, setDraft] = useState(value);

  useEffect(() => {
    setDraft(value);
  }, [value]);

  const commit = useCallback(
    (next: number, onCommit: (value: number) => void) => {
      // Dedupe against the committed prop value, which is the persisted
      // source of truth. The parent's save is fire-and-forget, so we can't
      // observe success/failure here — comparing to `value` (rather than an
      // optimistically-advanced marker) means a failed save leaves the prop
      // unchanged and a re-commit of the same number still fires the retry.
      if (next === value) return;
      onCommit(next);
    },
    [value],
  );

  return { draft, setDraft, commit };
}

/**
 * Settings UI for the two independent at-a-glance surfaces.
 */
export default function FloatBarSettingsSection({ settings, saving, set }: Props) {
  const opacity = useDraftNumber(settings.floatBarOpacity);
  const scale = useDraftNumber(settings.floatBarScale);
  const commitOpacity = () => {
    opacity.commit(opacity.draft, (value) => set({ floatBarOpacity: value }));
  };
  const commitScale = () => {
    scale.commit(scale.draft, (value) => set({ floatBarScale: value }));
  };

  return (
    <>
      <section className="settings-section">
        <h3 className="settings-section__title">Taskbar Usage</h3>
        <div className="settings-section__group">
          <Field
            label="Show Taskbar Usage"
            description="Show live provider usage between Windows taskbar controls."
            leading
          >
            <Toggle
              checked={settings.taskbarWidgetEnabled}
              ariaLabel="Show Taskbar Usage"
              disabled={saving}
              onChange={(v) => set({ taskbarWidgetEnabled: v })}
            />
          </Field>
          <Field
            label="Open on Hover"
            description="Open the usage glance after briefly resting the pointer on the taskbar widget."
            leading
          >
            <Toggle
              checked={settings.taskbarWidgetOpenOnHover}
              ariaLabel="Open on Hover"
              disabled={saving || !settings.taskbarWidgetEnabled}
              onChange={(v) => set({ taskbarWidgetOpenOnHover: v })}
            />
          </Field>
          <Field
            label="Show on All Monitors"
            description="Mirror one usage widget onto each verified Windows taskbar."
            leading
          >
            <Toggle
              checked={settings.taskbarWidgetAllMonitors}
              ariaLabel="Show on All Monitors"
              disabled={saving || !settings.taskbarWidgetEnabled}
              onChange={(v) => set({ taskbarWidgetAllMonitors: v })}
            />
          </Field>
          <Field
            label="Show Reset Time Inline"
            description="Show the reset countdown beside each taskbar percentage when known."
            leading
          >
            <Toggle
              checked={settings.floatBarShowResetInline}
              ariaLabel="Show Reset Time Inline"
              disabled={saving || !settings.taskbarWidgetEnabled}
              onChange={(v) => set({ floatBarShowResetInline: v })}
            />
          </Field>
        </div>
      </section>

      <section className="settings-section">
        <h3 className="settings-section__title">Floating Bar</h3>
        <div className="settings-section__group">
        <Field
          label="Show Floating Bar"
          description="Show a separate always-on-top usage strip on the desktop."
          leading
        >
          <Toggle
            checked={settings.floatBarEnabled}
            ariaLabel="Show Floating Bar"
            disabled={saving}
            onChange={(v) => set({ floatBarEnabled: v })}
          />
        </Field>
          <Field
            label="Orientation"
            description="Horizontal sits above a taskbar; vertical sits on a screen edge."
          >
            <Select
              value={settings.floatBarOrientation}
              disabled={saving || !settings.floatBarEnabled}
              options={[
                { value: "horizontal", label: "Horizontal" },
                { value: "vertical", label: "Vertical" },
              ]}
              onChange={(v) => set({ floatBarOrientation: v as FloatBarOrientation })}
            />
          </Field>
          <Field
            label="Density"
            description="Choose how much information each provider segment shows."
          >
            <Select
              value={settings.floatBarDensity}
              disabled={saving || !settings.floatBarEnabled}
              options={[
                { value: "compact", label: "Compact" },
                { value: "standard", label: "Standard" },
                { value: "detailed", label: "Detailed" },
              ]}
              onChange={(v) => set({ floatBarDensity: v as FloatBarDensity })}
            />
          </Field>
          <>
            <Field
              label="Contrast"
              description="Automatic follows the Windows light or dark appearance."
            >
              <Select
                value={settings.floatBarContrast}
                disabled={saving || !settings.floatBarEnabled}
                options={[
                  { value: "auto", label: "Automatic" },
                  { value: "light-text", label: "Light text" },
                  { value: "dark-text", label: "Dark text" },
                ]}
                onChange={(v) => set({ floatBarContrast: v as FloatBarContrast })}
              />
            </Field>
            <Field
              label={`Opacity (${opacity.draft}%)`}
              description="Lower values make the bar more see-through."
            >
              <input
                type="range"
                min={30}
                max={100}
                step={5}
                value={opacity.draft}
                disabled={!settings.floatBarEnabled}
                onChange={(e) => opacity.setDraft(Number(e.target.value))}
                onPointerUp={commitOpacity}
                onTouchEnd={commitOpacity}
                onBlur={commitOpacity}
                onKeyUp={commitOpacity}
                aria-label="Floating bar opacity"
              />
            </Field>
            <Field
              label={`Size (${scale.draft}%)`}
              description="Scales the floating bar icons, text, and pill spacing."
            >
              <input
                type="range"
                min={75}
                max={200}
                step={5}
                value={scale.draft}
                disabled={!settings.floatBarEnabled}
                onChange={(e) => scale.setDraft(Number(e.target.value))}
                onPointerUp={commitScale}
                onTouchEnd={commitScale}
                onBlur={commitScale}
                onKeyUp={commitScale}
                aria-label="Floating bar size"
              />
            </Field>
          </>
          <Field
            label="Click-Through"
            description="Mouse clicks pass through to the window underneath — pure overlay mode."
            leading
          >
            <Toggle
              checked={settings.floatBarClickThrough}
              ariaLabel="Click-Through"
              disabled={saving || !settings.floatBarEnabled}
              onChange={(v) => set({ floatBarClickThrough: v })}
            />
          </Field>
        </div>
      </section>
    </>
  );
}
