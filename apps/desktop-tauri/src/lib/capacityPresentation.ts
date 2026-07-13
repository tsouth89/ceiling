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

export type ActivePromoBoost = {
  id: string;
  title: string;
  description: string;
};

const STALE_AFTER_MS = 10 * 60 * 1000;

/** Pick the measured window with the highest used percent (constraining). */
export function constrainingWindow(
  provider: ProviderUsageSnapshot,
): ConstrainingWindow {
  let best: ConstrainingWindow = {
    id: "primary",
    label: provider.primaryLabel?.trim() || "Plan",
    window: provider.primary,
  };

  const consider = (
    id: string,
    label: string | null | undefined,
    window: RateWindowSnapshot | null | undefined,
    fallback: string,
  ) => {
    if (!window) return;
    if (window.usedPercent > best.window.usedPercent) {
      best = {
        id,
        label: label?.trim() || fallback,
        window,
      };
    }
  };

  consider("secondary", provider.secondaryLabel, provider.secondary, "Weekly");
  consider("model", null, provider.modelSpecific, "Model");
  consider("tertiary", null, provider.tertiary, "Extra");
  for (const extra of provider.extraRateWindows ?? []) {
    consider(`extra-${extra.id}`, extra.title, extra.window, extra.title);
  }

  return best;
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
