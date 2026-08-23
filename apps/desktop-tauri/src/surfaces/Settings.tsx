import { useCallback, useEffect, useState, type ReactElement, type ReactNode } from "react";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { getCurrentWebviewWindow } from "@tauri-apps/api/webviewWindow";
import type {
  BootstrapState,
  SettingsTabId,
  SettingsUpdate,
} from "../types/bridge";
import { useSettings } from "../hooks/useSettings";
import { useSurfaceTarget } from "../hooks/useSurfaceMode";
import { useLocale } from "../hooks/useLocale";
import { useTabListKeyboard } from "../hooks/useTabListKeyboard";
import type { LocaleKey } from "../i18n/keys";
import { closeSettingsWindow, setSurfaceMode } from "../lib/tauri";
import GeneralTab from "./settings/tabs/GeneralTab";
import DisplayTab from "./settings/tabs/DisplayTab";
import AdvancedTab from "./settings/tabs/AdvancedTab";
import AboutTab from "./settings/tabs/AboutTab";
import ProvidersTab from "./settings/tabs/ProvidersTab";
import { AccountsPanel } from "./settings/accounts/AccountsPanel";
import { CeilingMark } from "../components/CeilingMark";

// ── tab types ────────────────────────────────────────────────────────

type SettingsTab = SettingsTabId;

// Inline monochrome SVG icons stand in for the upstream macOS SF Symbols
// (gearshape / square.grid.2x2 / eye / slider.horizontal.3 / info.circle).
// They render in `currentColor` so they pick up the same secondary/accent
// text color as the tab label.
const ICON_SIZE = 16;

function Svg({ children }: { children: ReactNode }) {
  return (
    <svg
      width={ICON_SIZE}
      height={ICON_SIZE}
      viewBox="0 0 16 16"
      fill="none"
      stroke="currentColor"
      strokeWidth={1.4}
      strokeLinecap="round"
      strokeLinejoin="round"
      aria-hidden
    >
      {children}
    </svg>
  );
}

const TabIcons: Record<SettingsTab, ReactElement> = {
  general: (
    <Svg>
      <circle cx="8" cy="8" r="2" />
      <path d="M8 1.5v2M8 12.5v2M1.5 8h2M12.5 8h2M3.4 3.4l1.4 1.4M11.2 11.2l1.4 1.4M3.4 12.6l1.4-1.4M11.2 4.8l1.4-1.4" />
    </Svg>
  ),
  accounts: (
    <Svg>
      <circle cx="6" cy="5.5" r="2.5" />
      <path d="M1.5 14c0-2.5 2-4 4.5-4s4.5 1.5 4.5 4" />
      <path d="M10.5 3.4a2.5 2.5 0 0 1 0 4.2M12 10.4c1.6.5 2.5 1.8 2.5 3.6" />
    </Svg>
  ),
  providers: (
    <Svg>
      <circle cx="4" cy="4" r="1.5" />
      <circle cx="12" cy="4" r="1.5" />
      <circle cx="8" cy="12" r="1.5" />
      <path d="M5.3 4.8 7.2 10M10.7 4.8 8.8 10M5.5 4h5" />
    </Svg>
  ),
  notifications: (
    <Svg>
      <path d="M3.5 11.5h9l-1.2-1.8V7a3.3 3.3 0 0 0-6.6 0v2.7Z" />
      <path d="M6.5 13a1.7 1.7 0 0 0 3 0" />
    </Svg>
  ),
  menu: (
    <Svg>
      <rect x="2" y="2" width="5" height="5" rx="1" />
      <rect x="9" y="2" width="5" height="5" rx="1" />
      <rect x="2" y="9" width="5" height="5" rx="1" />
      <rect x="9" y="9" width="5" height="5" rx="1" />
    </Svg>
  ),
  advanced: (
    <Svg>
      <path d="M2 4h8M2 8h5M2 12h10" />
      <circle cx="11.5" cy="4" r="1.4" />
      <circle cx="8.5" cy="8" r="1.4" />
      <circle cx="13" cy="12" r="1.4" />
    </Svg>
  ),
  about: (
    <Svg>
      <circle cx="8" cy="8" r="6.25" />
      <path d="M8 7v4" />
      <circle cx="8" cy="5" r="0.6" fill="currentColor" stroke="none" />
    </Svg>
  ),
};

export const TAB_META: { id: SettingsTab; labelKey: LocaleKey }[] = [
  { id: "general", labelKey: "TabGeneral" },
  { id: "providers", labelKey: "TabProviders" },
  { id: "accounts", labelKey: "SectionAccounts" },
  { id: "notifications", labelKey: "TabNotifications" },
  { id: "menu", labelKey: "TabMenu" },
  { id: "advanced", labelKey: "TabAdvanced" },
  { id: "about", labelKey: "TabAbout" },
];

const TAB_IDS: SettingsTab[] = TAB_META.map((t) => t.id);

function isSettingsTab(value: string): value is SettingsTab {
  return TAB_META.some((t) => t.id === value);
}

export default function Settings({ state, initialTab: propTab }: { state: BootstrapState; initialTab?: string }) {
  const { settings, saving, error, update } = useSettings(state.settings);
  const { t } = useLocale();
  const shellTarget = useSurfaceTarget("settings");
  const initialTab: SettingsTab =
    propTab && isSettingsTab(propTab)
      ? propTab
      : shellTarget?.kind === "settings" && isSettingsTab(shellTarget.tab)
        ? shellTarget.tab
        : "general";
  const [activeTab, setActiveTab] = useState<SettingsTab>(initialTab);

  // Respond to prop-driven tab changes (detached window re-focus events).
  useEffect(() => {
    if (propTab && isSettingsTab(propTab)) {
      setActiveTab((current) => {
        if (current === propTab) return current;
        return propTab;
      });
    }
  }, [propTab]);

  useEffect(() => {
    if (shellTarget?.kind !== "settings" || !isSettingsTab(shellTarget.tab)) {
      return;
    }

    const nextTab: SettingsTab = shellTarget.tab;
    setActiveTab((current) => {
      if (current === nextTab) return current;
      return nextTab;
    });
  }, [shellTarget]);

  const set = (patch: SettingsUpdate) => void update(patch);
  const handleTabClick = useCallback((tab: SettingsTab) => {
    setActiveTab(tab);
    // Only transition the main window if we're NOT in the detached settings window
    if (getCurrentWebviewWindow().label !== "settings") {
      void setSurfaceMode("settings", { kind: "settings", tab });
    }
  }, []);

  const { tabListProps, getTabProps, getPanelProps } = useTabListKeyboard({
    tabIds: TAB_IDS,
    selectedId: activeTab,
    onSelect: handleTabClick,
  });

  return (
    <div
      className={`settings${activeTab === "providers" ? " settings--providers-active" : ""}`}
    >
      {/* custom title bar (decorations disabled for guaranteed dark theme) */}
      <div className="settings-titlebar" data-tauri-drag-region>
        <span
          className="settings-titlebar__title"
          data-tauri-drag-region
          style={{ display: "flex", alignItems: "center", gap: 8 }}
        >
          <CeilingMark size={16} />
          {t("SettingsWindowTitle")}
        </span>
        <div className="settings-titlebar__controls">
          <button
            className="settings-titlebar__control settings-titlebar__control--minimize"
            onClick={() => void getCurrentWindow().minimize()}
            aria-label={t("WindowMinimize")}
            title={t("WindowMinimize")}
          />
          <button
            className="settings-titlebar__control settings-titlebar__control--close"
            onClick={() => void closeSettingsWindow()}
            aria-label={t("WindowClose")}
            title={t("WindowClose")}
          >
            <svg aria-hidden viewBox="0 0 16 16" focusable="false">
              <path d="M4.5 4.5l7 7M11.5 4.5l-7 7" />
            </svg>
          </button>
        </div>
      </div>

      {/* tab bar */}
      <nav className="settings-tabs" {...tabListProps}>
        {TAB_META.map((tab) => (
          <button
            key={tab.id}
            {...getTabProps(tab.id)}
            className={`settings-tab ${activeTab === tab.id ? "settings-tab--active" : ""}`}
            onClick={() => handleTabClick(tab.id)}
          >
            <span className="settings-tab__icon">{TabIcons[tab.id]}</span>
            <span className="settings-tab__label">{t(tab.labelKey)}</span>
          </button>
        ))}
      </nav>

      {/* status bar */}
      {(saving || error) && (
        <div
          className={`settings-status ${error ? "settings-status--error" : ""}`}
        >
          {saving ? t("SettingsStatusSaving") : error}
        </div>
      )}

      {/* tab panels */}
      <div
        {...getPanelProps()}
        className={`settings-body${activeTab === "providers" ? " settings-body--providers" : ""}`}
      >
        {activeTab === "general" && (
          <GeneralTab mode="general" settings={settings} set={set} saving={saving} />
        )}
        {activeTab === "providers" && (
          <ProvidersTab
            settings={settings}
            providers={state.providers}
            set={set}
            saving={saving}
          />
        )}
        {activeTab === "accounts" && <AccountsPanel />}
        {activeTab === "notifications" && (
          <GeneralTab mode="notifications" settings={settings} set={set} saving={saving} />
        )}
        {activeTab === "menu" && (
          <DisplayTab mode="menu" settings={settings} set={set} saving={saving} />
        )}
        {activeTab === "advanced" && (
          <AdvancedTab settings={settings} set={set} saving={saving} />
        )}
        {activeTab === "about" && (
          <AboutTab settings={settings} set={set} saving={saving} />
        )}
      </div>
    </div>
  );
}

// ── Tab props shared with extracted tab components ──────────────────

export interface TabProps {
  settings: BootstrapState["settings"];
  set: (p: SettingsUpdate) => void;
  saving: boolean;
}
