import { beforeEach, describe, expect, it, vi } from "vitest";
import {
  MAX_ASKS,
  MIN_GAP_MS,
  SETTLE_MS,
  hasRealReading,
  isLaterVersion,
  readStarPromptState,
  recordStarPromptShown,
  recordStarPromptStarred,
  starPromptReason,
  type StarPromptInput,
} from "./starPrompt";
import type { ProviderUsageSnapshot } from "../types/bridge";

const NOW = 1_700_000_000_000;

function input(overrides: Partial<StarPromptInput> = {}): StarPromptInput {
  return {
    state: readStarPromptState(),
    version: "1.5.34",
    hasReading: true,
    readingSince: NOW - SETTLE_MS,
    now: NOW,
    ...overrides,
  };
}

function window(usedPercent: number, remainingPercent: number) {
  return {
    usedPercent,
    remainingPercent,
    windowMinutes: null,
    resetsAt: null,
    resetDescription: null,
    isExhausted: false,
    reservePercent: null,
    reserveDescription: null,
  };
}

function snapshot(
  overrides: Partial<ProviderUsageSnapshot> = {},
): ProviderUsageSnapshot {
  return {
    providerId: "claude",
    displayName: "Claude",
    primary: window(0, 100),
    secondary: null,
    modelSpecific: null,
    tertiary: null,
    extraRateWindows: [],
    cost: null,
    planName: null,
    accountEmail: null,
    sourceLabel: "test",
    updatedAt: new Date(NOW).toISOString(),
    error: null,
    pace: null,
    accountOrganization: null,
    trayStatusLabel: null,
    ...overrides,
  } as ProviderUsageSnapshot;
}

beforeEach(() => {
  localStorage.clear();
});

describe("hasRealReading", () => {
  it("counts a fresh account sitting at 0% used", () => {
    // The most common first reading a new user ever sees. Requiring usage
    // before asking would mean the first ask never fires for a light user.
    expect(hasRealReading([snapshot({ primary: window(0, 100) })])).toBe(true);
  });

  it("ignores a provider that errored", () => {
    expect(
      hasRealReading([
        snapshot({ error: "network unreachable", primary: window(0, 100) }),
      ]),
    ).toBe(false);
  });

  it("ignores an enabled provider reporting nothing", () => {
    expect(hasRealReading([snapshot({ primary: window(0, 0) })])).toBe(false);
  });

  it("counts a money-only provider with no metered window", () => {
    expect(
      hasRealReading([
        snapshot({
          primary: window(0, 0),
          cost: {
            used: 4.2,
            limit: null,
            remaining: null,
            currencyCode: "USD",
            period: "month",
            resetsAt: null,
            formattedUsed: "$4.20",
            formattedLimit: null,
          },
        }),
      ]),
    ).toBe(true);
  });

  it("is false with no providers at all", () => {
    expect(hasRealReading([])).toBe(false);
  });
});

describe("starPromptReason", () => {
  it("asks once a real reading has settled", () => {
    expect(starPromptReason(input())).toBe("firstValue");
  });

  it("stays quiet while the reading is still new", () => {
    // The numbers the user opened Ceiling for should be read first.
    expect(
      starPromptReason(input({ readingSince: NOW - SETTLE_MS + 1 })),
    ).toBeNull();
  });

  it("stays quiet with no reading on screen", () => {
    expect(
      starPromptReason(input({ hasReading: false, readingSince: null })),
    ).toBeNull();
  });

  it("stays quiet until the app version is known", () => {
    // Without a version the second ask could not be version-gated, so the
    // first one is held back rather than risk an ungated repeat.
    expect(starPromptReason(input({ version: null }))).toBeNull();
  });

  it("never asks again once the user went to GitHub", () => {
    recordStarPromptStarred();
    expect(starPromptReason(input({ state: readStarPromptState() }))).toBeNull();
  });

  it("never asks a third time", () => {
    recordStarPromptShown("1.5.30", NOW - 400 * MIN_GAP_MS);
    recordStarPromptShown("1.5.32", NOW - 200 * MIN_GAP_MS);
    expect(readStarPromptState().asked).toBe(MAX_ASKS);
    expect(starPromptReason(input({ state: readStarPromptState() }))).toBeNull();
  });

  it("asks a second time on a later version after the gap", () => {
    recordStarPromptShown("1.5.30", NOW - MIN_GAP_MS);
    expect(starPromptReason(input({ state: readStarPromptState() }))).toBe(
      "afterUpdate",
    );
  });

  it("does not ask again after a downgrade", () => {
    // Rolling back to an older build is not an update, and must not be spent
    // as one.
    recordStarPromptShown("1.5.40", NOW - 10 * MIN_GAP_MS);
    expect(
      starPromptReason(input({ state: readStarPromptState(), version: "1.5.39" })),
    ).toBeNull();
  });

  it("does not ask again when either version is not a plain triplet", () => {
    // A prerelease or locally built version cannot be ordered, so it is not
    // treated as a later release.
    recordStarPromptShown("1.5.30", NOW - 10 * MIN_GAP_MS);
    expect(
      starPromptReason(
        input({ state: readStarPromptState(), version: "1.5.34-rc.1" }),
      ),
    ).toBeNull();
  });

  it("does not ask again on the same version", () => {
    recordStarPromptShown("1.5.34", NOW - 10 * MIN_GAP_MS);
    expect(starPromptReason(input({ state: readStarPromptState() }))).toBeNull();
  });

  it("does not ask again the day after the first ask, even on a new version", () => {
    // A version bump alone is not consent to be asked twice in one week.
    recordStarPromptShown("1.5.30", NOW - 24 * 60 * 60 * 1000);
    expect(starPromptReason(input({ state: readStarPromptState() }))).toBeNull();
  });

  it("never asks on a corrupt record", () => {
    // A record we cannot read may already say "asked twice" or "starred".
    // Zeroing it would turn the lifetime cap into an ask on every launch.
    localStorage.setItem("ceiling.star-prompt.v1", "{not json");
    expect(readStarPromptState().unreadable).toBe(true);
    expect(starPromptReason(input({ state: readStarPromptState() }))).toBeNull();
  });

  it("never asks on a record with a field of the wrong type", () => {
    localStorage.setItem(
      "ceiling.star-prompt.v1",
      JSON.stringify({ asked: "1", starred: false }),
    );
    expect(readStarPromptState().unreadable).toBe(true);
    expect(starPromptReason(input({ state: readStarPromptState() }))).toBeNull();
  });

  it("never asks when storage itself cannot be read", () => {
    const getItem = vi
      .spyOn(Storage.prototype, "getItem")
      .mockImplementation(() => {
        throw new Error("storage disabled");
      });
    try {
      expect(readStarPromptState().unreadable).toBe(true);
      expect(
        starPromptReason(input({ state: readStarPromptState() })),
      ).toBeNull();
    } finally {
      getItem.mockRestore();
    }
  });

  it("still treats a missing key as a fresh install", () => {
    expect(readStarPromptState()).toEqual({
      asked: 0,
      starred: false,
      lastAskedVersion: null,
      lastAskedAt: null,
      unreadable: false,
    });
  });

  it("holds back the second ask when the first has no timestamp", () => {
    // An older record, or one written by a build that did not stamp the time:
    // unknown gap is treated as too soon.
    localStorage.setItem(
      "ceiling.star-prompt.v1",
      JSON.stringify({ asked: 1, starred: false, lastAskedVersion: "1.5.30" }),
    );
    expect(starPromptReason(input({ state: readStarPromptState() }))).toBeNull();
  });
});

describe("isLaterVersion", () => {
  it("orders plain triplets by release order, not string order", () => {
    expect(isLaterVersion("1.5.34", "1.5.9")).toBe(true);
    expect(isLaterVersion("1.10.0", "1.9.9")).toBe(true);
    expect(isLaterVersion("1.5.29", "1.5.30")).toBe(false);
    expect(isLaterVersion("1.5.34", "1.5.34")).toBe(false);
  });

  it("refuses to order anything that is not a plain triplet", () => {
    expect(isLaterVersion("1.5.34-rc.1", "1.5.30")).toBeNull();
    expect(isLaterVersion("1.5.34", "dev")).toBeNull();
    expect(isLaterVersion("1.5", "1.4")).toBeNull();
  });
});

describe("recordStarPromptShown", () => {
  it("carries the starred flag through a later ask being recorded", () => {
    recordStarPromptStarred();
    recordStarPromptShown("1.5.34", NOW);
    expect(readStarPromptState().starred).toBe(true);
  });

  it("reports failure when the record cannot be persisted", () => {
    // The caller must not show an ask it cannot count.
    const setItem = vi
      .spyOn(Storage.prototype, "setItem")
      .mockImplementation(() => {
        throw new Error("quota exceeded");
      });
    try {
      expect(recordStarPromptShown("1.5.34", NOW)).toBe(false);
    } finally {
      setItem.mockRestore();
    }
  });

  it("reports failure when a write is silently dropped", () => {
    const setItem = vi
      .spyOn(Storage.prototype, "setItem")
      .mockImplementation(() => {});
    try {
      // Nothing was stored, so the read back finds no record and the write
      // cannot be confirmed.
      expect(recordStarPromptShown("1.5.34", NOW)).toBe(false);
    } finally {
      setItem.mockRestore();
    }
  });
});
