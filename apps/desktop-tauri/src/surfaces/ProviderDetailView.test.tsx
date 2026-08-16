import { render, screen, waitFor } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import type { ProviderUsageSnapshot, RateWindowSnapshot } from "../types/bridge";
import ProviderDetailView from "./ProviderDetailView";

const tauriMocks = vi.hoisted(() => ({
  getProviderChartData: vi.fn(),
}));

vi.mock("../lib/tauri", () => tauriMocks);
vi.mock("../hooks/useLocale", () => ({
  useLocale: () => ({
    t: (key: string) =>
      ({
        ResetCreditsAvailableOne: "{} reset available",
        ResetCreditsAvailableMany: "{} resets available",
        NotCurrentlyEnforced: "Not currently enforced",
        WindowUnavailable: "Unavailable",
      }[key] ?? key),
  }),
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
      {
        id: "cursor-on-demand",
        title: "On-demand",
        window: rate(35),
        amount: {
          used: 3.5,
          limit: 10,
          currencyCode: "USD",
          formattedUsed: "$3.50",
          formattedLimit: "$10.00",
        },
      },
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
    // On-demand used to be filtered out of this view. It is the only Cursor
    // lane that bills real money, and it now carries that money beside its
    // bar, so it belongs here.
    expect(screen.getByText("On-demand")).toBeInTheDocument();
    // Connector word comes from the locale bundle, which this test does not
    // mount, so match the amounts rather than the joined sentence.
    expect(screen.getByText(/\$3\.50.*\$10\.00/)).toBeInTheDocument();
    expect(screen.getAllByText(/Weekly pace/)).toHaveLength(2);
    expect(screen.getAllByText(/Far ahead of budget/)).toHaveLength(2);
    expect(container.querySelector(".provider-focus__pace-fill")?.getAttribute("data-tone"))
      .toBe("risk");
    // Concrete depletion ETA when the pace will not last to reset (SOU-274).
    expect(screen.getByText(/about 1h left before this window runs out/)).toBeInTheDocument();

    await waitFor(() => expect(screen.getByText("542.5M")).toBeInTheDocument());
    expect(screen.getByText("96.0%")).toBeInTheDocument();
    expect(screen.getByText(/Most used model: gpt-5.6-sol/)).toBeInTheDocument();
  });

  /**
   * SBS-876: Cursor missing-plan still writes 0% primary. Detail must
   * headline the named state, not "0% used".
   */
  it("does not headline 0% when Cursor plan is unavailable", async () => {
    const cursor: ProviderUsageSnapshot = {
      ...codex(),
      providerId: "cursor",
      displayName: "Cursor",
      // Spreading the Codex fixture would otherwise render a ChatGPT plan name
      // inside a Cursor view, which is exactly what provider siloing forbids.
      planName: "Ultra",
      primary: rate(0),
      primaryLabel: "Plan",
      extraRateWindows: [],
      inactiveRateWindows: [
        {
          id: "cursor-plan",
          title: "Plan",
          description: "No usage reported",
          state: "unavailable",
        },
      ],
      // The bridge still derives pace from the required 0% primary, so a real
      // Cursor snapshot arrives with a verdict attached. Keep it here: a null
      // pace could not catch the pace copy leaking beside "Unavailable".
      pace: {
        windowLabel: "Plan",
        stage: "far_ahead",
        deltaPercent: -38.6,
        willLastToReset: true,
        etaSeconds: null,
        expectedUsedPercent: 38.6,
        actualUsedPercent: 0,
      },
      resetCreditsAvailable: null,
    };

    render(
      <ProviderDetailView
        provider={cursor}
        resetTimeRelative
        showAsUsed
      />,
    );

    expect(screen.getByText("Plan usage")).toBeInTheDocument();
    expect(screen.getByText("Unavailable")).toBeInTheDocument();
    expect(screen.queryByText("0%")).not.toBeInTheDocument();
    expect(screen.queryByText("100%")).not.toBeInTheDocument();
    // A pace verdict read off the placeholder 0% is not a reading either.
    expect(screen.queryByText(/Plan pace/)).not.toBeInTheDocument();
    expect(screen.queryByText(/ahead of budget/)).not.toBeInTheDocument();
  });

  /**
   * SBS-876: `preferred_pace` picks the worst delta across every long window,
   * so pace is often Auto rather than Plan. Dropping pace whenever the primary
   * is a placeholder threw away that valid Auto verdict too.
   */
  it("keeps an Auto pace when only the Cursor Plan is unavailable", async () => {
    const cursor: ProviderUsageSnapshot = {
      ...codex(),
      providerId: "cursor",
      displayName: "Cursor",
      planName: "Ultra",
      primary: rate(0),
      primaryLabel: "Plan",
      secondary: rate(44),
      secondaryLabel: "Auto",
      extraRateWindows: [],
      inactiveRateWindows: [
        {
          id: "cursor-plan",
          title: "Plan",
          description: "No usage reported",
          state: "unavailable",
        },
      ],
      pace: {
        windowLabel: "Auto",
        stage: "far_ahead",
        deltaPercent: 31.6,
        willLastToReset: false,
        etaSeconds: 3600,
        expectedUsedPercent: 12.4,
        actualUsedPercent: 44,
      },
      resetCreditsAvailable: null,
    };

    render(
      <ProviderDetailView provider={cursor} resetTimeRelative showAsUsed />,
    );

    expect(screen.getByText("Unavailable")).toBeInTheDocument();
    // The Auto verdict is a reading of a real window, so it survives.
    expect(screen.getAllByText(/Auto pace/).length).toBeGreaterThan(0);
  });
});
