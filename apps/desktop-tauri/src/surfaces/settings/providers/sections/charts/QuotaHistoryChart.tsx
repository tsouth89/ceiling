import { LineChart } from "../../../../../components/charts/LineChart";
import { providerCreditsColor } from "../../../../../components/charts/chartPalette";
import type { UsageHistoryPoint } from "../../../../../types/bridge";

interface Props {
  data: UsageHistoryPoint[];
  providerId: string;
  animations: boolean;
}

function quotaAxisLabel(value: string, spanMs: number): string {
  const date = new Date(value);
  if (Number.isNaN(date.getTime())) return value;
  if (spanMs < 24 * 60 * 60 * 1_000) {
    return date.toLocaleTimeString(undefined, {
      hour: "numeric",
      minute: "2-digit",
    });
  }
  return date.toLocaleDateString(undefined, {
    month: "short",
    day: "numeric",
    year: spanMs >= 365 * 24 * 60 * 60 * 1_000 ? "2-digit" : undefined,
  });
}

function quotaTooltipLabel(value: string): string {
  const date = new Date(value);
  if (Number.isNaN(date.getTime())) return value;
  return date.toLocaleString(undefined, {
    month: "short",
    day: "numeric",
    hour: "numeric",
    minute: "2-digit",
  });
}

export function QuotaHistoryChart({ data, providerId, animations }: Props) {
  const series = new Map<string, { label: string; points: Array<{ label: string; value: number }> }>();
  for (const sample of data) {
    for (const window of sample.windows) {
      const entry = series.get(window.id) ?? { label: window.label, points: [] };
      entry.label = window.label;
      entry.points.push({
        label: sample.recordedAt,
        value: window.usedPercent,
      });
      series.set(window.id, entry);
    }
  }

  return (
    <div className="quota-history">
      {[...series.entries()].map(([id, entry]) => {
        const latest = entry.points[entry.points.length - 1]?.value ?? 0;
        const firstAt = Date.parse(entry.points[0]?.label ?? "");
        const lastAt = Date.parse(entry.points[entry.points.length - 1]?.label ?? "");
        const spanMs = Number.isFinite(firstAt) && Number.isFinite(lastAt)
          ? Math.max(0, lastAt - firstAt)
          : 0;
        return (
          <div className="quota-history__series" key={id}>
            <div className="quota-history__header">
              <span>{entry.label}</span>
              <strong>{Math.round(latest)}% used</strong>
            </div>
            <LineChart
              data={entry.points}
              color={providerCreditsColor(providerId)}
              height={52}
              ariaLabel={`${entry.label} usage over time`}
              valueFormatter={(value) => `${Math.round(value)}%`}
              axisLabelFormatter={(label) => quotaAxisLabel(label, spanMs)}
              tooltipLabelFormatter={quotaTooltipLabel}
              animations={animations}
            />
          </div>
        );
      })}
    </div>
  );
}
