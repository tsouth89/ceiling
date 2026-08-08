import { render, screen } from "@testing-library/react";
import { describe, expect, it } from "vitest";

import type { RateWindowSnapshot } from "../../types/bridge";
import UsageBar from "./UsageBar";

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