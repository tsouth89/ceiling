import { useMemo, useRef, useState } from "react";
import { ChartTooltip } from "../../../../../components/charts/ChartTooltip";
import { serviceColorVar } from "../../../../../components/charts/chartPalette";
import { useChartAnimation } from "../../../../../components/charts/useChartAnimation";
import type { DailyUsageBreakdown } from "../../../../../types/bridge";

/**
 * UsageBreakdownChart — horizontal stacked bars, one row per day, each
 * segment proportional to that service's share of the day's total
 * credits. Rendered as pure SVG.
 *
 * Port target: the usage_breakdown stacked bar cluster in
 * `rust/src/native_ui/preferences.rs::render_provider_detail_panel`.
 *
 * Phase 10: tokenised palette (`--chart-service-*`), per-row entrance
 * animation, and hover tooltip.
 */

interface Props {
  data: DailyUsageBreakdown[];
  title: string;
  ariaLabel: string;
  animations: boolean;
  emptyMessage: string;
}

export function UsageBreakdownChart({
  data,
  title,
  ariaLabel,
  animations,
  emptyMessage,
}: Props) {
  const recent = data.slice(-14);
  const containerRef = useRef<HTMLDivElement | null>(null);
  const [hover, setHover] = useState<{
    label: string;
    value: number;
    x: number;
    y: number;
  } | null>(null);

  const anim = useChartAnimation(recent.length, animations, [
    recent.length,
    recent[0]?.day,
    recent[recent.length - 1]?.day,
  ]);

  const allServices = useMemo(
    () =>
      Array.from(new Set(recent.flatMap((d) => d.services.map((s) => s.service)))).sort(),
    [recent],
  );

  if (recent.length === 0) {
    return (
      <div className="provider-detail-chart">
        <div className="provider-detail-chart__title">{title}</div>
        <div className="chart chart--stacked">
          <div className="chart__empty">{emptyMessage}</div>
        </div>
      </div>
    );
  }

  const rowHeight = 14;
  const rowGap = 2;
  const labelWidth = 52;
  const totalWidth = 280;
  const barAreaWidth = totalWidth - labelWidth;
  const svgHeight = recent.length * (rowHeight + rowGap);

  const max = Math.max(...recent.map((d) => d.totalCreditsUsed), 0.0001);

  const onSegMove = (e: React.MouseEvent<SVGRectElement>, label: string, value: number) => {
    const host = containerRef.current;
    if (!host) return;
    const rect = host.getBoundingClientRect();
    setHover({ label, value, x: e.clientX - rect.left, y: e.clientY - rect.top });
  };
  const onSegLeave = () => setHover(null);

  return (
    <div className="provider-detail-chart">
      <div className="provider-detail-chart__title">{title}</div>
      <div className="chart chart--stacked" ref={containerRef}>
        <svg
          width={totalWidth}
          height={svgHeight}
          viewBox={`0 0 ${totalWidth} ${svgHeight}`}
          className="chart__svg"
          role="img"
          aria-label={ariaLabel}
        >
          {recent.map((day, rowIdx) => {
            const y = rowIdx * (rowHeight + rowGap);
            const eased = anim.barProgress(rowIdx);
            const rowWidth = (day.totalCreditsUsed / max) * barAreaWidth * eased;
            let xOffset = labelWidth;
            const sorted = [...day.services].sort((a, b) =>
              a.service.localeCompare(b.service),
            );
            return (
              <g key={day.day}>
                <text
                  x={0}
                  y={y + rowHeight - 3}
                  fontSize={10}
                  className="chart__row-label"
                  fill="var(--provider-row-text-secondary, #888)"
                >
                  {day.day.slice(-5)}
                </text>
                {sorted.map((svc) => {
                  const w =
                    day.totalCreditsUsed > 0
                      ? (svc.creditsUsed / day.totalCreditsUsed) * rowWidth
                      : 0;
                  const label = `${day.day} · ${svc.service}`;
                  const rect = (
                    <rect
                      key={`${day.day}-${svc.service}`}
                      x={xOffset}
                      y={y}
                      width={Math.max(0.5, w)}
                      height={rowHeight}
                      fill={serviceColorVar(svc.service, allServices)}
                      opacity={0.9}
                      rx={1}
                      className="chart__stack-rect"
                      onMouseMove={(e) => onSegMove(e, label, svc.creditsUsed)}
                      onMouseLeave={onSegLeave}
                    />
                  );
                  xOffset += w;
                  return rect;
                })}
              </g>
            );
          })}
        </svg>
        {allServices.length > 0 && (
          <div className="chart__legend">
            {allServices.slice(0, 8).map((svc) => (
              <span key={svc} className="chart__legend-item">
                <span
                  className="chart__legend-dot"
                  style={{ background: serviceColorVar(svc, allServices) }}
                />
                {svc}
              </span>
            ))}
          </div>
        )}
        {hover && !anim.running && (
          <ChartTooltip containerRef={containerRef} x={hover.x} y={hover.y}>
            <span className="chart__tooltip-label">{hover.label}</span>
            <strong>{hover.value.toFixed(2)}</strong>
          </ChartTooltip>
        )}
      </div>
    </div>
  );
}
