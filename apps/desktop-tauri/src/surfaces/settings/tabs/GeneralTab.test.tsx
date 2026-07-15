import { fireEvent, render, screen } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";

vi.mock("../../../hooks/useLocale", () => ({
  useLocale: () => ({ t: (key: string) => key }),
}));

import GeneralTab from "./GeneralTab";
import type { SettingsSnapshot } from "../../../types/bridge";

const settings: SettingsSnapshot = {
  enabledProviders: [],
  refreshIntervalSecs: 300,
  refreshAllProvidersOnMenuOpen: false,
  startAtLogin: false,
  startMinimized: false,
  showNotifications: true,
  capacityEventNotificationsEnabled: true,
  soundEnabled: true,
  soundVolume: 100,
  highUsageThreshold: 70,
  criticalUsageThreshold: 90,
  predictivePaceWarningEnabled: false,
  trayIconMode: "single",
  switcherShowsIcons: true,
  menuBarShowsHighestUsage: true,
  menuBarShowsPercent: true,
  showAsUsed: false,
  showAllTokenAccountsInMenu: true,
  enableAnimations: true,
  resetTimeRelative: true,
  menuBarDisplayMode: "compact",
  windowScalePercent: 125,
  trayScalePercent: 100,
  powertoysStatusPipeEnabled: false,
  hidePersonalInfo: false,
  autoDownloadUpdates: false,
  installUpdatesOnQuit: false,
  globalShortcut: "",
  codexCustomSessionsDirs: [],
  updateChannel: "stable",
  uiLanguage: "english",
  theme: "dark",
  claudeAvoidKeychainPrompts: true,
  codexSparkUsageVisible: true,
  disableKeychainAccess: false,
  providerMetrics: {},
  floatBarEnabled: false,
  taskbarWidgetEnabled: true,
  taskbarWidgetAllMonitors: false,
  floatBarOpacity: 0.9,
  floatBarScale: 100,
  floatBarOrientation: "horizontal",
  floatBarStyle: "floating",
  taskbarWidgetOpenOnHover: true,
  floatBarDensity: "standard",
  floatBarContrast: "auto",
  floatBarClickThrough: false,
  floatBarProviderIds: [],
  floatBarDarkText: false,
  floatBarShowResetInline: false,
  floatBarShowCost: false,
  showResetWhenExhausted: false,
};

describe("GeneralTab", () => {
  it("keeps general settings focused on startup behavior", () => {
    render(<GeneralTab settings={settings} set={vi.fn()} saving={false} />);

    expect(screen.getByText("StartAtLogin")).toBeInTheDocument();
    expect(screen.getByText("StartMinimized")).toBeInTheDocument();
    expect(screen.queryByText("InterfaceLanguage")).not.toBeInTheDocument();
    expect(screen.queryByText("RefreshIntervalLabel")).not.toBeInTheDocument();
    expect(screen.queryByText("RefreshAllProvidersOnMenuOpen")).not.toBeInTheDocument();
  });

  it("uses a simple sound toggle without a separate volume control", () => {
    render(
      <GeneralTab
        mode="notifications"
        settings={settings}
        set={vi.fn()}
        saving={false}
      />,
    );

    expect(screen.getByText("SoundEnabled")).toBeInTheDocument();
    expect(screen.queryByText("SoundVolume")).not.toBeInTheDocument();
  });

  it("updates the reset and capacity alert preference", () => {
    const set = vi.fn();
    render(
      <GeneralTab
        mode="notifications"
        settings={settings}
        set={set}
        saving={false}
      />,
    );

    fireEvent.click(
      screen.getByRole("checkbox", { name: "CapacityEventNotifications" }),
    );

    expect(set).toHaveBeenCalledWith({ capacityEventNotificationsEnabled: false });
  });

  it("uses one global usage warning threshold", () => {
    const set = vi.fn();
    render(
      <GeneralTab mode="notifications" settings={settings} set={set} saving={false} />,
    );

    expect(screen.getAllByRole("spinbutton")).toHaveLength(1);
    expect(screen.queryByText("PredictivePaceWarnings")).not.toBeInTheDocument();
    expect(screen.queryByText("CriticalUsageAlert")).not.toBeInTheDocument();
    expect(screen.queryByText("Codex · ProviderSession")).not.toBeInTheDocument();

    fireEvent.change(screen.getByRole("spinbutton"), { target: { value: "80" } });
    expect(set).toHaveBeenCalledWith({ highUsageThreshold: 80 });
  });
});
