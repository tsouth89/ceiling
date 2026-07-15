import { Fragment, useEffect, useMemo, useState } from "react";
import type { ProviderUsageSnapshot, RateWindowSnapshot } from "../types/bridge";
import { ProviderIcon } from "../components/providers/ProviderIcon";
import { allMeasuredWindows } from "../lib/capacityPresentation";

/**
 * Activity is a calm schedule of the rate-window resets providers report.
 * The soonest future reset is promoted into a useful glance, while the rest
 * stay in a compact chronological list. No history is implied or fabricated.
 */

type TimelineEntry = {
  key: string;
  providerId: string;
  displayName: string;
  label: string;
  window: RateWindowSnapshot;
  resetMs: number | null;
};

type Bucket = {
  id: string;
  label: string;
  entries: TimelineEntry[];
};

const DAY_MS = 24 * 60 * 60 * 1000;

/**
 * Classifies a rate window for usage bar styling.
 *
 * @param window - The rate window to classify
 * @returns `"exhausted"` for exhausted windows, `"critical"` for windows with 5% or less remaining, or `"normal"` otherwise
 */
function levelOf(window: RateWindowSnapshot): string {
  if (window.isExhausted) return "exhausted";
  if (window.remainingPercent <= 5) return "critical";
  return "normal";
}

/**
 * Formats a duration as a compact human-readable string.
 *
 * @param ms - The duration in milliseconds
 * @returns `"now"` for non-positive durations; otherwise, the duration in days and hours, hours and minutes, or minutes
 */
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

/**
 * Determines whether two dates fall on the same local calendar day.
 *
 * @param a - The first date to compare
 * @param b - The second date to compare
 * @returns `true` if both dates have the same local year, month, and day, `false` otherwise.
 */
function sameLocalDay(a: Date, b: Date): boolean {
  return (
    a.getFullYear() === b.getFullYear() &&
    a.getMonth() === b.getMonth() &&
    a.getDate() === b.getDate()
  );
}

/**
 * Formats a reset timestamp as a localized relative day and time label.
 *
 * @param resetMs - The reset timestamp in milliseconds.
 * @param nowMs - The reference timestamp in milliseconds.
 * @returns A localized label such as “Today at 3:00 PM,” “Tomorrow at 3:00 PM,” or a weekday and time.
 */
function localResetLabel(resetMs: number, nowMs: number): string {
  const reset = new Date(resetMs);
  const now = new Date(nowMs);
  const tomorrow = new Date(now);
  tomorrow.setDate(tomorrow.getDate() + 1);
  const time = reset.toLocaleTimeString(undefined, {
    hour: "numeric",
    minute: "2-digit",
  });
  if (sameLocalDay(reset, now)) return `Today at ${time}`;
  if (sameLocalDay(reset, tomorrow)) return `Tomorrow at ${time}`;
  const weekday = reset.toLocaleDateString(undefined, { weekday: "long" });
  return `${weekday} at ${time}`;
}

/**
 * Creates timeline entries for the measured rate windows reported by providers.
 *
 * Providers with errors are excluded. Invalid or missing reset timestamps are represented by `null`.
 *
 * @param providers - Provider usage snapshots containing measured rate windows
 * @returns The collected timeline entries
 */
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
 * Determines whether a timeline entry should be filtered out as quiet.
 *
 * @param entry - The timeline entry to evaluate
 * @param nowMs - The current time in milliseconds
 * @returns `true` if the entry has less than 1% usage and no reset within the next 24 hours, `false` otherwise
 */
function isQuiet(entry: TimelineEntry, nowMs: number): boolean {
  if (entry.window.usedPercent >= 1) return false;
  const resettingSoon =
    entry.resetMs !== null &&
    entry.resetMs - nowMs >= 0 &&
    entry.resetMs - nowMs <= DAY_MS;
  return !resettingSoon;
}

/**
 * Orders timeline entries by reset time, placing entries without a reset time last.
 *
 * @param entries - The timeline entries to order
 * @returns A new array containing the entries sorted by ascending reset time
 */
function sortEntries(entries: TimelineEntry[]): TimelineEntry[] {
  return [...entries].sort((a, b) => {
    if (a.resetMs === null) return b.resetMs === null ? 0 : 1;
    if (b.resetMs === null) return -1;
    return a.resetMs - b.resetMs;
  });
}

/**
 * Groups timeline entries by reset timing and omits the featured entry.
 *
 * @param entries - The timeline entries to group
 * @param nowMs - The current time in milliseconds
 * @param featuredKey - The key of the entry to exclude from the buckets
 * @returns The non-empty time-based buckets
 */
function bucketEntries(
  entries: TimelineEntry[],
  nowMs: number,
  featuredKey: string | null,
): Bucket[] {
  const readyNow: TimelineEntry[] = [];
  const today: TimelineEntry[] = [];
  const week: TimelineEntry[] = [];
  const later: TimelineEntry[] = [];
  const undated: TimelineEntry[] = [];

  for (const entry of entries) {
    if (entry.key === featuredKey) continue;
    if (entry.resetMs === null) {
      undated.push(entry);
      continue;
    }
    const delta = entry.resetMs - nowMs;
    if (delta <= 0) readyNow.push(entry);
    else if (delta < DAY_MS) today.push(entry);
    else if (delta < 7 * DAY_MS) week.push(entry);
    else later.push(entry);
  }

  return [
    { id: "now", label: "Ready now", entries: readyNow },
    { id: "today", label: "Next 24 hours", entries: today },
    { id: "week", label: "This week", entries: week },
    { id: "later", label: "Later", entries: later },
    { id: "noreset", label: "No scheduled reset", entries: undated },
  ].filter((bucket) => bucket.entries.length > 0);
}

/**
 * Renders a usage bar for a rate window.
 *
 * @param window - The rate window whose usage and status determine the bar appearance
 */
function UsageBar({ window }: { window: RateWindowSnapshot }) {
  const usedPct = Math.max(0, Math.min(100, window.usedPercent));
  return (
    <div className="activity-row__bar" aria-hidden>
      <div
        className="activity-row__bar-fill"
        data-level={levelOf(window)}
        style={{ width: `${usedPct}%` }}
      />
    </div>
  );
}

/**
 * Renders the upcoming reset with its provider, timing, and usage information.
 *
 * @param entry - The timeline entry representing the upcoming reset
 * @param nowMs - The current time in milliseconds
 */
function FeaturedReset({ entry, nowMs }: { entry: TimelineEntry; nowMs: number }) {
  const usedPct = Math.max(0, Math.min(100, entry.window.usedPercent));
  const duration = shortDuration((entry.resetMs as number) - nowMs);
  return (
    <section className="activity-next" data-activity-entry={entry.key}>
      <div className="activity-next__eyebrow">Next reset</div>
      <div className="activity-next__provider">
        <ProviderIcon
          providerId={entry.providerId}
          size={20}
          className="activity-next__icon"
          title={entry.displayName}
        />
        <span>{entry.displayName}</span>
        <span className="activity-next__window">{entry.label}</span>
      </div>
      <div className="activity-next__glance">
        <strong>{duration}</strong>
        <span>{localResetLabel(entry.resetMs as number, nowMs)}</span>
        <span className="activity-next__pct">{Math.round(usedPct)}% used</span>
      </div>
      <UsageBar window={entry.window} />
    </section>
  );
}

function TimelineRow({ entry, nowMs }: { entry: TimelineEntry; nowMs: number }) {
  const usedPct = Math.max(0, Math.min(100, entry.window.usedPercent));
  const when =
    entry.resetMs === null ? "—" : shortDuration(entry.resetMs - nowMs);

  return (
    <li className="activity-row" data-activity-entry={entry.key}>
      <div className="activity-row__when">{when}</div>
      <div className="activity-row__content">
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
        <UsageBar window={entry.window} />
      </div>
    </li>
  );
}

/**
 * Displays provider rate-window reset activity in a featured section and time-based groups.
 *
 * @param providers - Provider usage snapshots used to build the timeline.
 * @returns The activity timeline, or an empty-state message when no entries are scheduled.
 */
export default function ActivityTimeline({
  providers,
}: {
  providers: ProviderUsageSnapshot[];
}) {
  const [nowMs, setNowMs] = useState(() => Date.now());
  useEffect(() => {
    const id = window.setInterval(() => setNowMs(Date.now()), 30_000);
    return () => window.clearInterval(id);
  }, []);

  const entries = useMemo(
    () =>
      sortEntries(collectEntries(providers).filter((entry) => !isQuiet(entry, nowMs))),
    [providers, nowMs],
  );
  const featured =
    entries.find((entry) => entry.resetMs !== null && entry.resetMs > nowMs) ?? null;
  const buckets = useMemo(
    () => bucketEntries(entries, nowMs, featured?.key ?? null),
    [entries, featured?.key, nowMs],
  );

  if (!featured && buckets.length === 0) {
    return (
      <div className="activity-empty">
        <strong>Nothing scheduled</strong>
        Reset times appear here once your providers report usage windows.
      </div>
    );
  }

  return (
    <div className="activity-timeline">
      {featured && <FeaturedReset entry={featured} nowMs={nowMs} />}
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
