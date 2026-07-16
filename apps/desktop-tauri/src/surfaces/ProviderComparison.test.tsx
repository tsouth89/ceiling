import { render, waitFor } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import type { ProviderUsageSnapshot } from "../types/bridge";

const tauriMocks = vi.hoisted(() => ({
  getProviderChartData: vi.fn(),
}));

vi.mock("../lib/tauri", () => tauriMocks);
vi.mock("../components/providers/ProviderIcon", () => ({
  ProviderIcon: ({ providerId }: { providerId: string }) => <span>{providerId}-icon</span>,
}));

import ProviderComparison from "./ProviderComparison";

function provider(providerId: string, displayName: string): ProviderUsageSnapshot {
  return {
    providerId,
    displayName,
    primary: {
      usedPercent: 20,
      remainingPercent: 80,
      windowMinutes: 300,
      resetsAt: "2026-07-15T20:00:00Z",
      resetDescription: null,
      isExhausted: false,
      reservePercent: null,
      reserveDescription: null,
      reserveWillLastToReset: false,
      reserveEtaSeconds: null,
    },
    primaryLabel: "Session (5h)",
    secondary: null,
    modelSpecific: null,
    tertiary: null,
    extraRateWindows: [],
    inactiveRateWindows: [],
    cost: null,
    planName: null,
    accountEmail: null,
    sourceLabel: "local",
    updatedAt: new Date().toISOString(),
    error: null,
    pace: null,
    accountOrganization: null,
    trayStatusLabel: null,
    fetchDurationMs: null,
  };
}

function breakdown(processedTokens: number, outputTokens: number, cacheReadTokens: number) {
  return {
    processedTokens,
    freshInputTokens: processedTokens - outputTokens - cacheReadTokens,
    outputTokens,
    cacheReadTokens,
    cacheWriteTokens: 0,
  };
}

function chartData(providerId: string, fiveHours: number, sevenDays: number) {
  return {
    providerId,
    costHistory: [],
    creditsHistory: [],
    usageBreakdown: [],
    quotaHistory: [],
    localUsage: {
      comparisonPeriods: [
        {
          id: "five-hours",
          label: "Last 5 hours",
          currentTokens: fiveHours,
          currentBreakdown: breakdown(fiveHours, 2_000_000, fiveHours / 2),
          previousTokens: fiveHours / 2,
          previousBreakdown: breakdown(fiveHours / 2, 1_000_000, fiveHours / 4),
        },
        {
          id: "seven-days",
          label: "Last 7 days",
          currentTokens: sevenDays,
          currentBreakdown: breakdown(sevenDays, 20_000_000, sevenDays / 2),
          previousTokens: sevenDays,
          previousBreakdown: breakdown(sevenDays, 20_000_000, sevenDays / 2),
        },
      ],
    },
  };
}

describe("ProviderComparison", () => {
  it("compares Codex and Claude over identical rolling periods", async () => {
    tauriMocks.getProviderChartData.mockImplementation((providerId: string) =>
      Promise.resolve(providerId === "codex"
        ? chartData("codex", 40_000_000, 2_000_000_000)
        : chartData("claude", 20_000_000, 4_000_000_000))
    );

    const { getByText, getAllByText } = render(
      <ProviderComparison
        providers={[provider("codex", "Codex"), provider("claude", "Claude")]}
      />,
    );

    await waitFor(() => expect(getByText("Codex and Claude, on the same clock")).toBeTruthy());
    expect(getByText("Last 5 hours")).toBeTruthy();
    expect(getByText("Last 7 days")).toBeTruthy();
    expect(getByText("Codex processed 2.0× more")).toBeTruthy();
    expect(getByText("Claude processed 2.0× more")).toBeTruthy();
    expect(getAllByText("+100% vs prior")).toHaveLength(2);
    expect(getAllByText("50.0% cache").length).toBeGreaterThan(0);
    expect(tauriMocks.getProviderChartData).toHaveBeenCalledTimes(2);
  });
});
