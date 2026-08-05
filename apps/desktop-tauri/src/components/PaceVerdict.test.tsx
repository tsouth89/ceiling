import { render, screen } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";

const tauriMocks = vi.hoisted(() => ({
  getLocaleStrings: vi.fn(),
  setUiLanguage: vi.fn(),
}));

const eventMocks = vi.hoisted(() => ({ listen: vi.fn() }));

vi.mock("../lib/tauri", async (importOriginal) => ({
  ...(await importOriginal<typeof import("../lib/tauri")>()),
  ...tauriMocks,
}));
vi.mock("@tauri-apps/api/event", () => eventMocks);

import { LocaleProvider } from "../i18n/LocaleProvider";
import { buildBundle } from "../test/localeHarness";
import type { PaceSnapshot } from "../types/bridge";
import PaceVerdict, { formatEta } from "./PaceVerdict";

function pace(overrides: Partial<PaceSnapshot> = {}): PaceSnapshot {
  return {
    windowLabel: "Weekly",
    stage: "on_track",
    deltaPercent: 0,
    willLastToReset: true,
    etaSeconds: null,
    expectedUsedPercent: 50,
    actualUsedPercent: 50,
    ...overrides,
  };
}

function renderVerdict(snapshot: PaceSnapshot) {
  return render(
    <LocaleProvider>
      <PaceVerdict pace={snapshot} />
    </LocaleProvider>,
  );
}

describe("PaceVerdict", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    eventMocks.listen.mockResolvedValue(() => {});
    tauriMocks.getLocaleStrings.mockResolvedValue(
      buildBundle({
        PaceVerdictOnTrack: "On track",
        PaceVerdictAhead: "Ahead of pace",
        PaceVerdictPlenty: "Plenty left",
        PaceVerdictRunningOut: "Running out early",
        PaceVerdictRunsOutIn: "Runs out in about {} at this pace",
        PaceVerdictLastsToReset: "Lasts to reset, {}% to spare",
      }),
    );
  });

  it("reports how much is left when usage lasts to reset", async () => {
    renderVerdict(pace({ actualUsedPercent: 74 }));

    expect(await screen.findByText("On track")).toBeInTheDocument();
    expect(screen.getByText("Lasts to reset, 26% to spare")).toBeInTheDocument();
  });

  it("leads with the shortfall when usage runs out first", async () => {
    renderVerdict(
      pace({
        stage: "far_ahead",
        willLastToReset: false,
        etaSeconds: 2 * 24 * 3600,
        actualUsedPercent: 82,
        expectedUsedPercent: 41,
      }),
    );

    expect(await screen.findByText("Running out early")).toBeInTheDocument();
    expect(
      screen.getByText("Runs out in about 2d at this pace"),
    ).toBeInTheDocument();
  });

  it("keeps a warning tone when a slow pace still runs out early", async () => {
    // Behind pace but still exhausting before reset: the shortfall is what
    // matters, so this must not render in the calm 'slow' colour.
    const { container } = renderVerdict(
      pace({
        stage: "far_behind",
        willLastToReset: false,
        etaSeconds: 3600,
        actualUsedPercent: 30,
      }),
    );

    await screen.findByText("Running out early");
    expect(
      container.querySelector(".menu-card__pace-verdict-title"),
    ).toHaveAttribute("data-pace", "racing");
  });

  it("places the tick at expected usage and the fill at actual", async () => {
    const { container } = renderVerdict(
      pace({
        stage: "far_behind",
        actualUsedPercent: 30,
        expectedUsedPercent: 60,
      }),
    );

    await screen.findByText("Plenty left");
    const fill = container.querySelector<HTMLElement>(
      ".menu-card__pace-verdict-fill",
    );
    const tick = container.querySelector<HTMLElement>(
      ".menu-card__pace-verdict-tick",
    );
    expect(fill?.style.width).toBe("30%");
    expect(tick?.style.left).toBe("60%");
  });

  it("never renders NaN in the spare-capacity line", async () => {
    // The bar already clamped these; the copy must agree with it rather than
    // rendering "NaN% to spare" beside a clamped bar.
    renderVerdict(pace({ actualUsedPercent: Number.NaN }));

    expect(await screen.findByText("Lasts to reset, 100% to spare")).toBeInTheDocument();
  });

  it("reports no spare capacity when usage overshoots the window", async () => {
    renderVerdict(pace({ actualUsedPercent: 140 }));

    expect(await screen.findByText("Lasts to reset, 0% to spare")).toBeInTheDocument();
  });

  it("clamps out-of-range percentages instead of overflowing the track", async () => {
    const { container } = renderVerdict(
      pace({ actualUsedPercent: 140, expectedUsedPercent: -20 }),
    );

    await screen.findByText("On track");
    expect(
      container.querySelector<HTMLElement>(".menu-card__pace-verdict-fill")
        ?.style.width,
    ).toBe("100%");
    expect(
      container.querySelector<HTMLElement>(".menu-card__pace-verdict-tick")
        ?.style.left,
    ).toBe("0%");
  });
});

describe("formatEta", () => {
  it("formats coarse durations without noise", () => {
    expect(formatEta(45 * 60)).toBe("45m");
    expect(formatEta(3 * 3600)).toBe("3h");
    expect(formatEta(3 * 3600 + 25 * 60)).toBe("3h 25m");
    expect(formatEta(2 * 24 * 3600)).toBe("2d");
    expect(formatEta(2 * 24 * 3600 + 5 * 3600)).toBe("2d 5h");
  });

  it("does not render negative or non-finite time", () => {
    expect(formatEta(-10)).toBe("0m");
    expect(formatEta(Number.NaN)).toBe("0m");
  });
});
