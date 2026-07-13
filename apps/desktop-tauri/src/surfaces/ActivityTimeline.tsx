import { Fragment, useEffect, useMemo, useState } from "react";
import type { ProviderUsageSnapshot, RateWindowSnapshot } from "../types/bridge";
import { ProviderIcon } from "../components/providers/ProviderIcon";
import { allMeasuredWindows } from "../lib/capacityPresentation";

/**
 * Activity = an "upcoming resets" timeline. Each provider exposes one or more
 * rate windows (plan / session / weekly / model / extra), and every window
 * carries a `resetsAt`. We enumerate them all, sort by soonest reset, and group
 * by how far out they are — so at a glance you can see what frees up next and in
 * what order. This is built purely from the live snapshot (no fabricated
 * history); the Charts tab owns cost-over-time.
 */

type TimelineEntry = {
  key: string;
  providerId: string;
  displayName: string;
  label: string;
  window: RateWindowSnapshot;
  /** Parsed reset time in ms; null when the window has no known reset. */
  resetMs: number | null;
};

type Bucket = {
  id: string;
  label: string;
  entries: TimelineEntry[];
};

const DAY_MS = 24 * 60 * 60 * 1000;

/** Same urgency scale the dashboard cards use, keyed off remaining headroom. */
function levelOf(window: RateWindowSnapshot): string {
  if (window.isExhausted) return "exhausted";
  const remain = window.remainingPercent;
  if (remain <= 5) return "critical";
  if (remain <= 25) return "high";
  return "normal";
}

/** Language-neutral short duration: "6d 13h" / "1h 27m" / "12m" / "now". */
function shortDuration(ms: number): string {
  if (ms <= 0) return "now";
  const totalMin = Math.floor(ms / 60_000);
  const d = Math.floor(totalMin / 1440);
  const h = Math.floor((totalMin % 1440) / 60);
  const m = totalMin % 60;
  if (d > 0) return `${d}d ${h}h`;
  if (h > 0) return `${h}h ${m}m`;
  return `${m}m`;
}

function collectEntries(providers: ProviderUsageSnapshot[]): TimelineEntry[] {
  const entries: TimelineEntry[] = [];
  for (const provider of providers) {
    if (provider.error) continue;
    for (const measured of allMeasuredWindows(provider)) {
      const parsed = measured.window.resetsAt
        ? Date.parse(measured.window.resetsAt)
        : NaN;
      entries.push({
        key: `${provider.providerId}:${measured.id}`,
        providerId: provider.providerId,
        displayName: provider.displayName,
        label: measured.label,
        window: measured.window,
        resetMs: Number.isNaN(parsed) ? null : parsed,
      });
    }
  }
  return entries;
}

/**
 * A window is "quiet" — not worth a timeline row — when nothing is happening on
 * it: no usage yet AND no imminent reset. This drops the 0%-used lanes that
 * would otherwise pad the list (e.g. an untouched Promotional/On-demand pool
 * resetting weeks out, or a lifted window that reports 0% with no reset). A
 * 0%-used window still shows if it resets within the next day, since that's a
 * heads-up worth keeping.
 */
function isQuiet(entry: TimelineEntry, nowMs: number): boolean {
  if (entry.window.usedPercent >= 1) return false;
  const resettingSoon =
    entry.resetMs !== null &&
    entry.resetMs - nowMs >= 0 &&
    entry.resetMs - nowMs <= DAY_MS;
  return !resettingSoon;
}

function bucketEntries(entries: TimelineEntry[], nowMs: number): Bucket[] {
  const visible = entries.filter((e) => !isQuiet(e, nowMs));
  // Only windows with a known future/near reset appear on the timeline; sort by
  // soonest. Entries without a parseable reset fall to the end under "No reset".
  const dated = visible
    .filter((e) => e.resetMs !== null)
    .sort((a, b) => (a.resetMs as number) - (b.resetMs as number));
  const undated = visible.filter((e) => e.resetMs === null);

  const readyNow: TimelineEntry[] = [];
  const today: TimelineEntry[] = [];
  const week: TimelineEntry[] = [];
  const later: TimelineEntry[] = [];
  for (const e of dated) {
    const delta = (e.resetMs as number) - nowMs;
    if (delta <= 0) readyNow.push(e);
    else if (delta < DAY_MS) today.push(e);
    else if (delta < 7 * DAY_MS) week.push(e);
    else later.push(e);
  }

  return [
    { id: "now", label: "Ready now", entries: readyNow },
    { id: "today", label: "Next 24 hours", entries: today },
    { id: "week", label: "This week", entries: week },
    { id: "later", label: "Later", entries: later },
    { id: "noreset", label: "No scheduled reset", entries: undated },
  ].filter((b) => b.entries.length > 0);
}

function TimelineRow({
  entry,
  nowMs,
}: {
  entry: TimelineEntry;
  nowMs: number;
}) {
  const level = levelOf(entry.window);
  const usedPct = Math.max(0, Math.min(100, entry.window.usedPercent));
  const when =
    entry.resetMs === null
      ? "—"
      : shortDuration((entry.resetMs as number) - nowMs);

  return (
    <li className="activity-row">
      <div className="activity-row__when">
        {entry.resetMs !== null && when !== "now" && (
          <span className="activity-row__when-prefix">in</span>
        )}
        <span className="activity-row__when-value">{when}</span>
      </div>
      <div className="activity-row__track" aria-hidden>
        <span className="activity-row__dot" data-level={level} />
      </div>
      <div className="activity-row__card">
        <div className="activity-row__head">
          <ProviderIcon
            providerId={entry.providerId}
            size={18}
            className="activity-row__icon"
            title={entry.displayName}
          />
          <span className="activity-row__name">{entry.displayName}</span>
          <span className="activity-row__label">{entry.label}</span>
          <span className="activity-row__pct">{Math.round(usedPct)}% used</span>
        </div>
        <div className="activity-row__bar">
          <div
            className="activity-row__bar-fill"
            data-level={level}
            style={{ width: `${usedPct}%` }}
          />
        </div>
      </div>
    </li>
  );
}

export default function ActivityTimeline({
  providers,
}: {
  providers: ProviderUsageSnapshot[];
}) {
  // Live clock so the "in Xh Ym" durations stay current between refreshes.
  const [nowMs, setNowMs] = useState(() => Date.now());
  useEffect(() => {
    const id = window.setInterval(() => setNowMs(Date.now()), 30_000);
    return () => window.clearInterval(id);
  }, []);

  const buckets = useMemo(
    () => bucketEntries(collectEntries(providers), nowMs),
    [providers, nowMs],
  );

  if (buckets.length === 0) {
    return (
      <div className="activity-empty">
        <strong>Nothing scheduled</strong>
        Reset times appear here once your providers report usage windows.
      </div>
    );
  }

  return (
    <div className="activity-timeline">
      {buckets.map((bucket) => (
        <Fragment key={bucket.id}>
          <div className="activity-group__label">{bucket.label}</div>
          <ul className="activity-group">
            {bucket.entries.map((entry) => (
              <TimelineRow key={entry.key} entry={entry} nowMs={nowMs} />
            ))}
          </ul>
        </Fragment>
      ))}
    </div>
  );
}
