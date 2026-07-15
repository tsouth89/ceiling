import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import type { BootstrapState, ProviderUsageSnapshot } from "../types/bridge";

const windowMocks = vi.hoisted(() => ({
  setSize: vi.fn().mockResolvedValue(undefined),
}));

vi.mock("@tauri-apps/api/window", () => ({
  getCurrentWindow: () => windowMocks,
  LogicalSize: class LogicalSize {
    constructor(public width: number, public height: number) {}
  },
}));

const tauriMocks = vi.hoisted(() => ({
  dismissTrayPanel: vi.fn().mockResolvedValue(undefined),
  getTaskbarSurfaceColor: vi.fn().mockResolvedValue("#073b78"),
  reanchorTrayPanel: vi.fn().mockResolvedValue(undefined),
  revealTrayPanelWindow: vi.fn().mockResolvedValue(undefined),
  setSurfaceMode: vi.fn().mockResolvedValue(undefined),
}));

vi.mock("../lib/tauri", () => tauriMocks);

const providerState = vi.hoisted(() => ({
  providers: [] as ProviderUsageSnapshot[],
}));

vi.mock("../hooks/useProviders", () => ({
  useProviders: () => ({
    providers: providerState.providers,
    hasLoadedCache: true,
  }),
}));

vi.mock("../hooks/useSettings", () => ({
  useSettings: (settings: unknown) => ({ settings }),
}));

import TaskbarFlyout from "./TaskbarFlyout";

function provider(
  providerId: string,
  displayName: string,
  usedPercent: number,
  resetMinutes: number,
  primaryLabel: string,
): ProviderUsageSnapshot {
  return {
    providerId,
    displayName,
    primary: {
      usedPercent,
      remainingPercent: 100 - usedPercent,
      windowMinutes: 300,
      resetsAt: new Date(Date.now() + resetMinutes * 60_000).toISOString(),
      resetDescription: null,
      isExhausted: false,
      reservePercent: null,
      reserveDescription: null,
    },
    primaryLabel,
    secondary: null,
    modelSpecific: null,
    tertiary: null,
    extraRateWindows: [],
    cost: null,
    planName: null,
    accountEmail: null,
    sourceLabel: "test",
    updatedAt: new Date().toISOString(),
    error: null,
    pace: null,
    accountOrganization: null,
    trayStatusLabel: null,
  };
}

const state = {
  providers: [
    { id: "codex", displayName: "Codex" },
    { id: "claude", displayName: "Claude" },
  ],
  settings: {
    enabledProviders: ["codex", "claude"],
    providerOrder: ["codex", "claude"],
    showAsUsed: true,
  },
} as BootstrapState;

describe("TaskbarFlyout", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    providerState.providers = [
      provider("codex", "Codex", 41, 6 * 24 * 60, "Weekly"),
      provider("claude", "Claude", 25, 212, "Session (5h)"),
    ];
  });

  it("shows at-a-glance usage and the soonest provider reset", async () => {
    providerState.providers[0].resetCreditsAvailable = 1;
    render(<TaskbarFlyout state={state} />);

    expect(screen.getByText("Ceiling")).toBeInTheDocument();
    expect(screen.getByText("Codex")).toBeInTheDocument();
    expect(screen.getByText("Claude")).toBeInTheDocument();
    expect(screen.getByText("41%")).toBeInTheDocument();
    expect(screen.getByText("25%")).toBeInTheDocument();
    expect(screen.getByText(/1 reset ready/)).toBeInTheDocument();
    expect(screen.getByText(/^Next reset in 3h/)).toBeInTheDocument();
    expect(screen.getByRole("progressbar", { name: "Claude Session (5h) 25%" })).toHaveAttribute("data-level", "normal");

    await waitFor(() => {
      expect(windowMocks.setSize).toHaveBeenCalled();
      expect(tauriMocks.reanchorTrayPanel).toHaveBeenCalled();
      expect(tauriMocks.revealTrayPanelWindow).toHaveBeenCalled();
    });
  });

  it("shows each useful allowance without promotional Cursor pools", async () => {
    const claude = provider("claude", "Claude", 9, 240, "Session (5h)");
    claude.secondary = {
      ...claude.primary,
      usedPercent: 22,
      remainingPercent: 78,
      resetsAt: new Date(Date.now() + 5 * 24 * 60 * 60_000).toISOString(),
    };
    claude.secondaryLabel = "Weekly";
    const cursor = provider("cursor", "Cursor", 85, 22 * 24 * 60, "Plan");
    cursor.secondary = { ...cursor.primary, usedPercent: 99, remainingPercent: 1 };
    cursor.secondaryLabel = "Auto";
    cursor.extraRateWindows = [
      {
        id: "cursor-api",
        title: "API",
        window: { ...cursor.primary, usedPercent: 38, remainingPercent: 62 },
      },
      {
        id: "cursor-promotional",
        title: "Promotional",
        window: { ...cursor.primary, usedPercent: 100, remainingPercent: 0 },
      },
    ];
    providerState.providers = [claude, cursor];
    const multiWindowState = {
      ...state,
      providers: [
        { id: "claude", displayName: "Claude" },
        { id: "cursor", displayName: "Cursor" },
      ],
      settings: {
        ...state.settings,
        enabledProviders: ["claude", "cursor"],
        providerOrder: ["claude", "cursor"],
      },
    } as BootstrapState;

    render(<TaskbarFlyout state={multiWindowState} />);

    expect(screen.getByRole("progressbar", { name: "Claude Session (5h) 9%" })).toBeInTheDocument();
    expect(screen.getByRole("progressbar", { name: "Claude Weekly 22%" })).toBeInTheDocument();
    expect(screen.getByRole("progressbar", { name: "Cursor Plan 85%" })).toHaveAttribute("data-level", "warning");
    expect(screen.getByRole("progressbar", { name: "Cursor Auto 99%" })).toHaveAttribute("data-level", "critical");
    expect(screen.getByRole("progressbar", { name: "Cursor API 38%" })).toHaveAttribute("data-level", "normal");
    expect(screen.queryByText("Promotional")).not.toBeInTheDocument();
  });

  it("opens the full dashboard and dismisses the glance flyout", async () => {
    render(<TaskbarFlyout state={state} />);
    fireEvent.click(screen.getByRole("button", { name: "Open Ceiling" }));

    await waitFor(() => {
      expect(tauriMocks.setSurfaceMode).toHaveBeenCalledWith("popOut", {
        kind: "dashboard",
      });
      expect(tauriMocks.dismissTrayPanel).toHaveBeenCalled();
    });
  });

  it("uses the taskbar provider selection and excludes failed data from reset claims", async () => {
    const failedCodex = provider("codex", "Codex", 99, 15, "Weekly");
    failedCodex.error = "network timeout";
    const claude = provider("claude", "Claude", 25, 212, "Session (5h)");
    claude.secondary = {
      ...claude.primary,
      usedPercent: 12,
      remainingPercent: 88,
      resetsAt: new Date(Date.now() + 90 * 60_000).toISOString(),
    };
    providerState.providers = [failedCodex, claude];
    const selectedState = {
      ...state,
      settings: {
        ...state.settings,
        floatBarProviderIds: ["codex", "claude"],
      },
    } as BootstrapState;

    render(<TaskbarFlyout state={selectedState} />);

    expect(screen.getByText("Unavailable")).toBeInTheDocument();
    expect(screen.queryByText("99%")).not.toBeInTheDocument();
    expect(screen.getByText(/^Next reset in 1h/)).toBeInTheDocument();
    await waitFor(() => expect(windowMocks.setSize).toHaveBeenCalled());
  });

  it("does not show enabled providers omitted from the taskbar selection", async () => {
    const selectedState = {
      ...state,
      settings: {
        ...state.settings,
        floatBarProviderIds: ["claude"],
      },
    } as BootstrapState;

    render(<TaskbarFlyout state={selectedState} />);

    expect(screen.getByText("Claude")).toBeInTheDocument();
    expect(screen.queryByText("Codex")).not.toBeInTheDocument();
    await waitFor(() => expect(windowMocks.setSize).toHaveBeenCalled());
  });
});
