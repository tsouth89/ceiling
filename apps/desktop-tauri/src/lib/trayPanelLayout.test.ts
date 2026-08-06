import { describe, it, expect } from "vitest";

import {
  clampTrayHeight,
  isUserDragResize,
  measureTrayContentHeight,
  shouldApplyAutoFitSize,
  trayMaxHeight,
  trayMinHeight,
  TRAY_DENSE_OVERVIEW_HEIGHT,
  TRAY_DETAIL_MIN_HEIGHT,
  TRAY_MAX_MEASURE_HEIGHT,
  TRAY_OVERVIEW_MIN_HEIGHT,
  TRAY_WIDTH,
} from "./trayPanelLayout";

describe("trayMinHeight", () => {
  it("gives detail mode the tallest floor", () => {
    expect(trayMinHeight({ detailMode: true, denseOverview: false })).toBe(
      TRAY_DETAIL_MIN_HEIGHT,
    );
  });

  it("prefers detail mode over dense overview when both are set", () => {
    expect(trayMinHeight({ detailMode: true, denseOverview: true })).toBe(
      TRAY_DETAIL_MIN_HEIGHT,
    );
  });

  it("uses the dense floor for a dense overview", () => {
    expect(trayMinHeight({ detailMode: false, denseOverview: true })).toBe(
      TRAY_DENSE_OVERVIEW_HEIGHT,
    );
  });

  it("falls back to the plain overview floor", () => {
    expect(trayMinHeight({ detailMode: false, denseOverview: false })).toBe(
      TRAY_OVERVIEW_MIN_HEIGHT,
    );
  });
});

describe("trayMaxHeight", () => {
  it("leaves a margin below the work area", () => {
    expect(trayMaxHeight(TRAY_OVERVIEW_MIN_HEIGHT, 800)).toBe(784);
  });

  it("never exceeds the measure limit on a tall screen", () => {
    expect(trayMaxHeight(TRAY_OVERVIEW_MIN_HEIGHT, 4000)).toBe(
      TRAY_MAX_MEASURE_HEIGHT,
    );
  });

  it("falls back to the measure limit when the work area is unknown", () => {
    // getWorkAreaRect() rejected. Collapsing the panel would be worse than
    // guessing, so the fallback is the measure limit minus the margin.
    expect(trayMaxHeight(TRAY_OVERVIEW_MIN_HEIGHT, null)).toBe(
      TRAY_MAX_MEASURE_HEIGHT - 16,
    );
  });

  it("lets the min height win on a very short work area", () => {
    // A 300px work area would otherwise cap detail mode at 284, below its own
    // floor. The panel deliberately overflows rather than rendering unusably
    // small.
    expect(trayMaxHeight(TRAY_DETAIL_MIN_HEIGHT, 300)).toBe(
      TRAY_DETAIL_MIN_HEIGHT,
    );
  });

  it("returns a max that is never below the min it was given", () => {
    for (const workArea of [0, 1, 100, 300, 500, 900, 2000, null]) {
      expect(
        trayMaxHeight(TRAY_DETAIL_MIN_HEIGHT, workArea),
      ).toBeGreaterThanOrEqual(TRAY_DETAIL_MIN_HEIGHT);
    }
  });
});

describe("clampTrayHeight", () => {
  it("keeps content that already fits", () => {
    expect(clampTrayHeight(500, 200, 900)).toBe(500);
  });

  it("raises content shorter than the floor", () => {
    expect(clampTrayHeight(50, 200, 900)).toBe(200);
  });

  it("caps content taller than the ceiling", () => {
    expect(clampTrayHeight(5000, 200, 900)).toBe(900);
  });

  it("returns the floor when floor and ceiling collapse together", () => {
    expect(clampTrayHeight(10, 420, 420)).toBe(420);
    expect(clampTrayHeight(9000, 420, 420)).toBe(420);
  });
});

describe("measureTrayContentHeight", () => {
  const base = {
    surfaceTop: 0,
    surfaceHeight: 300,
    surfaceScrollHeight: 300,
  };

  it("uses the surface's own extent plus bottom padding", () => {
    expect(measureTrayContentHeight(base)).toBe(304);
  });

  it("prefers scrollHeight when the surface is visually clipped", () => {
    expect(
      measureTrayContentHeight({ ...base, surfaceHeight: 300, surfaceScrollHeight: 700 }),
    ).toBe(704);
  });

  it("rounds a fractional surface height up", () => {
    expect(
      measureTrayContentHeight({ ...base, surfaceHeight: 300.2, surfaceScrollHeight: 0 }),
    ).toBe(305);
  });

  it("extends to a body that overflows the surface", () => {
    expect(
      measureTrayContentHeight({ ...base, body: { height: 500, bottom: 520 } }),
    ).toBe(524);
  });

  it("extends to a footer that sits below the body", () => {
    expect(
      measureTrayContentHeight({
        ...base,
        body: { height: 500, bottom: 520 },
        footer: { height: 40, bottom: 560 },
      }),
    ).toBe(564);
  });

  it("ignores a zero-height body or footer", () => {
    // A hidden element reports bottom === top, which would otherwise drag the
    // measurement upward and clip the panel.
    expect(
      measureTrayContentHeight({
        ...base,
        body: { height: 0, bottom: 9999 },
        footer: { height: 0, bottom: 9999 },
      }),
    ).toBe(304);
  });

  it("ignores a body that does not reach past the surface", () => {
    expect(
      measureTrayContentHeight({ ...base, body: { height: 100, bottom: 120 } }),
    ).toBe(304);
  });

  it("measures relative to the surface, not the viewport", () => {
    // The surface can be scrolled away from the top of the document. Height is
    // an extent, so a non-zero top must not inflate it.
    expect(
      measureTrayContentHeight({
        surfaceTop: 150,
        surfaceHeight: 300,
        surfaceScrollHeight: 300,
        footer: { height: 40, bottom: 470 },
      }),
    ).toBe(324);
  });

  it("handles a negative surfaceTop from an off-screen surface", () => {
    expect(
      measureTrayContentHeight({
        surfaceTop: -80,
        surfaceHeight: 300,
        surfaceScrollHeight: 300,
      }),
    ).toBe(304);
  });
});

describe("shouldApplyAutoFitSize", () => {
  it("always resizes on the first pass", () => {
    expect(shouldApplyAutoFitSize(null, { width: TRAY_WIDTH, height: 400 })).toBe(
      true,
    );
  });

  it("resizes when the width changes", () => {
    expect(
      shouldApplyAutoFitSize(
        { width: 300, height: 400 },
        { width: TRAY_WIDTH, height: 400 },
      ),
    ).toBe(true);
  });

  it("ignores height jitter within the epsilon", () => {
    // Measurement noise must not cost a resize plus a re-anchor every pass.
    expect(
      shouldApplyAutoFitSize(
        { width: TRAY_WIDTH, height: 400 },
        { width: TRAY_WIDTH, height: 402 },
      ),
    ).toBe(false);
    expect(
      shouldApplyAutoFitSize(
        { width: TRAY_WIDTH, height: 400 },
        { width: TRAY_WIDTH, height: 398 },
      ),
    ).toBe(false);
  });

  it("resizes once the height moves past the epsilon", () => {
    expect(
      shouldApplyAutoFitSize(
        { width: TRAY_WIDTH, height: 400 },
        { width: TRAY_WIDTH, height: 403 },
      ),
    ).toBe(true);
  });

  it("does not resize when nothing changed", () => {
    expect(
      shouldApplyAutoFitSize(
        { width: TRAY_WIDTH, height: 400 },
        { width: TRAY_WIDTH, height: 400 },
      ),
    ).toBe(false);
  });
});

describe("isUserDragResize", () => {
  const lastApplied = { width: 656, height: 800 };

  it("ignores events while a programmatic resize is in flight", () => {
    expect(
      isUserDragResize({
        programmaticInFlight: 1,
        lastApplied,
        event: { width: 1200, height: 1200 },
      }),
    ).toBe(false);
  });

  it("ignores an echo of the size we just applied", () => {
    expect(
      isUserDragResize({
        programmaticInFlight: 0,
        lastApplied,
        event: { width: 656, height: 800 },
      }),
    ).toBe(false);
  });

  it("ignores rounding noise within tolerance on both axes", () => {
    expect(
      isUserDragResize({
        programmaticInFlight: 0,
        lastApplied,
        event: { width: 659, height: 797 },
      }),
    ).toBe(false);
  });

  it("reports a drag once either axis moves past tolerance", () => {
    expect(
      isUserDragResize({
        programmaticInFlight: 0,
        lastApplied,
        event: { width: 660, height: 800 },
      }),
    ).toBe(true);
    expect(
      isUserDragResize({
        programmaticInFlight: 0,
        lastApplied,
        event: { width: 656, height: 804 },
      }),
    ).toBe(true);
  });

  it("treats any resize as a drag before we have applied a size", () => {
    expect(
      isUserDragResize({
        programmaticInFlight: 0,
        lastApplied: null,
        event: { width: 656, height: 800 },
      }),
    ).toBe(true);
  });

  it("stays suppressed while several programmatic resizes overlap", () => {
    // The auto-fit pass fires a burst of setSize calls; the counter must gate
    // all of them, not just the first.
    expect(
      isUserDragResize({
        programmaticInFlight: 3,
        lastApplied: null,
        event: { width: 999, height: 999 },
      }),
    ).toBe(false);
  });
});
