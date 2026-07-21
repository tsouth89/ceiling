import { render, waitFor } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";

const tauriMocks = vi.hoisted(() => ({
  getProviderChartData: vi.fn(),
  getSettingsSnapshot: vi.fn(),
  getCursorModelActivity: vi.fn(),
  exportCostCsv: vi.fn(),
}));

vi.mock("../../../../../lib/tauri", () => tauriMocks);

import { ChartsSection } from "./ChartsSection";
import type { ProviderUsageSnapshot } from "../../../../../types/bridge";

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
        label: "5-hour window",
        startsAt: new Date().toISOString(),
        endsAt: new Date(Date.now() + 3_600_000).toISOString(),
        tokens: 18_400_000,
        cost: 12.4,
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
        label: "Weekly window",
        startsAt: new Date(Date.now() - 4 * 24 * 3_600_000).toISOString(),
        endsAt: new Date(Date.now() + 3 * 24 * 3_600_000).toISOString(),
        tokens: 843_400_000,
        cost: 842.5,
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
    modelBreakdown: [
      { model: "claude-opus-4-8", cost: 15_000, tokens: 20_000_000_000 },
      { model: "claude-sonnet-5", cost: 2_700, tokens: 3_500_000_000 },
      { model: "claude-retired-x", cost: null, tokens: 50_000_000 },
    ],
    effortBreakdown: [
      { effort: "high", cost: 12_000, tokens: 16_000_000_000 },
      { effort: "xhigh", cost: 5_700, tokens: 7_500_000_000 },
      { effort: "unknown", cost: null, tokens: 50_000_000 },
    ],
    projectBreakdown: [
      { project: "ceiling", cost: 14_000, tokens: 18_000_000_000 },
      { project: "burnwatch", cost: 3_700, tokens: 5_000_000_000 },
      { project: "unknown", cost: null, tokens: 50_000_000 },
    ],
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
    tauriMocks.getCursorModelActivity.mockResolvedValue([]);
    tauriMocks.exportCostCsv.mockResolvedValue("C:/Users/me/Downloads/ceiling-claude-spend.csv");
  });

  it("shows comparable processed totals and the seven-day token mix", async () => {
    const { getByText, getAllByText, getByLabelText } = render(
      <ChartsSection providerId="claude" accountEmail={null} t={(key) => key} />,
    );

    await waitFor(() => expect(getByText("4.9B")).toBeTruthy());
    expect(getByText("23.6B")).toBeTruthy();
    expect(getByText("5-hour window")).toBeTruthy();
    expect(getByText("Weekly window")).toBeTruthy();
    expect(getByText("18.4M · $12.40")).toBeTruthy();
    expect(getByText("843.4M · $842.50")).toBeTruthy();
    expect(() => getByText("Last session")).toThrow();
    expect(getByText("99.7% cache traffic")).toBeTruthy();
    expect(getAllByText(/processed tokens · calendar window/)).toHaveLength(2);
    expect(() => getByText("$3,427.91")).toThrow();
    expect(getByLabelText("Local usage summary").getAttribute("data-card-count")).toBe("4");

    const mix = getByLabelText("Last 7 days token breakdown");
    expect(mix.textContent).toContain("Fresh input2M");
    expect(mix.textContent).toContain("Output14.1M");
    expect(mix.textContent).toContain("Cache read4.8B");
    expect(mix.textContent).toContain("Cache write118.6M");

    const models = getByLabelText("Cost by model over 30 days");
    // Priced models show dollars; the total sums only priced rows.
    expect(models.textContent).toContain("claude-opus-4-8");
    expect(models.textContent).toContain("$15,000.00");
    expect(models.textContent).toContain("claude-sonnet-5");
    expect(models.textContent).toContain("$2,700.00");
    // Unpriced model keeps its tokens but shows "Not priced", no dollars.
    expect(models.textContent).toContain("claude-retired-x");
    expect(models.textContent).toContain("Not priced");
    // Header total = sum of priced rows only ($15,000 + $2,700).
    expect(models.textContent).toContain("$17,700.00");

    const efforts = getByLabelText("Cost by reasoning effort over 30 days");
    // Effort tiers show friendly labels and dollars; total sums priced rows.
    expect(efforts.textContent).toContain("High");
    expect(efforts.textContent).toContain("$12,000.00");
    expect(efforts.textContent).toContain("Extra high");
    expect(efforts.textContent).toContain("$5,700.00");
    expect(efforts.textContent).toContain("Unspecified");
    expect(efforts.textContent).toContain("$17,700.00");

    const projects = getByLabelText("Cost by project over 30 days");
    expect(projects.textContent).toContain("ceiling");
    expect(projects.textContent).toContain("$14,000.00");
    expect(projects.textContent).toContain("burnwatch");
    expect(projects.textContent).toContain("Unknown project");
    expect(projects.textContent).toContain("Not priced");
    expect(projects.textContent).toContain("$17,700.00");
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

  it("does not substitute a rolling period when the provider reset is unavailable", async () => {
    tauriMocks.getProviderChartData.mockResolvedValue({
      ...enrichedData,
      localUsage: null,
      quotaHistory: [
        {
          recordedAt: "2026-07-19T12:00:00.000Z",
          windows: [{ id: "primary", label: "Session", usedPercent: 20 }],
        },
      ],
    });
    const providerSnapshot: ProviderUsageSnapshot = {
      providerId: "claude",
      displayName: "Claude",
      primary: {
        usedPercent: 20,
        remainingPercent: 80,
        windowMinutes: 300,
        resetsAt: null,
        resetDescription: null,
        isExhausted: false,
        reservePercent: null,
        reserveDescription: null,
        reserveWillLastToReset: false,
        reserveEtaSeconds: null,
      },
      primaryLabel: "Session",
      secondary: null,
      modelSpecific: null,
      tertiary: null,
      extraRateWindows: [],
      cost: null,
      planName: null,
      accountEmail: null,
      sourceLabel: "Claude CLI",
      updatedAt: "2026-07-19T12:00:00.000Z",
      error: null,
      pace: null,
      accountOrganization: null,
      trayStatusLabel: null,
    };

    const { getByText } = render(
      <ChartsSection
        providerId="claude"
        accountEmail={null}
        providerSnapshot={providerSnapshot}
        t={(key) => key}
      />,
    );

    expect(
      await waitFor(() => getByText("Reset boundary unavailable. Ceiling will not substitute a rolling period.")),
    ).toBeTruthy();
    expect(tauriMocks.getProviderChartData).toHaveBeenCalledWith(
      "claude",
      undefined,
      [],
      "Claude CLI",
    );
  });

  it("shows a Cursor activity-by-model card with shares and Auto relabel", async () => {
    tauriMocks.getProviderChartData.mockResolvedValue({
      ...enrichedData,
      providerId: "cursor",
      localUsage: null,
    });
    tauriMocks.getCursorModelActivity.mockResolvedValue([
      { model: "grok-4.5", contributions: 750, requests: 30 },
      { model: "claude-sonnet-5", contributions: 250, requests: 10 },
      { model: "default", contributions: 0, requests: 1 },
    ]);

    const { getByLabelText } = render(
      <ChartsSection providerId="cursor" accountEmail={null} t={(key) => key} />,
    );

    const card = await waitFor(() => getByLabelText("Cursor activity by model over 30 days"));
    expect(card.textContent).toContain("grok-4.5");
    expect(card.textContent).toContain("75%"); // 750 of 1000
    expect(card.textContent).toContain("claude-sonnet-5");
    expect(card.textContent).toContain("25%");
    // "default" is relabeled to "Auto".
    expect(card.textContent).toContain("Auto");
    // Honest framing: activity, not spend.
    expect(card.textContent).toMatch(/activity, not tokens or spend/i);
  });

  it("shows Cursor activity even when chart history fails to load", async () => {
    tauriMocks.getProviderChartData.mockRejectedValue(new Error("no history"));
    tauriMocks.getCursorModelActivity.mockResolvedValue([
      { model: "grok-4.5", contributions: 100, requests: 5 },
    ]);

    const { getByLabelText, queryByText } = render(
      <ChartsSection providerId="cursor" accountEmail={null} t={(key) => key} />,
    );

    const card = await waitFor(() => getByLabelText("Cursor activity by model over 30 days"));
    expect(card.textContent).toContain("grok-4.5");
    // The bare "History unavailable" error must not replace the card.
    expect(queryByText("History unavailable")).toBeNull();
  });

  it("collapses a long project list behind a show-more toggle", async () => {
    const manyProjects = Array.from({ length: 12 }, (_, index) => ({
      project: `project-${index}`,
      cost: 1000 - index * 10,
      tokens: 1_000_000,
    }));
    tauriMocks.getProviderChartData.mockResolvedValue({
      ...enrichedData,
      localUsage: { ...enrichedData.localUsage, projectBreakdown: manyProjects },
    });

    const { getByLabelText, getByRole } = render(
      <ChartsSection providerId="claude" accountEmail={null} t={(key) => key} />,
    );

    const card = await waitFor(() => getByLabelText("Cost by project over 30 days"));
    // Collapsed: only the first 8 rows are shown.
    expect(card.querySelectorAll(".usage-model-costs__row")).toHaveLength(8);
    expect(card.textContent).toContain("project-0");
    expect(card.textContent).not.toContain("project-8");

    const toggle = getByRole("button", { name: "Show all 12 projects" });
    toggle.click();

    await waitFor(() =>
      expect(card.querySelectorAll(".usage-model-costs__row")).toHaveLength(12),
    );
    expect(card.textContent).toContain("project-11");
    expect(getByRole("button", { name: "Show fewer" })).toBeTruthy();
  });

  it("exports spend to CSV and shows the saved path", async () => {
    const { getByText, findByText } = render(
      <ChartsSection providerId="claude" accountEmail={null} t={(key) => key} />,
    );

    const button = await waitFor(() => getByText("Export CSV"));
    button.click();

    expect(tauriMocks.exportCostCsv).toHaveBeenCalledWith("claude");
    expect(await findByText(/Saved to .*ceiling-claude-spend\.csv/)).toBeTruthy();
  });
});
