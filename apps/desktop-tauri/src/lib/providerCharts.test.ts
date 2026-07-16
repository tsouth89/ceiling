import { describe, expect, it } from "vitest";
import {
  providerLocalUsageWindows,
  providerSupportsChartData,
} from "./providerCharts";
import type { ProviderUsageSnapshot, RateWindowSnapshot } from "../types/bridge";

describe("providerSupportsChartData", () => {
  it("keeps chart fetches limited to providers with chart/local usage data", () => {
    expect(providerSupportsChartData("codex")).toBe(true);
    expect(providerSupportsChartData("claude")).toBe(true);
    expect(providerSupportsChartData("openai")).toBe(true);
    expect(providerSupportsChartData("cursor")).toBe(true);
    expect(providerSupportsChartData("OpenAI")).toBe(true);

    expect(providerSupportsChartData("copilot")).toBe(false);
    expect(providerSupportsChartData("deepseek")).toBe(false);
  });
});

describe("providerLocalUsageWindows", () => {
  const rateWindow = (
    usedPercent: number,
    windowMinutes: number,
    resetsAt: string,
  ): RateWindowSnapshot => ({
    usedPercent,
    remainingPercent: 100 - usedPercent,
    windowMinutes,
    resetsAt,
    resetDescription: null,
    isExhausted: false,
    reservePercent: null,
    reserveDescription: null,
    reserveWillLastToReset: false,
    reserveEtaSeconds: null,
  });

  it("derives exact Claude 5h and weekly ranges from live reset metadata", () => {
    const provider = {
      providerId: "claude",
      primaryLabel: "Session (5h)",
      primary: rateWindow(20, 300, "2026-07-15T20:00:00.000Z"),
      secondaryLabel: "Weekly",
      secondary: rateWindow(10, 10_080, "2026-07-20T20:00:00.000Z"),
    } as ProviderUsageSnapshot;

    expect(providerLocalUsageWindows(provider)).toEqual([
      {
        id: "primary",
        label: "Current 5h window",
        startsAt: "2026-07-15T15:00:00.000Z",
        endsAt: "2026-07-15T20:00:00.000Z",
      },
      {
        id: "secondary",
        label: "Current weekly window",
        startsAt: "2026-07-13T20:00:00.000Z",
        endsAt: "2026-07-20T20:00:00.000Z",
      },
    ]);
  });
});
