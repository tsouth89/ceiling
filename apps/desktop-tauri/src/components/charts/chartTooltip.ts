export type ChartTooltipAlignment = "start" | "center" | "end";

export interface ChartTooltipPosition {
  x: number;
  y: number;
  alignment: ChartTooltipAlignment;
}

const EDGE_GUARD = 96;
const INSET = 4;

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
