import type {
  PaceSnapshot,
  ProviderUsageSnapshot,
  RateWindowSnapshot,
} from "../types/bridge";

export type CapacityFreshness = "live" | "stale" | "error";

export type ConstrainingWindow = {
  id: string;
  label: string;
  window: RateWindowSnapshot;
};

export type GlanceMeters = {
  /** Account plan pool — always the overview hero. */
  primary: ConstrainingWindow;
  /** Compact non-primary lanes shown beneath the hero. */
  companions: ConstrainingWindow[];
};

export type ProviderGlanceStatus = "ok" | "warning" | "exhausted" | "error";

export type ActivePromoBoost = {
  id: string;
  title: string;
  description: string;
};

const STALE_AFTER_MS = 10 * 60 * 1000;
/** Companion lanes appear on overview when used reaches this share. */
export const GLANCE_COMPANION_HOT_PERCENT = 70;

/** Pick the measured window with the highest used percent (constraining). */
export function constrainingWindow(
  provider: ProviderUsageSnapshot,
): ConstrainingWindow {
  let best: ConstrainingWindow = {
    id: "primary",
    label: provider.primaryLabel?.trim() || "Plan",
    window: provider.primary,
  };

  for (const candidate of nonPrimaryWindows(provider)) {
    if (candidate.window.usedPercent > best.window.usedPercent) {
      best = candidate;
    }
  }

  return best;
}

/**
 * Overview glance model: primary plan pool as hero, plus compact companion
 * lanes. Cursor always shows its reported Auto and API lanes because they are
 * distinct allowances users need to compare. Claude always shows Weekly beside
 * its 5-hour session because both limits define the subscription. Other
 * providers keep the single hottest materially constrained lane. Clicking
 * never toggles meters — detail mode lists every window.
 */
export function glanceMeters(provider: ProviderUsageSnapshot): GlanceMeters {
  const primary: ConstrainingWindow = {
    id: "primary",
    label: provider.primaryLabel?.trim() || "Plan",
    window: provider.primary,
  };

  const candidates = nonPrimaryWindows(provider);
  if (provider.providerId === "cursor") {
    const cursorCompanions = [
      candidates.find((candidate) => candidate.id === "secondary"),
      candidates.find((candidate) => candidate.id === "extra-cursor-api"),
    ].filter((candidate): candidate is ConstrainingWindow => Boolean(candidate));
    return { primary, companions: cursorCompanions };
  }

  if (provider.providerId === "claude") {
    const weekly = candidates.find((candidate) => candidate.id === "secondary");
    return { primary, companions: weekly ? [weekly] : [] };
  }

  let companion: ConstrainingWindow | null = null;
  for (const candidate of candidates) {
    if (!isCompanionHot(candidate.window, primary.window)) continue;
    if (
      !companion ||
      candidate.window.usedPercent > companion.window.usedPercent
    ) {
      companion = candidate;
    }
  }

  return { primary, companions: companion ? [companion] : [] };
}

function isCompanionHot(
  companion: RateWindowSnapshot,
  primary: RateWindowSnapshot,
): boolean {
  return (
    companion.usedPercent >= GLANCE_COMPANION_HOT_PERCENT ||
    companion.usedPercent >= primary.usedPercent
  );
}

function nonPrimaryWindows(
  provider: ProviderUsageSnapshot,
): ConstrainingWindow[] {
  const out: ConstrainingWindow[] = [];
  const push = (
    id: string,
    label: string | null | undefined,
    window: RateWindowSnapshot | null | undefined,
    fallback: string,
  ) => {
    if (!window) return;
    out.push({
      id,
      label: label?.trim() || fallback,
      window,
    });
  };

  push("secondary", provider.secondaryLabel, provider.secondary, "Weekly");
  push("model", null, provider.modelSpecific, "Model");
  push("tertiary", null, provider.tertiary, "Extra");
  for (const extra of provider.extraRateWindows ?? []) {
    if (extra.id === "reset-credits") continue;
    push(`extra-${extra.id}`, extra.title, extra.window, extra.title);
  }
  return out;
}

/**
 * Provider-reported banked resets, including legacy cached snapshots. Returns
 * the observed count — `0` is a real reading and stays distinct from `null`,
 * which means the provider does not report resets (or the fetch is unknown).
 */
export function resetCreditsAvailable(
  provider: ProviderUsageSnapshot,
): number | null {
  if (
    typeof provider.resetCreditsAvailable === "number" &&
    provider.resetCreditsAvailable >= 0
  ) {
    return Math.floor(provider.resetCreditsAvailable);
  }
  const legacy = provider.extraRateWindows?.find(
    (window) => window.id === "reset-credits",
  );
  const match = legacy?.window.resetDescription?.match(/\d+/);
  return match ? Number(match[0]) : null;
}

/**
 * Codex banked resets for the persistent indicator. Codex is the only provider
 * with this concept, so it is shown Codex-only and always (including the `0`
 * state) whenever we have a trustworthy reading, keeping the feature visible
 * and building trust that Ceiling is tracking it.
 */
export function codexResetCredits(
  provider: ProviderUsageSnapshot,
): number | null {
  if (provider.providerId !== "codex") return null;
  return resetCreditsAvailable(provider);
}

/**
 * Every measured rate window for a provider (plan pool + session/weekly/model/
 * extra lanes), each carrying its display label. Powers the Activity timeline,
 * which enumerates all reset windows across providers rather than just the
 * glance hero + companion.
 */
export function allMeasuredWindows(
  provider: ProviderUsageSnapshot,
): ConstrainingWindow[] {
  const primary: ConstrainingWindow = {
    id: "primary",
    label: provider.primaryLabel?.trim() || "Plan",
    window: provider.primary,
  };
  return [primary, ...nonPrimaryWindows(provider)];
}

/** Grid / glance status chip from constraining pressure. */
export function providerGlanceStatus(
  provider: ProviderUsageSnapshot,
): ProviderGlanceStatus {
  if (provider.error) return "error";
  const constraining = constrainingWindow(provider);
  if (
    constraining.window.isExhausted ||
    constraining.window.usedPercent >= 100
  ) {
    return "exhausted";
  }
  if (constraining.window.usedPercent > 80) return "warning";
  return "ok";
}

/**
 * Strip/flyout freshness from the provider timestamp and error only:
 * error > stale > live. Inactive/lifted windows are surfaced as their own
 * `inactiveRateWindows` rows, never as provider-level freshness (SOU-152).
 */
export function capacityFreshness(
  provider: ProviderUsageSnapshot,
  nowMs = Date.now(),
): CapacityFreshness {
  if (provider.error) return "error";
  const updated = Date.parse(provider.updatedAt);
  if (Number.isFinite(updated) && nowMs - updated > STALE_AFTER_MS) {
    return "stale";
  }
  return "live";
}

export type CalmPaceTone = "steady" | "watch";
export type CalmPaceState = { label: string; tone: CalmPaceTone };
export type CalmPresentation = {
  /** Trustworthy pace state, or null when pace isn't fresh + supported. */
  pace: CalmPaceState | null;
  /** The window whose reset + label headline the calm pill. */
  window: ConstrainingWindow;
  /** Whether that window exposes a reset (a time or a description). */
  hasReset: boolean;
  /** No pace and no reset — callers fall back to the exact percentage. */
  showExactFallback: boolean;
};

/**
 * Compact human duration for the pace pill: "under 1m", "42m", "1h 20m",
 * "2d 3h". Kept short so it fits the float-bar pill.
 */
export function formatShortDuration(seconds: number): string {
  const total = Math.max(0, Math.round(seconds));
  if (total < 60) return "under 1m";
  const minutes = Math.round(total / 60);
  if (minutes < 60) return `${minutes}m`;
  const hours = Math.floor(minutes / 60);
  const restMinutes = minutes % 60;
  if (hours < 24) return restMinutes > 0 ? `${hours}h ${restMinutes}m` : `${hours}h`;
  const days = Math.floor(hours / 24);
  const restHours = hours % 24;
  return restHours > 0 ? `${days}d ${restHours}h` : `${days}d`;
}

/**
 * Trustworthy pace state for Calm mode, or null when there isn't enough signal.
 * Never invents an "on pace" state (SOU-178): it only speaks when the provider
 * actually reports usable pace data. When the current pace will NOT last to the
 * reset, surface the concrete time left ("~42m left") instead of a vague
 * "Running low", since the number is already computed (SOU-274). "On pace"
 * already carries the reset-aware meaning: the pace lasts until the window
 * resets.
 */
function calmPaceState(pace: PaceSnapshot | null): CalmPaceState | null {
  if (!pace) return null;
  // Data-backed and reassuring: the current pace lasts to the reset.
  if (pace.willLastToReset) return { label: "On pace", tone: "steady" };
  // Only warn when there is a real, finite estimate of running out; otherwise
  // stay silent rather than fabricate a state. Show the concrete ETA.
  if (
    typeof pace.etaSeconds === "number" &&
    Number.isFinite(pace.etaSeconds) &&
    pace.etaSeconds > 0
  ) {
    return { label: `~${formatShortDuration(pace.etaSeconds)} left`, tone: "watch" };
  }
  return null;
}

/**
 * What Calm mode surfaces for a provider: a trustworthy pace state plus the
 * next reset of the displayed window, with exact percentages left to expand.
 * Pace is claimed only when the snapshot is live and the provider reports
 * usable pace data. When pace is unavailable we fall back to the next reset;
 * when that is missing too, callers show the exact percentage instead.
 */
export function calmPresentation(
  provider: ProviderUsageSnapshot,
  window: ConstrainingWindow,
  nowMs: number = Date.now(),
): CalmPresentation {
  const fresh = capacityFreshness(provider, nowMs) === "live";
  const pace = fresh && !provider.error ? calmPaceState(provider.pace) : null;
  const hasReset = Boolean(
    window.window.resetsAt || window.window.resetDescription,
  );
  return { pace, window, hasReset, showExactFallback: !pace && !hasReset };
}

/** Boost promos that affect glance surfaces (strip / overview hero). */
export function activePromoBoosts(
  provider: ProviderUsageSnapshot,
): ActivePromoBoost[] {
  return (provider.promoSignals ?? [])
    .filter((signal) => signal.kind === "boost")
    .map((signal) => ({
      id: signal.id,
      title: signal.title,
      description: signal.description,
    }));
}

/** Quieter inclusion notes for detail / overview meta only. */
export function activePromoInclusions(
  provider: ProviderUsageSnapshot,
): ActivePromoBoost[] {
  return (provider.promoSignals ?? [])
    .filter((signal) => signal.kind === "inclusion")
    .map((signal) => ({
      id: signal.id,
      title: signal.title,
      description: signal.description,
    }));
}
