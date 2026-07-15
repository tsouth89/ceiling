import { render, waitFor } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";

const tauriMocks = vi.hoisted(() => ({
  getProviderChartData: vi.fn(),
  getSettingsSnapshot: vi.fn(),
}));

vi.mock("../../../../../lib/tauri", () => tauriMocks);

import { ChartsSection } from "./ChartsSection";

const enrichedData = {
  providerId: "claude",
  costHistory: [],
  creditsHistory: [],
  usageBreakdown: [],
  quotaHistory: [],
  localUsage: {
    todayCost: 30,
    lastSessionCost: 12.5,
    lastSessionTokens: 2_100_000,
    sevenDayCost: 3427.91,
    sevenDayTokens: 4_949_300_000,
    thirtyDayCost: 17_700,
    thirtyDayTokens: 23_550_000_000,
    currentWindows: [
      {
        id: "primary",
        label: "Current 5h window",
        startsAt: new Date().toISOString(),
        endsAt: new Date(Date.now() + 3_600_000).toISOString(),
        tokens: 18_400_000,
        tokenBreakdown: {
          processedTokens: 18_400_000,
          freshInputTokens: 100_000,
          outputTokens: 300_000,
          cacheReadTokens: 18_000_000,
          cacheWriteTokens: 0,
        },
      },
      {
        id: "secondary",
        label: "Current weekly window",
        startsAt: new Date(Date.now() - 4 * 24 * 3_600_000).toISOString(),
        endsAt: new Date(Date.now() + 3 * 24 * 3_600_000).toISOString(),
        tokens: 843_400_000,
        tokenBreakdown: {
          processedTokens: 843_400_000,
          freshInputTokens: 1_000_000,
          outputTokens: 4_000_000,
          cacheReadTokens: 838_400_000,
          cacheWriteTokens: 0,
        },
      },
    ],
    comparisonPeriods: [],
    latestTokens: 2_100_000,
    topModel: "claude-opus-4-8",
    estimateNote: "API-equivalent estimate from local logs; not subscription spend",
    tokenCostUpdatedAtMs: 1,
    sevenDayTokenBreakdown: {
      processedTokens: 4_949_300_000,
      freshInputTokens: 2_048_000,
      outputTokens: 14_129_000,
      cacheReadTokens: 4_814_540_000,
      cacheWriteTokens: 118_583_000,
    },
  },
};

describe("ChartsSection local usage summary", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    tauriMocks.getSettingsSnapshot.mockResolvedValue({ enableAnimations: false });
    tauriMocks.getProviderChartData.mockResolvedValue(enrichedData);
  });

  it("shows comparable processed totals and the seven-day token mix", async () => {
    const { getByText, getAllByText, getByLabelText } = render(
      <ChartsSection providerId="claude" accountEmail={null} t={(key) => key} />,
    );

    await waitFor(() => expect(getByText("4.9B")).toBeTruthy());
    expect(getByText("23.6B")).toBeTruthy();
    expect(getByText("Current 5h window")).toBeTruthy();
    expect(getByText("Current weekly window")).toBeTruthy();
    expect(getByText("18.4M")).toBeTruthy();
    expect(() => getByText("Last session")).toThrow();
    expect(getByText("99.7% cache traffic")).toBeTruthy();
    expect(getAllByText("processed tokens")).toHaveLength(2);
    expect(() => getByText("$3,427.91")).toThrow();
    expect(getByLabelText("Local usage summary").getAttribute("data-card-count")).toBe("4");

    const mix = getByLabelText("Last 7 days token breakdown");
    expect(mix.textContent).toContain("Fresh input2M");
    expect(mix.textContent).toContain("Output14.1M");
    expect(mix.textContent).toContain("Cache read4.8B");
    expect(mix.textContent).toContain("Cache write118.6M");
  });

  it("keeps quota history visible while local history loads, then enriches promptly", async () => {
    tauriMocks.getProviderChartData
      .mockResolvedValueOnce({
        ...enrichedData,
        localUsage: null,
        quotaHistory: [
          {
            recordedAt: "2026-07-14T15:51:00Z",
            windows: [{ id: "weekly", label: "Weekly", usedPercent: 5 }],
          },
        ],
      })
      .mockResolvedValue(enrichedData);

    const { getByRole, getByText } = render(
      <ChartsSection providerId="claude" accountEmail={null} t={(key) => key} />,
    );

    await waitFor(() => expect(getByRole("status").textContent).toContain("Reading local token history"));
    await waitFor(() => expect(getByText("4.9B")).toBeTruthy(), { timeout: 2_500 });
    expect(tauriMocks.getProviderChartData).toHaveBeenCalledTimes(2);
  });
});
