import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";

const { setLanguageMock } = vi.hoisted(() => ({
  setLanguageMock: vi.fn(() => Promise.resolve()),
}));

vi.mock("../../../hooks/useLocale", () => ({
  useLocale: () => ({ t: (key: string) => key, setLanguage: setLanguageMock }),
}));

const { sendTestNotificationMock } = vi.hoisted(() => ({
  sendTestNotificationMock: vi.fn(() => Promise.resolve()),
}));

vi.mock("../../../lib/tauri", () => ({
  playNotificationSound: vi.fn(() => Promise.resolve()),
  sendTestNotification: sendTestNotificationMock,
  getProviderIncidents: vi.fn(() => Promise.resolve({})),
}));

const { resetIncidentsMock } = vi.hoisted(() => ({ resetIncidentsMock: vi.fn() }));

vi.mock("../../../hooks/useProviderIncidents", () => ({
  resetProviderIncidentsCache: resetIncidentsMock,
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
  spendBudgetAlertsEnabled: false,
  spendBudgetPeriod: "daily",
  spendBudgetWarningUsd: 5,
  spendBudgetLimitUsd: 15,
  predictivePaceWarningEnabled: false,
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
  floatBarInformationMode: "exact",
  floatBarSelectionMode: "pinned",
  floatBarForegroundDetection: true,
  floatBarContrast: "auto",
  floatBarClickThrough: false,
  floatBarProviderIds: [],
    taskbarAccountByProvider: {},
  floatBarDarkText: false,
  floatBarShowResetInline: false,
  floatBarShowCost: false,
  showResetWhenExhausted: false,
};

describe("GeneralTab", () => {
  it("keeps general settings focused on startup and language", () => {
    render(<GeneralTab settings={settings} set={vi.fn()} saving={false} />);

    expect(screen.getByText("StartAtLogin")).toBeInTheDocument();
    expect(screen.getByText("StartMinimized")).toBeInTheDocument();
    expect(screen.getByText("InterfaceLanguage")).toBeInTheDocument();
    expect(screen.queryByText("RefreshIntervalLabel")).not.toBeInTheDocument();
    expect(screen.queryByText("RefreshAllProvidersOnMenuOpen")).not.toBeInTheDocument();
  });

  it("persists the interface language through settings", () => {
    const set = vi.fn();
    render(<GeneralTab settings={settings} set={set} saving={false} />);

    fireEvent.click(screen.getByRole("button", { name: "InterfaceLanguage" }));
    fireEvent.click(screen.getByRole("option", { name: "中文" }));

    expect(set).toHaveBeenCalledWith({ uiLanguage: "chinese" });
  });

  it("shows English for persisted languages without a shipped bundle", () => {
    render(
      <GeneralTab
        settings={{ ...settings, uiLanguage: "spanish" }}
        set={vi.fn()}
        saving={false}
      />,
    );

    expect(screen.getByText("English")).toBeInTheDocument();
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

  it("sends a real test notification so delivery can be verified", async () => {
    sendTestNotificationMock.mockClear();
    render(
      <GeneralTab
        mode="notifications"
        settings={settings}
        set={vi.fn()}
        saving={false}
      />,
    );

    fireEvent.click(screen.getByRole("button", { name: "NotificationTestButton" }));

    expect(sendTestNotificationMock).toHaveBeenCalledTimes(1);
    await waitFor(() =>
      expect(
        screen.getByRole("button", { name: "NotificationTestSent" }),
      ).toBeInTheDocument(),
    );
  });

  it("shows the Windows delivery error returned by the notification test", async () => {
    sendTestNotificationMock.mockRejectedValueOnce(
      "Windows notifications are turned off for Ceiling.",
    );
    render(
      <GeneralTab
        mode="notifications"
        settings={settings}
        set={vi.fn()}
        saving={false}
      />,
    );

    fireEvent.click(screen.getByRole("button", { name: "NotificationTestButton" }));

    expect(
      await screen.findByRole("alert"),
    ).toHaveTextContent("Windows notifications are turned off for Ceiling.");
    expect(
      screen.getByRole("button", { name: "NotificationTestFailed" }),
    ).toBeInTheDocument();
  });

  it("disables the test notification button when notifications are off", () => {
    render(
      <GeneralTab
        mode="notifications"
        settings={{ ...settings, showNotifications: false }}
        set={vi.fn()}
        saving={false}
      />,
    );

    expect(
      screen.getByRole("button", { name: "NotificationTestButton" }),
    ).toBeDisabled();
  });

  it("uses one global usage warning threshold", () => {
    const set = vi.fn();
    render(
      <GeneralTab mode="notifications" settings={settings} set={set} saving={false} />,
    );

    // One usage threshold, plus the budget warning, cap, and spike factor.
    expect(screen.getAllByRole("spinbutton")).toHaveLength(4);
    expect(screen.queryByText("CriticalUsageAlert")).not.toBeInTheDocument();
    expect(screen.queryByText("Codex · ProviderSession")).not.toBeInTheDocument();

    fireEvent.change(screen.getAllByRole("spinbutton")[0], { target: { value: "80" } });
    expect(set).toHaveBeenCalledWith({ highUsageThreshold: 80 });
  });

  it("offers predictive pace warnings as an opt-in", () => {
    const set = vi.fn();
    render(
      <GeneralTab mode="notifications" settings={settings} set={set} saving={false} />,
    );

    const toggle = screen.getByRole("checkbox", { name: "PredictivePaceWarnings" });
    expect(toggle).not.toBeChecked();
    fireEvent.click(toggle);
    expect(set).toHaveBeenCalledWith({ predictivePaceWarningEnabled: true });
  });

  it("disables predictive pace warnings when notifications are off", () => {
    render(
      <GeneralTab
        mode="notifications"
        settings={{ ...settings, showNotifications: false }}
        set={vi.fn()}
        saving={false}
      />,
    );

    expect(
      screen.getByRole("checkbox", { name: "PredictivePaceWarnings" }),
    ).toBeDisabled();
  });

  it("configures a daily or month-to-date estimated API value budget", () => {
    const set = vi.fn();
    render(
      <GeneralTab mode="notifications" settings={settings} set={set} saving={false} />,
    );

    fireEvent.click(screen.getByRole("checkbox", { name: "SpendBudgetAlerts" }));
    expect(set).toHaveBeenCalledWith({ spendBudgetAlertsEnabled: true });
    expect(screen.getByRole("button", { name: "SpendBudgetPeriod" })).toBeDisabled();
    expect(screen.getByRole("spinbutton", { name: "SpendBudgetWarning" })).toBeDisabled();
    expect(screen.getByRole("spinbutton", { name: "SpendBudgetCap" })).toBeDisabled();
  });

  it("offers unusual-spend alerts without requiring a budget", () => {
    const set = vi.fn();
    render(
      <GeneralTab mode="notifications" settings={settings} set={set} saving={false} />,
    );

    const toggle = screen.getByRole("checkbox", { name: "SpendAnomalyAlerts" });
    expect(toggle).not.toBeChecked();
    // The budget toggle is off, and this one must still be reachable.
    expect(toggle).not.toBeDisabled();

    fireEvent.click(toggle);
    expect(set).toHaveBeenCalledWith({ spendAnomalyAlertsEnabled: true });
    // The factor stays locked until the alert itself is on.
    expect(screen.getByRole("spinbutton", { name: "SpendAnomalyMultiplier" })).toBeDisabled();
  });

  it("edits the spike factor once unusual-spend alerts are on", () => {
    const set = vi.fn();
    render(
      <GeneralTab
        mode="notifications"
        settings={{ ...settings, spendAnomalyAlertsEnabled: true }}
        set={set}
        saving={false}
      />,
    );

    const factor = screen.getByRole("spinbutton", { name: "SpendAnomalyMultiplier" });
    expect(factor).not.toBeDisabled();
    fireEvent.change(factor, { target: { value: "5" } });
    expect(set).toHaveBeenCalledWith({ spendAnomalyMultiplier: 5 });
  });

  it("disables unusual-spend alerts when notifications are off", () => {
    render(
      <GeneralTab
        mode="notifications"
        settings={{ ...settings, showNotifications: false }}
        set={vi.fn()}
        saving={false}
      />,
    );

    expect(screen.getByRole("checkbox", { name: "SpendAnomalyAlerts" })).toBeDisabled();
  });

  it("offers incident badges independently of notifications and budgets", () => {
    const set = vi.fn();
    render(
      <GeneralTab
        mode="notifications"
        settings={{ ...settings, showNotifications: false }}
        set={set}
        saving={false}
      />,
    );

    // A badge is drawn in the UI, not raised as a toast, so switching
    // notifications off must not lock it.
    const toggle = screen.getByRole("checkbox", { name: "ProviderIncidentBadges" });
    expect(toggle).not.toBeChecked();
    expect(toggle).not.toBeDisabled();

    fireEvent.click(toggle);
    expect(set).toHaveBeenCalledWith({ providerIncidentBadgesEnabled: true });
  });

  /// The shared reading is held for fifteen minutes. Without dropping it, an
  /// off/on inside that window reapplied badges from before the toggle and
  /// never asked the backend.
  it("drops the shared incident reading when the toggle moves", () => {
    resetIncidentsMock.mockClear();
    render(
      <GeneralTab
        mode="notifications"
        settings={{ ...settings, providerIncidentBadgesEnabled: true }}
        set={vi.fn()}
        saving={false}
      />,
    );

    fireEvent.click(screen.getByRole("checkbox", { name: "ProviderIncidentBadges" }));
    expect(resetIncidentsMock).toHaveBeenCalled();
  });
});
