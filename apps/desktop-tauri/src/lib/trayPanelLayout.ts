/**
 * Pure sizing math for the tray flyout.
 *
 * Extracted from `useTrayPanelLayout` so the arithmetic can be tested without a
 * window, a Tauri bridge, or a live DOM. The hook keeps the effects: measuring
 * the surface, calling `setSize`, and re-anchoring.
 *
 * Positioning is NOT here. Where the flyout sits is decided by
 * `reanchorTrayPanel()` on the Rust side; this module only decides how big it is.
 */

export const TRAY_WIDTH = 328;
export const TRAY_MAX_MEASURE_HEIGHT = 920;
export const TRAY_OVERVIEW_MIN_HEIGHT = 200;
export const TRAY_DETAIL_MIN_HEIGHT = 420;
export const TRAY_DENSE_OVERVIEW_HEIGHT = 776;

/** Gap left between the flyout and the edge of the work area. */
const WORK_AREA_MARGIN = 16;

/** Slack added below the measured content so the last row is never clipped. */
const CONTENT_BOTTOM_PADDING = 4;

/**
 * Re-applying a size costs a window resize plus a re-anchor, so ignore
 * sub-pixel churn from the measure pass.
 */
const HEIGHT_CHANGE_EPSILON = 2;

/**
 * A user drag has to move the border by more than this before we treat it as
 * intentional. Physical pixels, compared against the last size we applied.
 */
const USER_RESIZE_TOLERANCE = 3;

export interface TraySizeInputs {
  detailMode: boolean;
  denseOverview: boolean;
}

/**
 * The floor for the flyout. Detail mode needs room for a provider pane, and a
 * dense overview is tall enough that starting small just causes a second resize.
 */
export function trayMinHeight({ detailMode, denseOverview }: TraySizeInputs): number {
  if (detailMode) return TRAY_DETAIL_MIN_HEIGHT;
  if (denseOverview) return TRAY_DENSE_OVERVIEW_HEIGHT;
  return TRAY_OVERVIEW_MIN_HEIGHT;
}

/**
 * The ceiling for the flyout: the work area minus a margin, capped by the
 * measure limit, but never below `minHeight`.
 *
 * `workAreaHeight` is null when the work-area query fails. Falling back to the
 * measure limit keeps the panel usable rather than collapsing it.
 *
 * Note the min-height floor wins over the work area. On a very short work area
 * the panel deliberately overflows rather than rendering unusably small.
 */
export function trayMaxHeight(
  minHeight: number,
  workAreaHeight: number | null,
): number {
  return Math.max(
    minHeight,
    Math.min(
      TRAY_MAX_MEASURE_HEIGHT,
      (workAreaHeight ?? TRAY_MAX_MEASURE_HEIGHT) - WORK_AREA_MARGIN,
    ),
  );
}

/** Clamp measured content into the allowed band. */
export function clampTrayHeight(
  contentHeight: number,
  minHeight: number,
  maxHeight: number,
): number {
  return Math.min(Math.max(contentHeight, minHeight), maxHeight);
}

export interface RectExtent {
  height: number;
  bottom: number;
}

export interface TrayContentMetrics {
  /** `getBoundingClientRect().top` of the tray surface. */
  surfaceTop: number;
  /** `getBoundingClientRect().height` of the tray surface. */
  surfaceHeight: number;
  /** `scrollHeight` of the tray surface. */
  surfaceScrollHeight: number;
  /** The scrollable body, when present. */
  body?: RectExtent | null;
  /** The pinned footer, when present. */
  footer?: RectExtent | null;
}

/**
 * How tall the content actually is, in logical pixels.
 *
 * The surface's own box is not enough: during the measure pass the body and
 * footer can extend past it, so the real extent is the lowest bottom edge of
 * the three. Zero-height rects are ignored because a hidden element reports
 * `bottom === top`, which would otherwise drag the measurement upward.
 */
export function measureTrayContentHeight({
  surfaceTop,
  surfaceHeight,
  surfaceScrollHeight,
  body = null,
  footer = null,
}: TrayContentMetrics): number {
  const ownHeight = Math.max(surfaceScrollHeight, Math.ceil(surfaceHeight));
  let maxBottom = surfaceTop + ownHeight;

  if (body && body.height > 0 && body.bottom > maxBottom) {
    maxBottom = body.bottom;
  }
  if (footer && footer.height > 0 && footer.bottom > maxBottom) {
    maxBottom = footer.bottom;
  }

  return Math.ceil(maxBottom - surfaceTop) + CONTENT_BOTTOM_PADDING;
}

export interface AppliedSize {
  width: number;
  height: number;
}

/**
 * Whether an auto-fit pass should actually resize the window.
 *
 * `previous` is null before the first auto-fit. Width is compared exactly since
 * it is a constant; height gets an epsilon because measurement jitters.
 */
export function shouldApplyAutoFitSize(
  previous: AppliedSize | null,
  next: AppliedSize,
): boolean {
  if (previous === null) return true;
  if (previous.width !== next.width) return true;
  return Math.abs(previous.height - next.height) > HEIGHT_CHANGE_EPSILON;
}

export interface ResizeEventContext {
  /** How many programmatic resizes are in flight. */
  programmaticInFlight: number;
  /** Actual physical size after the last resize we performed. */
  lastApplied: AppliedSize | null;
  /** Physical size reported by the event. */
  event: AppliedSize;
}

/**
 * Whether a resize event came from the user dragging the border.
 *
 * Everything here is in PHYSICAL pixels on both sides. Tauri's scale factor,
 * the webview's `devicePixelRatio`, and Win32 can disagree on a scaled display,
 * and converting between them is what previously compounded into a per-open
 * size growth. Comparing physical to physical needs no conversion at all.
 */
export function isUserDragResize({
  programmaticInFlight,
  lastApplied,
  event,
}: ResizeEventContext): boolean {
  if (programmaticInFlight > 0) return false;
  if (
    lastApplied &&
    Math.abs(event.width - lastApplied.width) <= USER_RESIZE_TOLERANCE &&
    Math.abs(event.height - lastApplied.height) <= USER_RESIZE_TOLERANCE
  ) {
    return false;
  }
  return true;
}
