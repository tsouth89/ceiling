import { render, screen } from "@testing-library/react";
import { describe, expect, it } from "vitest";

import type { RateWindowSnapshot } from "../../types/bridge";
import UsageBar, { usageLevel } from "./UsageBar";

function windowSnapshot(
  overrides: Partial<RateWindowSnapshot> = {},
): RateWindowSnapshot {
  return {
    usedPercent: 72,
    remainingPercent: 28,
    windowMinutes: 300,
    resetsAt: null,
    resetDescription: null,
    isExhausted: false,
    reservePercent: null,
    reserveDescription: null,
    ...overrides,
  };
}

describe("UsageBar", () => {
  it("exposes progressbar semantics with the clamped percentage", () => {
    render(<UsageBar window={windowSnapshot({ usedPercent: 72 })} label="Weekly" />);

    const track = screen.getByRole("progressbar");
    expect(track).toHaveAttribute("aria-valuenow", "72");
    expect(track).toHaveAttribute("aria-valuemin", "0");
    expect(track).toHaveAttribute("aria-valuemax", "100");
    expect(track).toHaveAttribute("aria-label", "Weekly usage: 72%");
  });

  it("falls back to a generic label when no label prop is given", () => {
    render(<UsageBar window={windowSnapshot({ usedPercent: 40 })} />);

    expect(screen.getByRole("progressbar")).toHaveAttribute(
      "aria-label",
      "Usage: 40%",
    );
  });

  it("clamps aria-valuenow to 100 but keeps the raw percentage in the label", () => {
    render(<UsageBar window={windowSnapshot({ usedPercent: 140 })} label="Session" />);

    const track = screen.getByRole("progressbar");
    expect(track).toHaveAttribute("aria-valuenow", "100");
    // aria-valuenow must stay within aria-valuemax per the ARIA spec, but the
    // label still reports the true 140% — a screen reader user must not be
    // told they're at the limit when they're actually 40% over it.
    expect(track).toHaveAttribute("aria-label", "Session usage: 140%");
  });
});

describe("usageLevel", () => {
  it("returns normal below the high threshold", () => {
    expect(usageLevel(0, false)).toBe("normal");
    expect(usageLevel(69, false)).toBe("normal");
  });

  it("returns high at and above 70, below the critical threshold", () => {
    expect(usageLevel(70, false)).toBe("high");
    expect(usageLevel(89, false)).toBe("high");
  });

  it("returns critical at and above 90", () => {
    expect(usageLevel(90, false)).toBe("critical");
    expect(usageLevel(100, false)).toBe("critical");
  });

  it("returns exhausted when exhausted is true, regardless of a low percentage", () => {
    expect(usageLevel(0, true)).toBe("exhausted");
    expect(usageLevel(40, true)).toBe("exhausted");
  });
});
