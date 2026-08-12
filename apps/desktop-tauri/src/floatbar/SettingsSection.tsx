import { useCallback, useEffect, useMemo, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import { Field, Select, Toggle } from "../components/FormControls";
import { ProviderIcon } from "../components/providers/ProviderIcon";
import { getDirectoryAccounts, getTaskbarWidgetStatus } from "../lib/tauri";
import type {
  FloatBarOrientation,
  FloatBarContrast,
  FloatBarDensity,
  FloatBarInformationMode,
  ProviderAccountsBridge,
  SettingsSnapshot,
  SettingsUpdate,
  TaskbarWidgetStatus,
} from "../types/bridge";

/** Keep in sync with `MAX_TASKBAR_WIDGET_PROVIDERS` in taskbar_widget.rs. */
const MAX_STRIP_PROVIDERS = 5;

const PROVIDER_LABELS: Record<string, string> = {
  codex: "Codex",
  claude: "Claude",
  cursor: "Cursor",
  grok: "Grok",
  gemini: "Gemini",
  copilot: "Copilot",
  openaiapi: "OpenAI",
};

interface Props {
  settings: SettingsSnapshot;
  saving: boolean;
  set: (patch: SettingsUpdate) => void;
}

function providerLabel(id: string): string {
  return PROVIDER_LABELS[id] ?? id.charAt(0).toUpperCase() + id.slice(1);
}

/** English status line for the Taskbar Usage group; `null` hides the row. */
function taskbarWidgetStatusMessage(status: TaskbarWidgetStatus | null): string | null {
  if (!status) return null;
  switch (status.kind) {
    case "active":
      return `Shown on ${status.taskbars} taskbar${status.taskbars === 1 ? "" : "s"}.`;
    case "noFit":
      return "Hidden: no free space on the taskbar between Widgets and Start.";
    case "waitingLandmarks":
      return "Waiting for taskbar landmarks (Start button not found). A taskbar mod may be interfering.";
    case "noProviders":
      return "No enabled providers to show.";
    case "disabled":
    case "unavailable":
      return null;
  }
}

/** Providers that can track more than one config-directory account. */
const MULTI_ACCOUNT_PROVIDER_IDS = new Set(["codex", "claude"]);

function accountOptionLabel(account: {
  label: string;
  email?: string | null;
}): string {
  const email = account.email?.trim();
  if (email && email !== account.label) {
    return `${account.label} · ${email}`;
  }
  return account.label;
}

/** Enabled providers in Providers-tab order (fallback: settings.enabled list). */
function enabledProvidersInDisplayOrder(settings: SettingsSnapshot): string[] {
  const enabled = new Set(settings.enabledProviders);
  const order =
    settings.providerOrder && settings.providerOrder.length > 0
      ? settings.providerOrder
      : settings.enabledProviders;
  const ordered = order.filter((id) => enabled.has(id));
  for (const id of settings.enabledProviders) {
    if (!ordered.includes(id)) ordered.push(id);
  }
  return ordered;
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

  const [taskbarStatus, setTaskbarStatus] = useState<TaskbarWidgetStatus | null>(
    null,
  );
  useEffect(() => {
    let cancelled = false;
    const refreshTaskbarStatus = () => {
      getTaskbarWidgetStatus()
        .then((status) => {
          if (!cancelled) setTaskbarStatus(status);
        })
        .catch(() => {
          // Leave the last known status in place if the read fails.
        });
    };
    refreshTaskbarStatus();

    let unlisten: (() => void) | undefined;
    Promise.resolve(listen("taskbar-widget-status-changed", refreshTaskbarStatus))
      .then((fn) => {
        if (cancelled) {
          fn?.();
        } else {
          unlisten = fn;
        }
      })
      .catch(() => {});

    return () => {
      cancelled = true;
      unlisten?.();
    };
  }, []);
  const taskbarStatusMessage = taskbarWidgetStatusMessage(taskbarStatus);

  const [directoryAccounts, setDirectoryAccounts] = useState<
    ProviderAccountsBridge[]
  >([]);
  useEffect(() => {
    let cancelled = false;
    getDirectoryAccounts()
      .then((rows) => {
        if (!cancelled) setDirectoryAccounts(rows);
      })
      .catch(() => {
        if (!cancelled) setDirectoryAccounts([]);
      });
    return () => {
      cancelled = true;
    };
  }, [settings.enabledProviders, settings.taskbarAccountByProvider]);

  const multiAccountByProvider = useMemo(() => {
    const map = new Map<string, ProviderAccountsBridge>();
    for (const row of directoryAccounts) {
      if (
        MULTI_ACCOUNT_PROVIDER_IDS.has(row.providerId) &&
        row.accounts.length > 1
      ) {
        map.set(row.providerId, row);
      }
    }
    return map;
  }, [directoryAccounts]);

  const enabledOrdered = useMemo(
    () => enabledProvidersInDisplayOrder(settings),
    [settings],
  );
  const customStrip = (settings.floatBarProviderIds?.length ?? 0) > 0;
  const selectedStripIds = useMemo(() => {
    if (!customStrip) {
      return enabledOrdered.slice(0, MAX_STRIP_PROVIDERS);
    }
    const enabled = new Set(settings.enabledProviders);
    return settings.floatBarProviderIds.filter((id) => enabled.has(id));
  }, [customStrip, enabledOrdered, settings.enabledProviders, settings.floatBarProviderIds]);

  const commitStripIds = (ids: string[]) => {
    // Empty list restores automatic order (enabled providers, Providers tab order).
    set({ floatBarProviderIds: ids });
  };

  const toggleStripProvider = (id: string, include: boolean) => {
    const base = customStrip
      ? settings.floatBarProviderIds.filter((pid) =>
          settings.enabledProviders.includes(pid),
        )
      : enabledOrdered.slice(0, MAX_STRIP_PROVIDERS);
    if (include) {
      if (base.includes(id)) return;
      if (base.length >= MAX_STRIP_PROVIDERS) return;
      commitStripIds([...base, id]);
      return;
    }
    commitStripIds(base.filter((pid) => pid !== id));
  };

  const moveStripProvider = (id: string, delta: -1 | 1) => {
    const base = customStrip
      ? settings.floatBarProviderIds.filter((pid) =>
          settings.enabledProviders.includes(pid),
        )
      : [...selectedStripIds];
    const index = base.indexOf(id);
    if (index < 0) return;
    const next = index + delta;
    if (next < 0 || next >= base.length) return;
    const copy = [...base];
    const [row] = copy.splice(index, 1);
    copy.splice(next, 0, row);
    commitStripIds(copy);
  };

  const pinnedAccounts = settings.taskbarAccountByProvider ?? {};
  const setPinnedAccount = (providerId: string, accountId: string) => {
    const next: Record<string, string> = { ...pinnedAccounts };
    if (!accountId) {
      delete next[providerId];
    } else {
      next[providerId] = accountId;
    }
    set({ taskbarAccountByProvider: next });
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
          {settings.taskbarWidgetEnabled && taskbarStatusMessage && (
            <p className="settings-section__hint" role="status">
              {taskbarStatusMessage}
            </p>
          )}
        </div>

        <div className="settings-section__group taskbar-provider-picker">
          <div className="taskbar-provider-picker__header">
            <div>
              <div className="taskbar-provider-picker__title">Providers on the strip</div>
              <p className="settings-section__hint">
                Choose up to {MAX_STRIP_PROVIDERS} enabled providers and their order for the
                taskbar strip and floating bar. Automatic uses your Providers tab order
                (Codex, Claude, Cursor, Grok, …).
              </p>
            </div>
            {customStrip && (
              <button
                type="button"
                className="btn btn--ghost btn--sm"
                disabled={saving}
                onClick={() => commitStripIds([])}
              >
                Use automatic order
              </button>
            )}
          </div>
          {enabledOrdered.length === 0 ? (
            <p className="settings-section__hint">Enable providers on the Providers tab first.</p>
          ) : (
            <ul className="taskbar-provider-picker__list">
              {enabledOrdered.map((id) => {
                const checked = selectedStripIds.includes(id);
                const rank = selectedStripIds.indexOf(id);
                const atCap =
                  !checked && selectedStripIds.length >= MAX_STRIP_PROVIDERS;
                const multi = multiAccountByProvider.get(id);
                return (
                  <li key={id} className="taskbar-provider-picker__row">
                    <div className="taskbar-provider-picker__main">
                      <label className="taskbar-provider-picker__label">
                        <input
                          type="checkbox"
                          className="toggle"
                          checked={checked}
                          disabled={
                            saving ||
                            !settings.taskbarWidgetEnabled ||
                            (atCap && !checked)
                          }
                          aria-label={`Show ${providerLabel(id)} on taskbar strip`}
                          onChange={(e) =>
                            toggleStripProvider(id, e.target.checked)
                          }
                        />
                        <ProviderIcon
                          providerId={id}
                          size={16}
                          title={providerLabel(id)}
                        />
                        <span>{providerLabel(id)}</span>
                        {checked && rank >= 0 && (
                          <span className="taskbar-provider-picker__rank">
                            {rank + 1}
                          </span>
                        )}
                      </label>
                      <span className="providers-sidebar__reorder-controls">
                        <button
                          type="button"
                          className="providers-sidebar__reorder-button"
                          aria-label={`Move ${providerLabel(id)} up`}
                          disabled={saving || !checked || rank <= 0}
                          onClick={() => moveStripProvider(id, -1)}
                        >
                          ↑
                        </button>
                        <button
                          type="button"
                          className="providers-sidebar__reorder-button"
                          aria-label={`Move ${providerLabel(id)} down`}
                          disabled={
                            saving ||
                            !checked ||
                            rank < 0 ||
                            rank >= selectedStripIds.length - 1
                          }
                          onClick={() => moveStripProvider(id, 1)}
                        >
                          ↓
                        </button>
                      </span>
                    </div>
                    {multi && checked && (
                      <label className="taskbar-provider-picker__account">
                        <span className="taskbar-provider-picker__account-label">
                          Taskbar shows
                        </span>
                        <select
                          className="select"
                          aria-label={`Taskbar account for ${providerLabel(id)}`}
                          disabled={saving || !settings.taskbarWidgetEnabled}
                          value={pinnedAccounts[id] ?? ""}
                          onChange={(e) =>
                            setPinnedAccount(id, e.target.value)
                          }
                        >
                          <option value="">
                            Auto (closest to limit)
                          </option>
                          {multi.accounts.map((account) => (
                            <option key={account.id} value={account.id}>
                              {accountOptionLabel(account)}
                            </option>
                          ))}
                        </select>
                      </label>
                    )}
                  </li>
                );
              })}
            </ul>
          )}
          {multiAccountByProvider.size > 0 && (
            <p className="settings-section__hint">
              With two Codex or Claude accounts, pick which one the compact strip
              shows. Auto keeps the account closest to its limit. This does not
              change which account is active in the Accounts tab.
            </p>
          )}
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
          <Field
            label="Information"
            description="Exact shows the percentage. Calm shows a pace state and the next reset, with the exact percentage on click."
          >
            <Select
              value={settings.floatBarInformationMode}
              disabled={saving || !settings.floatBarEnabled}
              options={[
                { value: "exact", label: "Exact" },
                { value: "calm", label: "Calm" },
              ]}
              onChange={(v) =>
                set({ floatBarInformationMode: v as FloatBarInformationMode })
              }
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
