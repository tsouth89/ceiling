import { render, screen } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import PlanStatusCard from "./PlanStatusCard";
import type { ProviderUsageSnapshot, RateWindowSnapshot } from "../types/bridge";
import { expectNoAccessibilityViolations } from "../test/accessibility";

vi.mock("../hooks/useLocale", () => ({
  useLocale: () => ({
    t: (key: string) => {
      if (key === "PanelLeftSuffix") return "left";
      if (key === "PanelUsedSuffix") return "used";
      if (key === "PanelAmountOf") return "of";
      if (key === "ResetCreditsAvailableOne") return "{} reset available";
      if (key === "ResetCreditsAvailableMany") return "{} resets available";
      if (key === "FreshnessStale") return "Stale";
      if (key === "NotCurrentlyEnforced") return "not currently enforced";
      if (key === "WindowUnavailable") return "unavailable";
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
  it("has no detectable accessibility violations", async () => {
    const { container } = render(
      <PlanStatusCard
        provider={provider()}
        resetTimeRelative
        showAsUsed
        onSelect={vi.fn()}
      />,
    );

    await expectNoAccessibilityViolations(container);
  });

  it("includes displayed quota windows in the card button name", () => {
    render(
      <PlanStatusCard
        provider={provider()}
        resetTimeRelative
        showAsUsed
        onSelect={vi.fn()}
      />,
    );

    expect(
      screen.getByRole("button", {
        name: "Cursor; Monthly: 62% used; Auto: 90% used",
      }),
    ).toBeInTheDocument();
  });

  it("includes provider errors in the card button name", () => {
    render(
      <PlanStatusCard
        provider={provider({ error: "Authentication failed" })}
        resetTimeRelative
        onSelect={vi.fn()}
      />,
    );

    expect(
      screen.getByRole("button", {
        name: "Cursor: Authentication failed",
      }),
    ).toBeInTheDocument();
  });

  it("includes unavailable windows in the card button name", () => {
    render(
      <PlanStatusCard
        provider={provider({
          primary: window(0),
          primaryLabel: "Plan",
          secondary: null,
          inactiveRateWindows: [
            {
              id: "cursor-plan",
              title: "Plan",
              description: "No usage reported",
              state: "unavailable",
            },
          ],
        })}
        resetTimeRelative
        onSelect={vi.fn()}
      />,
    );

    expect(
      screen.getByRole("button", { name: "Cursor; Plan: unavailable" }),
    ).toBeInTheDocument();
  });

  it("names each account by email when a provider has two (the reported bug)", () => {
    // Exactly the user's setup: two Codex accounts.
    const { unmount } = render(
      <PlanStatusCard
        provider={provider({
          providerId: "codex",
          displayName: "Codex",
          accountId: "acct-personal",
          accountEmail: "tsouth2@gmail.com",
          planName: "Pro Lite",
          // A custom label must NOT be what shows.
          accountLabel: "Personal",
        })}
        showAccount
        resetTimeRelative
      />,
    );
    expect(screen.getByText("tsouth2@gmail.com (Pro Lite)")).toBeInTheDocument();
    // The custom label is not shown on the card.
    expect(screen.queryByText("Personal")).toBeNull();
    unmount();

    render(
      <PlanStatusCard
        provider={provider({
          providerId: "codex",
          displayName: "Codex",
          accountId: "acct-work",
          accountEmail: "bts@cssi.us",
          planName: "ChatGPT Team",
          accountLabel: "Work",
        })}
        showAccount
        resetTimeRelative
      />,
    );
    // The second account shows its OWN email, not "Work".
    expect(screen.getByText("bts@cssi.us (ChatGPT Team)")).toBeInTheDocument();
    expect(screen.queryByText("Work")).toBeNull();
  });

  it("stays on the plan chip for a single account, no email clutter", () => {
    render(
      <PlanStatusCard
        provider={provider({
          providerId: "codex",
          displayName: "Codex",
          accountEmail: "solo@example.com",
          planName: "ChatGPT Pro",
        })}
        showAccount={false}
        resetTimeRelative
      />,
    );
    // One account: the email is not shown, the plan chip is.
    expect(screen.queryByText(/solo@example.com/)).toBeNull();
    expect(screen.getByText("ChatGPT Pro")).toBeInTheDocument();
  });

  it("shows an available Codex reset as a quiet chip", () => {
    render(
      <PlanStatusCard
        provider={provider({
          providerId: "codex",
          displayName: "Codex",
          resetCreditsAvailable: 1,
        })}
        resetTimeRelative
      />,
    );
    expect(screen.getByText(/1 reset available/)).toBeInTheDocument();
  });

  it("shows an available Grok reset as a quiet chip", () => {
    render(
      <PlanStatusCard
        provider={provider({
          providerId: "grok",
          displayName: "Grok",
          resetCreditsAvailable: 1,
        })}
        resetTimeRelative
      />,
    );
    expect(screen.getByText(/1 reset available/)).toBeInTheDocument();
  });

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

  it("shows Cursor on-demand dollars once included usage is exhausted", () => {
    render(
      <PlanStatusCard
        provider={provider({
          primary: window(100),
          secondary: window(100),
          extraRateWindows: [
            { id: "cursor-api", title: "API", window: window(100) },
            {
              id: "cursor-on-demand",
              title: "On-demand",
              window: window(56),
              amount: {
                used: 1002.16,
                limit: 1800,
                currencyCode: "USD",
                formattedUsed: "$1,002.16",
                formattedLimit: "$1,800.00",
              },
            },
          ],
        })}
        resetTimeRelative
        showAsUsed
      />,
    );

    expect(screen.getByText("On-demand")).toBeInTheDocument();
    expect(screen.getByText("56% used")).toBeInTheDocument();
    expect(screen.getByText("$1,002.16 of $1,800.00")).toBeInTheDocument();
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
    expect(screen.getByText("Depleted")).toBeTruthy();
    expect(screen.getByText("Resets in 12d")).toBeTruthy();
  });

  it("uses a quiet status label instead of recoloring the usage bar", () => {
    const { container } = render(
      <PlanStatusCard
        provider={provider({
          primary: window(82),
          secondary: null,
          secondaryLabel: undefined,
        })}
        resetTimeRelative
        showAsUsed
      />,
    );

    expect(screen.getByText("Near limit")).toBeTruthy();
    expect(
      container.querySelector('.plan-status-card__bar-fill[data-level="high"]'),
    ).toBeTruthy();
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

  /**
   * SBS-876: a missing Cursor plan used to paint Plan 0% used (or 100% left)
   * and a quiet "Plan Unavailable" row at the same time.
   */
  it("shows Unavailable for a missing Cursor plan and does not paint 0% or 100% left", () => {
    render(
      <PlanStatusCard
        provider={provider({
          primary: window(0),
          primaryLabel: "Plan",
          secondary: null,
          secondaryLabel: undefined,
          extraRateWindows: [],
          inactiveRateWindows: [
            {
              id: "cursor-plan",
              title: "Plan",
              description: "No usage reported",
              state: "unavailable",
            },
          ],
        })}
        resetTimeRelative
        showAsUsed
      />,
    );

    expect(screen.getByText("unavailable")).toBeInTheDocument();
    expect(screen.getByText("Plan")).toBeInTheDocument();
    expect(screen.queryByText(/0% used/)).toBeNull();
    expect(screen.queryByText(/100% left/)).toBeNull();
    expect(screen.queryByText(/0% left/)).toBeNull();
  });

  it("separates unavailable windows from not-enforced ones", () => {
    render(
      <PlanStatusCard
        provider={provider({
          inactiveRateWindows: [
            {
              id: "session",
              title: "5-hour",
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
        })}
        resetTimeRelative
      />,
    );

    expect(screen.getByText("not currently enforced")).toBeTruthy();
    expect(screen.getByText("unavailable")).toBeTruthy();
    // The unavailable window must not be lumped under "not currently enforced".
    expect(screen.getByText("Weekly")).toBeTruthy();
  });
});
