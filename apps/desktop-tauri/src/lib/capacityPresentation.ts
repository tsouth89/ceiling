import type {
  ProviderUsageSnapshot,
  RateWindowSnapshot,
} from "../types/bridge";

export type CapacityFreshness = "live" | "stale" | "error" | "lifted";

export type ConstrainingWindow = {
  id: string;
  label: string;
  window: RateWindowSnapshot;
};

export type GlanceMeters = {
  /** Account plan pool — always the overview hero. */
  primary: ConstrainingWindow;
  /** Hottest non-primary lane when materially hot; otherwise omitted. */
  companion: ConstrainingWindow | null;
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
 * Overview glance model: primary plan pool as hero, plus an optional hot
 * companion lane (Auto/API/5-hour/etc.). Clicking never toggles meters —
 * detail mode lists every window.
 */
export function glanceMeters(provider: ProviderUsageSnapshot): GlanceMeters {
  const primary: ConstrainingWindow = {
    id: "primary",
    label: provider.primaryLabel?.trim() || "Plan",
    window: provider.primary,
  };

  let companion: ConstrainingWindow | null = null;
  for (const candidate of nonPrimaryWindows(provider)) {
    if (!isCompanionHot(candidate.window, primary.window)) continue;
    if (
      !companion ||
      candidate.window.usedPercent > companion.window.usedPercent
    ) {
      companion = candidate;
    }
  }

  return { primary, companion };
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
    push(`extra-${extra.id}`, extra.title, extra.window, extra.title);
  }
  return out;
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

/** Strip/flyout freshness: error > stale > lifted > live. */
export function capacityFreshness(
  provider: ProviderUsageSnapshot,
  nowMs = Date.now(),
): CapacityFreshness {
  if (provider.error) return "error";
  const updated = Date.parse(provider.updatedAt);
  if (Number.isFinite(updated) && nowMs - updated > STALE_AFTER_MS) {
    return "stale";
  }
  if ((provider.inactiveRateWindows?.length ?? 0) > 0) {
    return "lifted";
  }
  return "live";
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
