import { describe, expect, it } from "vitest";
import {
  formatResetCountdown,
  remainingCountdownParts,
} from "./resetCountdown";

describe("remainingCountdownParts / formatResetCountdown", () => {
  // SBS-927: last sub-minute must not floor to "0m".
  it("clamps a still-future sub-minute remainder to 1m", () => {
    expect(formatResetCountdown(30_000)).toBe("1m");
    expect(formatResetCountdown(59_000)).toBe("1m");
    expect(remainingCountdownParts(30_000)).toEqual({
      days: 0,
      hours: 0,
      minutes: 1,
    });
  });

  it("floors 61-119s to 1m, not a ceil of 2m", () => {
    expect(formatResetCountdown(61_000)).toBe("1m");
    expect(formatResetCountdown(119_000)).toBe("1m");
  });

  // SBS-927: 24h cut is minutes/1440, not hours > 24.
  it("cuts a day at twenty-four hours", () => {
    expect(formatResetCountdown(24 * 3600_000)).toBe("1d 0h");
    expect(formatResetCountdown(24 * 3600_000 + 1_000)).toBe("1d 0h");
    expect(formatResetCountdown(24 * 3600_000 + 10 * 60_000)).toBe("1d 0h");
    expect(formatResetCountdown(24 * 3600_000 + 30 * 60_000)).toBe("1d 0h");
    expect(formatResetCountdown(25 * 3600_000)).toBe("1d 1h");
    expect(formatResetCountdown(23 * 3600_000 + 59 * 60_000 + 59_000)).toBe(
      "23h 59m",
    );
  });

  it("returns null once the target is due", () => {
    expect(formatResetCountdown(0)).toBeNull();
    expect(formatResetCountdown(-1)).toBeNull();
  });
});
