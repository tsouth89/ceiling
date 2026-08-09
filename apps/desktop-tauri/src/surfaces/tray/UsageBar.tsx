import type { RateWindowSnapshot } from "../../types/bridge";

interface UsageBarProps {
  window: RateWindowSnapshot;
  label?: string;
  compact?: boolean;
}

type UsageLevel = "normal" | "high" | "critical" | "exhausted";

export function usageLevel(pct: number, exhausted: boolean): UsageLevel {
  if (exhausted) return "exhausted";
  if (pct >= 90) return "critical";
  if (pct >= 70) return "high";
  return "normal";
}

export default function UsageBar({ window: w, label, compact }: UsageBarProps) {
  const rawPct = Number.isFinite(w.usedPercent) ? Math.max(0, w.usedPercent) : 0;
  const pct = Math.min(100, rawPct);
  const level = usageLevel(rawPct, w.isExhausted);

  return (
    <div className={`usage-bar ${compact ? "usage-bar--compact" : ""}`}>
      {label && <span className="usage-bar__label">{label}</span>}
      <div
        className="usage-bar__track"
        role="progressbar"
        aria-valuenow={pct}
        aria-valuemin={0}
        aria-valuemax={100}
        aria-label={
          label
            ? `${label} usage: ${rawPct.toFixed(0)}%`
            : `Usage: ${rawPct.toFixed(0)}%`
        }
      >
        <div
          className="usage-bar__fill"
          data-level={level}
          style={{ width: `${pct}%` }}
        />
      </div>
      <span className="usage-bar__pct">{rawPct.toFixed(0)}%</span>
    </div>
  );
}