export type ChartTooltipAlignment = "start" | "center" | "end";

export interface ChartTooltipPosition {
  x: number;
  y: number;
  alignment: ChartTooltipAlignment;
}

const EDGE_GUARD = 96;
const INSET = 4;

/**
 * Calculates tooltip coordinates relative to a host element and selects their horizontal alignment.
 *
 * @param clientX - The pointer's horizontal client coordinate
 * @param clientY - The pointer's vertical client coordinate
 * @param hostRect - The host element's position and width
 * @returns The relative coordinates and horizontal tooltip alignment
 */
export function chartTooltipPosition(
  clientX: number,
  clientY: number,
  hostRect: Pick<DOMRect, "left" | "top" | "width">,
): ChartTooltipPosition {
  const rawX = clientX - hostRect.left;
  const x = Math.max(INSET, Math.min(hostRect.width - INSET, rawX));
  const alignment = x <= EDGE_GUARD
    ? "start"
    : x >= hostRect.width - EDGE_GUARD
      ? "end"
      : "center";
  return { x, y: clientY - hostRect.top, alignment };
}
