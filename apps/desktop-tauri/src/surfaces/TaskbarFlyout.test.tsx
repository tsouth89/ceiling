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
    render(<TaskbarFlyout state={state} />);

    expect(screen.getByText("Ceiling")).toBeInTheDocument();
    expect(screen.getByText("Codex")).toBeInTheDocument();
    expect(screen.getByText("Claude")).toBeInTheDocument();
    expect(screen.getByText("41%")).toBeInTheDocument();
    expect(screen.getByText("25%")).toBeInTheDocument();
    expect(screen.getByText(/^Next reset in 3h/)).toBeInTheDocument();
    expect(screen.getByText(/^Session \(5h\) · 3h/)).toBeInTheDocument();

    await waitFor(() => {
      expect(windowMocks.setSize).toHaveBeenCalled();
      expect(tauriMocks.reanchorTrayPanel).toHaveBeenCalled();
      expect(tauriMocks.revealTrayPanelWindow).toHaveBeenCalled();
    });
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
});
