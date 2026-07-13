import { describe, expect, it } from "vitest";
import { render } from "@testing-library/react";
import ActivityTimeline from "./ActivityTimeline";
import type {
  ProviderUsageSnapshot,
  RateWindowSnapshot,
} from "../types/bridge";

const HOUR = 60 * 60 * 1000;
const DAY = 24 * HOUR;

function window(
  usedPercent: number,
  resetInMs: number | null,
): RateWindowSnapshot {
  return {
    usedPercent,
    remainingPercent: 100 - usedPercent,
    windowMinutes: null,
    resetsAt: resetInMs === null ? null : new Date(Date.now() + resetInMs).toISOString(),
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
    providerId: "codex",
    displayName: "Codex",
    primary: window(30, 6 * DAY),
    primaryLabel: "Weekly",
    secondary: null,
    modelSpecific: null,
    tertiary: null,
    extraRateWindows: [],
    inactiveRateWindows: [],
    cost: null,
    planName: null,
    accountEmail: null,
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

function rowsOf(container: HTMLElement) {
  return [...container.querySelectorAll<HTMLElement>(".activity-row")];
}

describe("ActivityTimeline", () => {
  it("lists every reset window across providers, soonest first", () => {
    const providers = [
      provider({
        providerId: "codex",
        displayName: "Codex",
        primary: window(80, 6 * DAY), // this week
      }),
      provider({
        providerId: "claude",
        displayName: "Claude",
        primary: window(57, 1 * HOUR + 27 * 60 * 1000), // soonest
        primaryLabel: "Session (5h)",
        secondary: window(63, 12 * HOUR), // today
        secondaryLabel: "Weekly",
      }),
    ];

    const { container } = render(<ActivityTimeline providers={providers} />);
    const rows = rowsOf(container);

    // Three windows total (Codex weekly, Claude session, Claude weekly).
    expect(rows).toHaveLength(3);
    // Sorted by soonest reset: Claude session (1h27m) → Claude weekly (12h) →
    // Codex weekly (6d).
    expect(rows[0].textContent).toContain("Session (5h)");
    expect(rows[1].textContent).toContain("Weekly");
    expect(rows[2].textContent).toContain("Codex");
  });

  it("buckets by how far out the reset is", () => {
    const providers = [
      provider({
        providerId: "claude",
        displayName: "Claude",
        primary: window(57, 2 * HOUR), // Next 24 hours
      }),
      provider({
        providerId: "codex",
        displayName: "Codex",
        primary: window(80, 6 * DAY), // This week
      }),
      provider({
        providerId: "cursor",
        displayName: "Cursor",
        primary: window(85, 24 * DAY), // Later
      }),
    ];

    const { container } = render(<ActivityTimeline providers={providers} />);
    const text = container.textContent ?? "";
    expect(text).toContain("Next 24 hours");
    expect(text).toContain("This week");
    expect(text).toContain("Later");
  });

  it("excludes errored providers and marks usage level on the bar", () => {
    const providers = [
      provider({
        providerId: "codex",
        displayName: "Codex",
        error: "network timeout",
        primary: window(80, 2 * HOUR),
      }),
      provider({
        providerId: "cursor",
        displayName: "Cursor",
        primary: window(80, 3 * DAY), // 20% headroom → "high" level
        primaryLabel: "Auto",
      }),
    ];

    const { container } = render(<ActivityTimeline providers={providers} />);
    const rows = rowsOf(container);

    // Only the healthy provider's window appears.
    expect(rows).toHaveLength(1);
    expect(rows[0].textContent).toContain("Cursor");
    // A window down to its last 20% carries a hot level on its bar fill.
    const fill = container.querySelector(".activity-row__bar-fill");
    expect(fill?.getAttribute("data-level")).toBe("high");
  });

  it("hides 0%-used windows unless they reset soon", () => {
    const providers = [
      provider({
        providerId: "cursor",
        displayName: "Cursor",
        primary: window(40, 2 * HOUR), // active → shown
        primaryLabel: "Plan",
        secondary: window(0, 24 * DAY), // 0% + far off → hidden
        secondaryLabel: "Promotional",
        extraRateWindows: [
          // 0% but imminent → kept as a heads-up.
          { id: "trial", title: "Trial", window: window(0, 3 * HOUR) },
        ],
      }),
    ];

    const { container } = render(<ActivityTimeline providers={providers} />);
    const text = container.textContent ?? "";
    expect(text).toContain("Plan");
    expect(text).toContain("Trial");
    expect(text).not.toContain("Promotional");
  });

  it("shows an empty state when nothing is scheduled", () => {
    const providers = [
      provider({ primary: window(30, null), primaryLabel: "Weekly" }),
    ];
    const { container } = render(<ActivityTimeline providers={providers} />);
    // A window with no resetsAt falls under "No scheduled reset", not empty.
    expect(container.textContent).toContain("No scheduled reset");

    const { container: emptyContainer } = render(
      <ActivityTimeline providers={[]} />,
    );
    expect(emptyContainer.textContent).toContain("Nothing scheduled");
  });
});
