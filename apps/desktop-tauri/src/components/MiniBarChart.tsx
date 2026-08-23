/**
 * MiniBarChart — lightweight SVG bar chart with no external dependencies.
 * Used in ProviderDetail for cost history, credits history, and usage breakdown.
 */

import type { DailyCostPoint, DailyUsageBreakdown } from "../types/bridge";
import { useLocale } from "../hooks/useLocale";

interface BarChartProps {
  points: DailyCostPoint[];
  color?: string;
  height?: number;
  label?: string;
  formatValue?: (v: number) => string;
}

/** Simple bar chart for daily cost or credits history. */
export function SimpleBarChart({
  points,
  color = "#5d87ff",
  height = 48,
  label,
  formatValue,
}: BarChartProps) {
  const { t } = useLocale();
  if (points.length === 0) {
    const emptyText = t("MiniChartNoData");
    return (
      <div
        className="mini-chart mini-chart--empty"
        role="img"
        aria-label={label ? `${label}: ${emptyText}` : emptyText}
      >
        {label && <span className="mini-chart__label">{label}</span>}
        <span className="mini-chart__empty-msg">{emptyText}</span>
      </div>
    );
  }

  const max = Math.max(...points.map((p) => p.value), 0.0001);
  const BAR_GAP = 2;
  const fmt = formatValue ?? ((v: number) => v.toFixed(2));

  // Show at most 30 bars; abbreviate label to last 2 chars of date
  const visible = points.slice(-30);
  const svgWidth = 280;
  const barWidth = Math.max(
    1,
    Math.floor((svgWidth - (visible.length - 1) * BAR_GAP) / visible.length),
  );
  const actualWidth = visible.length * barWidth + (visible.length - 1) * BAR_GAP;

  return (
    <div className="mini-chart">
      {label && <span className="mini-chart__label">{label}</span>}
      <svg
        width={actualWidth}
        height={height}
        viewBox={`0 0 ${actualWidth} ${height}`}
        className="mini-chart__svg"
        aria-label={label ?? "bar chart"}
      >
        {visible.map((p, i) => {
          const barH = Math.max(1, (p.value / max) * (height - 4));
          const x = i * (barWidth + BAR_GAP);
          const y = height - barH;
          return (
            <rect
              key={p.date + i}
              x={x}
              y={y}
              width={barWidth}
              height={barH}
              fill={color}
              opacity={p.value === 0 ? 0.25 : 0.9}
              rx={1}
            >
              <title>
                {p.date}: {fmt(p.value)}
              </title>
            </rect>
          );
        })}
      </svg>
      <div className="mini-chart__axis">
        {visible.length > 0 && (
          <>
            <span>{visible[0].date.slice(-5)}</span>
            <span>{fmt(max)}</span>
            <span>{visible[visible.length - 1].date.slice(-5)}</span>
          </>
        )}
      </div>
    </div>
  );
}

// Deterministic palette for service names
const SERVICE_COLORS = [
  "#5d87ff",
  "#06d6a0",
  "#ffd166",
  "#ef476f",
  "#a78bfa",
  "#38bdf8",
  "#fb923c",
  "#4ade80",
];

function serviceColor(name: string, allServices: string[]): string {
  const idx = allServices.indexOf(name);
  return SERVICE_COLORS[idx % SERVICE_COLORS.length];
}

interface StackedBarChartProps {
  points: DailyUsageBreakdown[];
  height?: number;
  label?: string;
}

/** Stacked bar chart for daily usage breakdown by service. */
export function StackedBarChart({
  points,
  height = 64,
  label,
}: StackedBarChartProps) {
  const { t } = useLocale();
  if (points.length === 0) {
    const emptyText = t("MiniChartNoData");
    return (
      <div
        className="mini-chart mini-chart--empty"
        role="img"
        aria-label={label ? `${label}: ${emptyText}` : emptyText}
      >
        {label && <span className="mini-chart__label">{label}</span>}
        <span className="mini-chart__empty-msg">{emptyText}</span>
      </div>
    );
  }

  const visible = points.slice(-30);
  const max = Math.max(...visible.map((p) => p.totalCreditsUsed), 0.0001);

  // Collect all unique service names for consistent coloring
  const allServices = Array.from(
    new Set(visible.flatMap((p) => p.services.map((s) => s.service))),
  ).sort();

  const BAR_GAP = 2;
  const svgWidth = 280;
  const barWidth = Math.max(
    1,
    Math.floor((svgWidth - (visible.length - 1) * BAR_GAP) / visible.length),
  );
  const actualWidth = visible.length * barWidth + (visible.length - 1) * BAR_GAP;

  return (
    <div className="mini-chart">
      {label && <span className="mini-chart__label">{label}</span>}
      <svg
        width={actualWidth}
        height={height}
        viewBox={`0 0 ${actualWidth} ${height}`}
        className="mini-chart__svg"
        aria-label={label ?? "stacked bar chart"}
      >
        {visible.map((p, i) => {
          const x = i * (barWidth + BAR_GAP);
          const totalH = Math.max(1, (p.totalCreditsUsed / max) * (height - 4));
          // Sort services to stack predictably
          const sorted = [...p.services].sort((a, b) =>
            a.service.localeCompare(b.service),
          );
          let yOffset = height - totalH;
          return sorted.map((s) => {
            const segH = (s.creditsUsed / max) * (height - 4);
            const segY = yOffset;
            yOffset += segH;
            return (
              <rect
                key={`${p.day}-${s.service}`}
                x={x}
                y={segY}
                width={barWidth}
                height={Math.max(0.5, segH)}
                fill={serviceColor(s.service, allServices)}
                opacity={0.9}
                rx={1}
              >
                <title>
                  {p.day} {s.service}: {s.creditsUsed.toFixed(2)} credits
                </title>
              </rect>
            );
          });
        })}
      </svg>

      {/* Legend */}
      {allServices.length > 0 && (
        <div className="mini-chart__legend">
          {allServices.slice(0, 6).map((svc) => (
            <span key={svc} className="mini-chart__legend-item">
              <span
                className="mini-chart__legend-dot"
                style={{ background: serviceColor(svc, allServices) }}
              />
              {svc}
            </span>
          ))}
        </div>
      )}

      <div className="mini-chart__axis">
        {visible.length > 0 && (
          <>
            <span>{visible[0].day.slice(-5)}</span>
            <span>{max.toFixed(1)}</span>
            <span>{visible[visible.length - 1].day.slice(-5)}</span>
          </>
        )}
      </div>
    </div>
  );
}
