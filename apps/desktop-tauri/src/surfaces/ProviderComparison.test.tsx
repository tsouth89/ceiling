import { render, waitFor } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import type { ProviderChartData, ProviderUsageSnapshot } from "../types/bridge";
import ProviderComparison from "./ProviderComparison";

const mocks = vi.hoisted(() => ({
  getProviderChartData: vi.fn(),
}));

vi.mock("../lib/tauri", () => ({
  getProviderChartData: mocks.getProviderChartData,
}));

vi.mock("../components/providers/ProviderIcon", () => ({
  ProviderIcon: () => <span aria-hidden="true" />,
}));

function provider(
  providerId: string,
  accountId: string,
  accountEmail: string,
): ProviderUsageSnapshot {
  return {
    providerId,
    displayName: providerId === "codex" ? "Codex" : "Claude",
    accountId,
    accountEmail,
    primary: {
      usedPercent: 25,
      remainingPercent: 75,
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
    inactiveRateWindows: [],
    cost: null,
    planName: null,
    sourceLabel: "oauth",
    updatedAt: new Date().toISOString(),
    error: null,
    pace: null,
    accountOrganization: null,
    trayStatusLabel: null,
  };
}

describe("ProviderComparison", () => {
  beforeEach(() => {
    mocks.getProviderChartData.mockReset();
    mocks.getProviderChartData.mockImplementation(
      () => new Promise<ProviderChartData>(() => undefined),
    );
  });

  it("requests the explicit machine-wide scope instead of a representative account", async () => {
    render(
      <ProviderComparison
        providers={[
          provider("codex", "codex-account", "codex@example.com"),
          provider("claude", "claude-account", "claude@example.com"),
        ]}
      />,
    );

    await waitFor(() =>
      expect(mocks.getProviderChartData).toHaveBeenCalledTimes(2),
    );
    for (const call of mocks.getProviderChartData.mock.calls) {
      expect(call[1]).toBeUndefined();
      expect(call[2]).toBeUndefined();
      expect(call[5]).toBeUndefined();
    }
  });
});
