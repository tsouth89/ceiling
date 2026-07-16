import { render, screen } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import PlanStatusCard from "./PlanStatusCard";
import type { ProviderUsageSnapshot, RateWindowSnapshot } from "../types/bridge";

vi.mock("../hooks/useLocale", () => ({
  useLocale: () => ({
    t: (key: string) => {
      if (key === "PanelLeftSuffix") return "left";
      if (key === "PanelUsedSuffix") return "used";
      return key;
    },
  }),
}));

vi.mock("../hooks/useFormattedResetTime", () => ({
  useFormattedResetTime: () => "Resets in 12d",
}));

function window(usedPercent: number): RateWindowSnapshot {
  return {
    usedPercent,
    remainingPercent: 100 - usedPercent,
    windowMinutes: null,
    resetsAt: "2099-01-01T00:00:00Z",
    resetDescription: null,
    isExhausted: usedPercent >= 100,
    reservePercent: null,
    reserveDescription: null,
    reserveWillLastToReset: false,
    reserveEtaSeconds: null,
  };
}

function provider(
  overrides: Partial<ProviderUsageSnapshot> = {},
): ProviderUsageSnapshot {
  return {
    providerId: "cursor",
    displayName: "Cursor",
    primary: window(62),
    primaryLabel: "Monthly",
    secondary: window(90),
    secondaryLabel: "Auto",
    modelSpecific: null,
    tertiary: null,
    extraRateWindows: [],
    inactiveRateWindows: [],
    cost: null,
    planName: "Pro",
    accountEmail: "you@example.com",
    sourceLabel: "web",
    updatedAt: new Date().toISOString(),
    error: null,
    pace: null,
    accountOrganization: null,
    trayStatusLabel: null,
    fetchDurationMs: null,
    ...overrides,
  };
}

describe("PlanStatusCard", () => {
  it("shows logo, plan, pool hero, and hot companion", () => {
    render(
      <PlanStatusCard
        provider={provider()}
        resetTimeRelative
        showAsUsed={false}
      />,
    );

    expect(screen.getByText("Cursor")).toBeTruthy();
    expect(screen.getByText("Pro")).toBeTruthy();
    expect(screen.getByText("Monthly")).toBeTruthy();
    expect(screen.getByText(/38% left/)).toBeTruthy();
    expect(screen.getByText("Auto")).toBeTruthy();
    expect(screen.getByText(/10% left/)).toBeTruthy();
  });

  it("shows Cursor API as a compact third allowance row", () => {
    render(
      <PlanStatusCard
        provider={provider({
          extraRateWindows: [
            { id: "cursor-api", title: "API", window: window(8) },
          ],
        })}
        resetTimeRelative
        showAsUsed
      />,
    );

    expect(screen.getByText("Monthly")).toBeTruthy();
    expect(screen.getByText("Auto")).toBeTruthy();
    expect(screen.getByText("API")).toBeTruthy();
    expect(screen.getByText(/8% used/)).toBeTruthy();
  });

  it("still shows reset timing when the session is exhausted", () => {
    render(
      <PlanStatusCard
        provider={provider({
          providerId: "claude",
          displayName: "Claude",
          primary: window(100),
          primaryLabel: "Session (5h)",
          secondary: null,
          secondaryLabel: undefined,
          planName: "Claude Max 5x",
        })}
        resetTimeRelative
        showAsUsed
      />,
    );

    expect(screen.getByText(/100% used/)).toBeTruthy();
    expect(screen.getByText("Resets in 12d")).toBeTruthy();
  });

  it("always shows Claude weekly beside the 5-hour session", () => {
    render(
      <PlanStatusCard
        provider={provider({
          providerId: "claude",
          displayName: "Claude",
          primary: window(29),
          primaryLabel: "Session (5h)",
          secondary: window(12),
          secondaryLabel: "Weekly",
          planName: "Claude Max 5x",
        })}
        resetTimeRelative
        showAsUsed
      />,
    );

    expect(screen.getByText("Session (5h)")).toBeTruthy();
    expect(screen.getByText(/29% used/)).toBeTruthy();
    expect(screen.getByText("Weekly")).toBeTruthy();
    expect(screen.getByText(/12% used/)).toBeTruthy();
  });

  it("keeps overview identity quiet and strips redundant plan branding", () => {
    render(
      <PlanStatusCard
        provider={provider({
          planName: "Cursor Pro",
          promoSignals: [
            {
              id: "cursor-promotional",
              kind: "boost",
              title: "promotional",
              description: "Temporary promotional capacity",
              windowId: "cursor-promotional",
              endsAt: null,
            },
          ],
        })}
        resetTimeRelative
      />,
    );

    expect(screen.getByText("Pro")).toBeTruthy();
    expect(screen.queryByText("you@example.com")).toBeNull();
    expect(screen.queryByText("promotional")).toBeNull();
  });

  it("describes inactive limits as a quiet row instead of a vague chip", () => {
    render(
      <PlanStatusCard
        provider={provider({
          inactiveRateWindows: [
            {
              id: "codex-five-hour",
              title: "5-hour",
              description: "Not currently enforced by OpenAI",
            },
          ],
        })}
        resetTimeRelative
      />,
    );

    expect(screen.getByText("5-hour")).toBeTruthy();
    expect(screen.getByText("not currently enforced")).toBeTruthy();
    expect(screen.queryByText("lifted")).toBeNull();
  });
});
