import { useLayoutEffect, useRef, useState, type ReactNode, type RefObject } from "react";

const TOOLTIP_MARGIN = 8;

export function clampChartTooltipLeft(
  anchorX: number,
  tooltipWidth: number,
  containerWidth: number,
  margin = TOOLTIP_MARGIN,
): number {
  const centered = anchorX - tooltipWidth / 2;
  const maximum = Math.max(margin, containerWidth - tooltipWidth - margin);
  return Math.min(Math.max(centered, margin), maximum);
}

interface ChartTooltipProps {
  containerRef: RefObject<HTMLDivElement | null>;
  x: number;
  y: number;
  children: ReactNode;
}

/** A measured tooltip that stays inside its chart container. */
export function ChartTooltip({ containerRef, x, y, children }: ChartTooltipProps) {
  const tooltipRef = useRef<HTMLDivElement | null>(null);
  const [left, setLeft] = useState(TOOLTIP_MARGIN);

  useLayoutEffect(() => {
    const container = containerRef.current;
    const tooltip = tooltipRef.current;
    if (!container || !tooltip) return;

    const nextLeft = clampChartTooltipLeft(
      x,
      tooltip.getBoundingClientRect().width,
      container.getBoundingClientRect().width,
    );
    setLeft((current) => (current === nextLeft ? current : nextLeft));
  }, [containerRef, x, children]);

  return (
    <div
      ref={tooltipRef}
      className="chart__tooltip"
      style={{ left, top: y }}
      role="tooltip"
    >
      {children}
    </div>
  );
}
