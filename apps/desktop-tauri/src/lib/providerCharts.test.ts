import { describe, expect, it } from "vitest";
import {
  providerHasUnavailableResetBoundary,
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
  const rateWindow = (resetsAt: string): RateWindowSnapshot => ({
    usedPercent: 20,
    remainingPercent: 80,
    windowMinutes: 300,
    resetsAt,
    resetDescription: null,
    isExhausted: false,
    reservePercent: null,
    reserveDescription: null,
    reserveWillLastToReset: false,
    reserveEtaSeconds: null,
  });

  const provider = (resetsAt: string): ProviderUsageSnapshot => ({
    providerId: "claude",
    displayName: "Claude",
    primary: rateWindow(resetsAt),
    primaryLabel: "Session (5h)",
    secondary: null,
    modelSpecific: null,
    tertiary: null,
    extraRateWindows: [],
    inactiveRateWindows: [],
    cost: null,
    planName: null,
    accountEmail: null,
    sourceLabel: "cli",
    updatedAt: resetsAt,
    error: null,
    pace: null,
    accountOrganization: null,
    trayStatusLabel: null,
    fetchDurationMs: null,
  });

  it("stabilizes sub-second reset drift into one local-history window", () => {
    const justBefore = providerLocalUsageWindows(provider("2026-07-15T21:19:59.860Z"));
    const justAfter = providerLocalUsageWindows(provider("2026-07-15T21:20:00.363Z"));

    expect(justBefore).toEqual(justAfter);
    expect(justAfter[0]).toMatchObject({
      startsAt: "2026-07-15T16:20:00.000Z",
      endsAt: "2026-07-15T21:20:00.000Z",
    });
  });

  it("reports an unavailable boundary instead of creating a rolling window", () => {
    const snapshot = provider("2026-07-15T21:20:00.000Z");
    snapshot.primary = { ...snapshot.primary, resetsAt: null };

    expect(providerLocalUsageWindows(snapshot)).toEqual([]);
    expect(providerHasUnavailableResetBoundary(snapshot)).toBe(true);
  });
});
