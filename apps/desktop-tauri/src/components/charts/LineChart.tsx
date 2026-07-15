import { useId, useRef, useState } from "react";
import { chartTooltipPosition, type ChartTooltipPosition } from "./chartTooltip";
import { useChartAnimation } from "./useChartAnimation";

/**
 * LineChart — dependency-free SVG line chart with optional area fill,
 * entrance animation that sweeps the polyline up from the baseline,
 * and per-point hover tooltip.
 *
 * Port target: the credits-history line in
 * `rust/src/native_ui/charts.rs`.
 */

export interface LineChartPoint {
  label: string;
  value: number;
}

export interface LineChartProps {
  data: LineChartPoint[];
  color?: string;
  height?: number;
  valueFormatter?: (n: number) => string;
  axisLabelFormatter?: (label: string) => string;
  tooltipLabelFormatter?: (label: string) => string;
  ariaLabel: string;
  /** When true, render a faint filled area under the line. Defaults true. */
  area?: boolean;
  animations?: boolean;
  emptyMessage?: string;
}

const DEFAULT_COLOR = "var(--chart-credits)";
const SVG_WIDTH = 280;

/**
 * Renders an accessible SVG line chart with optional area fill, animations, formatting, and hover tooltips.
 *
 * @param data - The labeled numeric points to plot.
 * @param ariaLabel - Accessible name for the chart.
 * @returns The rendered line chart or its empty state when no data is provided.
 */
export function LineChart({
  data,
  color = DEFAULT_COLOR,
  height = 56,
  valueFormatter,
  axisLabelFormatter,
  tooltipLabelFormatter,
  ariaLabel,
  area = true,
  animations = true,
  emptyMessage,
}: LineChartProps) {
  const fmt = valueFormatter ?? ((v: number) => v.toFixed(2));
  const axisFmt = axisLabelFormatter ?? ((label: string) => label.slice(-5));
  const tooltipFmt = tooltipLabelFormatter ?? ((label: string) => label);
  const containerRef = useRef<HTMLDivElement | null>(null);
  const clipId = useId().replace(/:/g, "");
  const [hover, setHover] = useState<(ChartTooltipPosition & { i: number }) | null>(null);

  const anim = useChartAnimation(data.length, animations, [
    data.length,
    data[0]?.label,
    data[data.length - 1]?.label,
  ]);

  if (data.length === 0) {
    return (
      <div className="chart chart--line">
        <div className="chart__empty">{emptyMessage ?? ""}</div>
      </div>
    );
  }

  const values = data.map((p) => p.value);
  const max = Math.max(...values, 0.0001);
  const min = Math.min(...values, 0);
  const range = Math.max(max - min, 0.0001);

  const plotHeight = Math.max(1, height - 4);
  const pad = 2;
  const usableWidth = SVG_WIDTH - pad * 2;

  const baselineY = pad + plotHeight;

  const step = data.length > 1 ? usableWidth / (data.length - 1) : 0;
  const coords = data.map((p, i) => {
    const x = pad + i * step;
    const finalY = pad + plotHeight - ((p.value - min) / range) * plotHeight;
    return { x, y: finalY };
  });

  if (coords.length === 1) {
    coords.push({ x: pad + usableWidth, y: coords[0].y });
  }

  const polyline = coords.map((c) => `${c.x.toFixed(1)},${c.y.toFixed(1)}`).join(" ");

  const areaPath = area
    ? [
        `M ${coords[0].x.toFixed(1)} ${baselineY.toFixed(1)}`,
        ...coords.map((c) => `L ${c.x.toFixed(1)} ${c.y.toFixed(1)}`),
        `L ${coords[coords.length - 1].x.toFixed(1)} ${baselineY.toFixed(1)}`,
        "Z",
      ].join(" ")
    : null;

  const onPointMove = (e: React.MouseEvent<SVGCircleElement>, i: number) => {
    const host = containerRef.current;
    if (!host) return;
    const rect = host.getBoundingClientRect();
    setHover({ i, ...chartTooltipPosition(e.clientX, e.clientY, rect) });
  };
  const onLeave = () => setHover(null);

  return (
    <div className="chart chart--line" ref={containerRef}>
      <svg
        width={SVG_WIDTH}
        height={height}
        viewBox={`0 0 ${SVG_WIDTH} ${height}`}
        className="chart__svg"
        role="img"
        aria-label={ariaLabel}
      >
        <defs>
          <clipPath id={clipId}>
            <rect
              x={0}
              y={0}
              width={SVG_WIDTH}
              height={height}
              className={anim.enabled ? "chart__reveal" : undefined}
              style={anim.enabled ? { animationDuration: `${anim.durationMs}ms` } : undefined}
            />
          </clipPath>
        </defs>
        <g clipPath={`url(#${clipId})`}>
          {areaPath && (
            <path d={areaPath} fill={color} opacity={0.18} className="chart__area" />
          )}
          <polyline
            points={polyline}
            fill="none"
            stroke={color}
            strokeWidth={1.5}
            strokeLinejoin="round"
            strokeLinecap="round"
            opacity={0.95}
            className="chart__line"
          />
          {data.map((p, i) => (
            <circle
              key={`${p.label}-${i}`}
              cx={coords[i].x}
              cy={coords[i].y}
              r={hover?.i === i ? 3 : 1.8}
              fill={color}
              className="chart__point"
              onMouseMove={(e) => onPointMove(e, i)}
              onMouseLeave={onLeave}
            >
              <title>
                {tooltipFmt(p.label)}: {fmt(p.value)}
              </title>
            </circle>
          ))}
        </g>
      </svg>
      <div className="chart__axis">
        <span>{axisFmt(data[0].label)}</span>
        <span className="chart__axis-max">{fmt(max)}</span>
        <span>{axisFmt(data[data.length - 1].label)}</span>
      </div>
      {hover && !anim.running && (
        <div
          className={`chart__tooltip chart__tooltip--${hover.alignment}`}
          style={{ left: hover.x, top: hover.y }}
          role="tooltip"
        >
          <span className="chart__tooltip-label">{tooltipFmt(data[hover.i].label)}</span>
          <strong>{fmt(data[hover.i].value)}</strong>
        </div>
      )}
    </div>
  );
}
