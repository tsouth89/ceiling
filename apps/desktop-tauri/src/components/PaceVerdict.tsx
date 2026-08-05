import { useLocale } from "../i18n/LocaleProvider";
import { paceCategory } from "../surfaces/tray/paceCategory";
import type { PaceSnapshot } from "../types/bridge";

/**
 * The one-line answer to "am I going to run out before this resets?".
 *
 * The detailed expected-vs-actual bars are too tall for the tray, so they stay
 * hidden there. This carries the same conclusion in a third of the height: a
 * verdict, its consequence, and a single bar whose tick marks where usage
 * should be by now.
 */
export default function PaceVerdict({ pace }: { pace: PaceSnapshot }) {
  const { t } = useLocale();
  const category = paceCategory(pace.stage);
  const runningOut = !pace.willLastToReset && pace.etaSeconds != null;
  // Running out dominates: being "slow" is irrelevant if the meter still dies
  // before the window closes.
  const tone = runningOut && category !== "burning" ? "racing" : category;

  const actual = clampPercent(pace.actualUsedPercent);
  const expected = clampPercent(pace.expectedUsedPercent);
  // Derive from the clamped value, or a NaN off the bridge would render as
  // "NaN% to spare" beside a bar that had already clamped it away.
  const remaining = Math.round(100 - actual);
  const headline = runningOut
    ? t("PaceVerdictRunningOut")
    : t(
        category === "burning" || category === "racing"
          ? "PaceVerdictAhead"
          : category === "slow"
            ? "PaceVerdictPlenty"
            : "PaceVerdictOnTrack",
      );
  const detail = runningOut
    ? t("PaceVerdictRunsOutIn").replace(
        "{}",
        formatEta(pace.etaSeconds as number),
      )
    : t("PaceVerdictLastsToReset").replace("{}", String(remaining));

  return (
    <div className="menu-card__pace-verdict" data-pace={tone}>
      <div className="menu-card__pace-verdict-head">
        <span className="menu-card__pace-verdict-dot" data-pace={tone} />
        <span className="menu-card__pace-verdict-title" data-pace={tone}>
          {headline}
        </span>
      </div>
      <span className="menu-card__pace-verdict-detail">{detail}</span>
      <div
        className="menu-card__pace-verdict-track"
        role="img"
        aria-label={`${headline}. ${detail}`}
      >
        <div
          className="menu-card__pace-verdict-fill"
          data-pace={tone}
          style={{ width: `${actual.toFixed(1)}%` }}
        />
        <div
          className="menu-card__pace-verdict-tick"
          style={{ left: `${expected.toFixed(1)}%` }}
        />
      </div>
    </div>
  );
}

function clampPercent(value: number): number {
  if (!Number.isFinite(value)) return 0;
  return Math.min(100, Math.max(0, value));
}

/** Compact duration for the "runs out in X" line. */
export function formatEta(seconds: number): string {
  if (!Number.isFinite(seconds) || seconds <= 0) return "0m";
  const totalMinutes = Math.round(seconds / 60);
  const days = Math.floor(totalMinutes / (60 * 24));
  const hours = Math.floor((totalMinutes % (60 * 24)) / 60);
  const minutes = totalMinutes % 60;
  if (days > 0) return hours > 0 ? `${days}d ${hours}h` : `${days}d`;
  if (hours > 0) return minutes > 0 ? `${hours}h ${minutes}m` : `${hours}h`;
  return `${minutes}m`;
}
