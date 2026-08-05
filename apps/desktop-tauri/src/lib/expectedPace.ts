import type { RateWindowSnapshot } from "../types/bridge";

/**
 * Windows shorter than this get no expected-usage marker.
 *
 * "Where you should be by now" assumes consumption spreads evenly across the
 * window. That is a fair budget for a weekly or monthly quota, but meaningless
 * for a 5-hour session: nobody paces a session linearly, so a marker there
 * would sweep across the bar and mean nothing.
 */
const MIN_WINDOW_MINUTES = 12 * 60;

/** Below this the fill edge and the marker are the same pixel; drawing a gap
 * there is visual noise rather than information. */
const MIN_GAP_PERCENT = 1.5;

export interface ExpectedOverlay {
  /** Marker position along the bar, already mirrored for the display mode. */
  tickPercent: number;
  /** Shaded span between the fill edge and the marker, when worth drawing. */
  gap: { left: number; width: number } | null;
  /** True when more has been used than the calendar says it should be. */
  ahead: boolean;
  /** Expected usage as a percentage, for labelling. */
  expectedUsedPercent: number;
}

/**
 * Where usage *should* be by this point in the window, as a percentage.
 *
 * Derived from elapsed time alone, so it works for any window that reports a
 * duration and a reset — no pace calculation and no provider support needed.
 * Returns null when the window is too short to be worth marking, or when the
 * provider does not report enough to place the marker honestly.
 */
export function expectedUsedPercent(
  snap: RateWindowSnapshot,
  nowMs: number = Date.now(),
): number | null {
  const minutes = snap.windowMinutes;
  if (minutes == null || !Number.isFinite(minutes) || minutes <= 0) return null;
  if (minutes < MIN_WINDOW_MINUTES) return null;
  if (!snap.resetsAt) return null;

  const resetMs = Date.parse(snap.resetsAt);
  if (!Number.isFinite(resetMs)) return null;

  const durationMs = minutes * 60 * 1000;
  const startMs = resetMs - durationMs;
  const elapsed = nowMs - startMs;
  if (elapsed <= 0 || elapsed >= durationMs) return null;

  return (elapsed / durationMs) * 100;
}

/**
 * Everything needed to draw the expected-usage overlay on a bar.
 *
 * The bar can show either used or remaining capacity, so the marker is mirrored
 * to match: in both modes it lands where the bar's edge *should* be right now.
 * `ahead` is computed from usage rather than from bar geometry, so it means the
 * same thing in either mode.
 */
export function expectedOverlay(
  snap: RateWindowSnapshot,
  showAsUsed: boolean,
  nowMs: number = Date.now(),
): ExpectedOverlay | null {
  const expected = expectedUsedPercent(snap, nowMs);
  if (expected == null) return null;

  const used = clamp(snap.usedPercent);
  const edge = showAsUsed ? used : clamp(snap.remainingPercent);
  const tickPercent = clamp(showAsUsed ? expected : 100 - expected);

  const lo = Math.min(edge, tickPercent);
  const hi = Math.max(edge, tickPercent);
  const width = hi - lo;
  const ahead = used > expected;

  return {
    tickPercent,
    // Only shade when overspending: that is the actionable case, and a band on
    // every bar all the time would be noise.
    gap: ahead && width >= MIN_GAP_PERCENT ? { left: lo, width } : null,
    ahead,
    expectedUsedPercent: expected,
  };
}

function clamp(value: number): number {
  if (!Number.isFinite(value)) return 0;
  return Math.min(100, Math.max(0, value));
}
