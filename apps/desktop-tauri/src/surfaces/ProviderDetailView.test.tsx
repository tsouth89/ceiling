import { render, screen, waitFor } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import type { ProviderUsageSnapshot, RateWindowSnapshot } from "../types/bridge";
import ProviderDetailView from "./ProviderDetailView";

const tauriMocks = vi.hoisted(() => ({
  getProviderChartData: vi.fn(),
}));

vi.mock("../lib/tauri", () => tauriMocks);
vi.mock("../hooks/useLocale", () => ({
  useLocale: () => ({ t: (key: string) => key }),
}));

function rate(usedPercent: number): RateWindowSnapshot {
  return {
    usedPercent,
    remainingPercent: 100 - usedPercent,
    windowMinutes: 7 * 24 * 60,
    resetsAt: null,
    resetDescription: "in 6d",
    isExhausted: false,
    reservePercent: null,
    reserveDescription: null,
  };
}

function codex(): ProviderUsageSnapshot {
  return {
    providerId: "codex",
    displayName: "Codex",
    primary: rate(51),
    primaryLabel: "Weekly",
    secondary: null,
    modelSpecific: null,
    tertiary: null,
    extraRateWindows: [
      { id: "spark", title: "Codex Spark", window: rate(0) },
      { id: "promo", title: "Promotional", window: rate(100) },
    ],
    inactiveRateWindows: [
      {
        id: "session",
        title: "5-hour session",
        description: "Not currently enforced by OpenAI",
        state: "notEnforced",
      },
      {
        id: "weekly",
        title: "Weekly",
        description: "Not reported in the latest update",
        state: "unavailable",
      },
    ],
    cost: null,
    planName: "Pro Lite",
    accountEmail: null,
    sourceLabel: "local",
    updatedAt: new Date().toISOString(),
    error: null,
    pace: {
      windowLabel: "Weekly",
      stage: "far_ahead",
      deltaPercent: 38.6,
      willLastToReset: false,
      etaSeconds: 3600,
      expectedUsedPercent: 12.4,
      actualUsedPercent: 51,
    },
    accountOrganization: null,
    trayStatusLabel: null,
    fetchDurationMs: 10,
    resetCreditsAvailable: 1,
  };
}

describe("ProviderDetailView", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    tauriMocks.getProviderChartData.mockResolvedValue({
      providerId: "codex",
      costHistory: [],
      creditsHistory: [],
      usageBreakdown: [],
      quotaHistory: [],
      localUsage: {
        todayCost: null,
        lastSessionCost: null,
        lastSessionTokens: 542_500_000,
        sevenDayCost: null,
        sevenDayTokens: 2_600_000_000,
        thirtyDayCost: null,
        thirtyDayTokens: 2_600_000_000,
        latestTokens: null,
        topModel: "gpt-5.6-sol",
        estimateNote: "",
        tokenCostUpdatedAtMs: 0,
        sevenDayTokenBreakdown: {
          processedTokens: 1_000,
          freshInputTokens: 20,
          outputTokens: 20,
          cacheReadTokens: 960,
          cacheWriteTokens: 0,
        },
      },
    });
  });

  it("presents the primary limit, quiet secondary limits, and accurate pace", async () => {
    const { container } = render(
      <ProviderDetailView
        provider={codex()}
        resetTimeRelative
        showAsUsed
      />,
    );

    expect(screen.getByText("Weekly usage")).toBeInTheDocument();
    expect(screen.getByText(/1 reset available/)).toBeInTheDocument();
    expect(screen.getByText("51%")).toBeInTheDocument();
    expect(screen.getByText("Codex Spark")).toBeInTheDocument();
    expect(screen.getByText("5-hour session")).toBeInTheDocument();
    expect(screen.getByText("Not currently enforced")).toBeInTheDocument();
    // A window that dropped out of a successful response reads as Unavailable,
    // never as "not currently enforced" or a fabricated percentage.
    expect(screen.getByText("Unavailable")).toBeInTheDocument();
    expect(screen.queryByText("Promotional")).toBeNull();
    expect(screen.getAllByText(/Weekly pace/)).toHaveLength(2);
    expect(screen.getAllByText(/Far ahead of budget/)).toHaveLength(2);
    expect(container.querySelector(".provider-focus__pace-fill")?.getAttribute("data-tone"))
      .toBe("risk");

    await waitFor(() => expect(screen.getByText("542.5M")).toBeInTheDocument());
    expect(screen.getByText("96.0%")).toBeInTheDocument();
    expect(screen.getByText(/Most used model: gpt-5.6-sol/)).toBeInTheDocument();
  });
});
