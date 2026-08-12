import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import type { SettingsSnapshot, TaskbarWidgetStatus } from "../types/bridge";
import FloatBarSettingsSection from "./SettingsSection";

vi.mock("../hooks/useLocale", () => ({
  useLocale: () => ({ t: (key: string) => key }),
}));

const getDirectoryAccounts = vi.fn();
const getTaskbarWidgetStatus = vi.fn();
vi.mock("../lib/tauri", () => ({
  getDirectoryAccounts: () => getDirectoryAccounts(),
  getTaskbarWidgetStatus: () => getTaskbarWidgetStatus(),
}));

vi.mock("@tauri-apps/api/event", () => ({
  listen: vi.fn().mockResolvedValue(() => {}),
}));

const settings = {
  floatBarEnabled: true,
  taskbarWidgetEnabled: true,
  taskbarWidgetAllMonitors: false,
  floatBarOpacity: 90,
  floatBarScale: 100,
  floatBarOrientation: "horizontal",
  floatBarStyle: "floating",
  taskbarWidgetOpenOnHover: true,
  floatBarDensity: "standard",
  floatBarInformationMode: "exact",
  floatBarContrast: "auto",
  floatBarShowCost: false,
  floatBarShowResetInline: false,
  floatBarDarkText: false,
  floatBarClickThrough: false,
  floatBarProviderIds: [],
    taskbarAccountByProvider: {},
  enabledProviders: ["codex", "claude", "cursor", "grok"],
  providerOrder: ["codex", "claude", "cursor", "grok"],
} as unknown as SettingsSnapshot;

describe("FloatBar settings", () => {
  beforeEach(() => {
    getDirectoryAccounts.mockResolvedValue([]);
    getTaskbarWidgetStatus.mockResolvedValue({ kind: "active", taskbars: 1 });
  });

  it("does not offer the legacy API-equivalent cost toggle", () => {
    render(
      <FloatBarSettingsSection settings={settings} saving={false} set={vi.fn()} />,
    );

    expect(screen.queryByText("FloatBarShowCost")).not.toBeInTheDocument();
  });

  it("shows independent taskbar and floating bar controls", () => {
    render(
      <FloatBarSettingsSection
        settings={settings}
        saving={false}
        set={vi.fn()}
      />,
    );

    expect(screen.getByText("Taskbar Usage")).toBeInTheDocument();
    expect(screen.getByText("Show Taskbar Usage")).toBeInTheDocument();
    expect(screen.getByText("Floating Bar")).toBeInTheDocument();
    expect(screen.getByText("Show Floating Bar")).toBeInTheDocument();
    expect(screen.getByText("Open on Hover")).toBeInTheDocument();
    expect(screen.getByText("Show on All Monitors")).toBeInTheDocument();
    expect(screen.getByText("Orientation")).toBeInTheDocument();
    expect(screen.getByText("Density")).toBeInTheDocument();
    expect(screen.queryByText("Placement")).not.toBeInTheDocument();
  });

  it("persists the taskbar hover preference", () => {
    const set = vi.fn();
    render(
      <FloatBarSettingsSection
        settings={settings}
        saving={false}
        set={set}
      />,
    );

    fireEvent.click(screen.getByRole("checkbox", { name: "Open on Hover" }));
    expect(set).toHaveBeenCalledWith({ taskbarWidgetOpenOnHover: false });
  });

  it("toggles the taskbar without changing the floating bar", () => {
    const set = vi.fn();
    render(
      <FloatBarSettingsSection settings={settings} saving={false} set={set} />,
    );

    fireEvent.click(screen.getByRole("checkbox", { name: "Show Taskbar Usage" }));
    expect(set).toHaveBeenCalledWith({ taskbarWidgetEnabled: false });
    expect(set).not.toHaveBeenCalledWith(expect.objectContaining({ floatBarEnabled: false }));
  });

  it("lists enabled providers for the taskbar strip and can pin a custom order", () => {
    const set = vi.fn();
    render(
      <FloatBarSettingsSection settings={settings} saving={false} set={set} />,
    );

    expect(screen.getByText("Providers on the strip")).toBeInTheDocument();
    expect(
      screen.getByRole("checkbox", { name: "Show Grok on taskbar strip" }),
    ).toBeChecked();

    fireEvent.click(
      screen.getByRole("button", { name: "Move Grok up" }),
    );
    expect(set).toHaveBeenCalledWith({
      floatBarProviderIds: ["codex", "claude", "grok", "cursor"],
    });
  });

  it("restores automatic strip order", () => {
    const set = vi.fn();
    render(
      <FloatBarSettingsSection
        settings={{
          ...settings,
          floatBarProviderIds: ["grok", "cursor"],
        }}
        saving={false}
        set={set}
      />,
    );

    fireEvent.click(screen.getByRole("button", { name: "Use automatic order" }));
    expect(set).toHaveBeenCalledWith({ floatBarProviderIds: [] });
  });

  it("persists the all-monitors preference independently", () => {
    const set = vi.fn();
    render(
      <FloatBarSettingsSection settings={settings} saving={false} set={set} />,
    );

    fireEvent.click(screen.getByRole("checkbox", { name: "Show on All Monitors" }));
    expect(set).toHaveBeenCalledWith({ taskbarWidgetAllMonitors: true });
  });

  it("lets multi-account Codex pin which seat the strip shows", async () => {
    getDirectoryAccounts.mockResolvedValue([
      {
        providerId: "codex",
        displayName: "Codex",
        envVar: "CODEX_HOME",
        activeIndex: 0,
        followingCli: false,
        ambientDir: "C:\\codex",
        accounts: [
          {
            id: "personal-id",
            label: "Personal",
            configDir: "C:\\codex-personal",
            tint: null,
            isActive: true,
            signedIn: true,
            email: "me@home.test",
            organization: null,
            plan: "plus",
            addedAt: "Jan 1, 2026",
            lastUsed: null,
          },
          {
            id: "work-id",
            label: "Work",
            configDir: "C:\\codex-work",
            tint: null,
            isActive: false,
            signedIn: true,
            email: "me@job.test",
            organization: null,
            plan: "team",
            addedAt: "Jan 1, 2026",
            lastUsed: null,
          },
        ],
      },
    ]);
    const set = vi.fn();
    render(
      <FloatBarSettingsSection settings={settings} saving={false} set={set} />,
    );

    const select = await screen.findByRole("combobox", {
      name: "Taskbar account for Codex",
    });
    fireEvent.change(select, { target: { value: "work-id" } });
    expect(set).toHaveBeenCalledWith({
      taskbarAccountByProvider: { codex: "work-id" },
    });
  });

  it("does not show an account picker for single-account providers", async () => {
    getDirectoryAccounts.mockResolvedValue([
      {
        providerId: "codex",
        displayName: "Codex",
        envVar: "CODEX_HOME",
        activeIndex: 0,
        followingCli: false,
        ambientDir: "C:\\codex",
        accounts: [
          {
            id: "only",
            label: "Only",
            configDir: "C:\\codex",
            tint: null,
            isActive: true,
            signedIn: true,
            email: null,
            organization: null,
            plan: null,
            addedAt: "Jan 1, 2026",
            lastUsed: null,
          },
        ],
      },
    ]);
    render(
      <FloatBarSettingsSection
        settings={settings}
        saving={false}
        set={vi.fn()}
      />,
    );
    await waitFor(() => expect(getDirectoryAccounts).toHaveBeenCalled());
    expect(
      screen.queryByRole("combobox", { name: /Taskbar account/ }),
    ).not.toBeInTheDocument();
  });

  it("shows how many taskbars the widget is active on", async () => {
    getTaskbarWidgetStatus.mockResolvedValue({ kind: "active", taskbars: 2 });
    render(
      <FloatBarSettingsSection settings={settings} saving={false} set={vi.fn()} />,
    );

    expect(await screen.findByRole("status")).toHaveTextContent(
      "Shown on 2 taskbars.",
    );
  });

  it.each<[TaskbarWidgetStatus, string]>([
    [
      { kind: "noFit" },
      "Hidden: no free space on the taskbar between Widgets and Start.",
    ],
    [
      { kind: "waitingLandmarks" },
      "Waiting for taskbar landmarks (Start button not found). A taskbar mod may be interfering.",
    ],
    [{ kind: "noProviders" }, "No enabled providers to show."],
  ])("surfaces the %o status as a status row", async (status, text) => {
    getTaskbarWidgetStatus.mockResolvedValue(status);
    render(
      <FloatBarSettingsSection settings={settings} saving={false} set={vi.fn()} />,
    );

    expect(await screen.findByRole("status")).toHaveTextContent(text);
  });

  it("hides the status row when the native widget is disabled or unavailable", async () => {
    for (const status of [{ kind: "disabled" }, { kind: "unavailable" }] as const) {
      getTaskbarWidgetStatus.mockResolvedValue(status);
      const { unmount } = render(
        <FloatBarSettingsSection settings={settings} saving={false} set={vi.fn()} />,
      );

      await waitFor(() => expect(getTaskbarWidgetStatus).toHaveBeenCalled());
      expect(screen.queryByRole("status")).not.toBeInTheDocument();
      unmount();
    }
  });

  it("hides the status row when Show Taskbar Usage is off, even with an active status", async () => {
    getTaskbarWidgetStatus.mockResolvedValue({ kind: "active", taskbars: 1 });
    render(
      <FloatBarSettingsSection
        settings={{ ...settings, taskbarWidgetEnabled: false }}
        saving={false}
        set={vi.fn()}
      />,
    );

    await waitFor(() => expect(getTaskbarWidgetStatus).toHaveBeenCalled());
    expect(screen.queryByRole("status")).not.toBeInTheDocument();
  });
});
