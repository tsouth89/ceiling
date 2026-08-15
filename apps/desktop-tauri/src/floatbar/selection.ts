import type { FloatBarSelectionMode } from "../types/bridge";

export function selectVisibleFloatBarProviders<T extends { providerId: string }>(
  pinned: T[],
  allEligible: T[],
  options: {
    mode: FloatBarSelectionMode;
    detectionEnabled: boolean;
    lastActiveProviderId: string | null;
    usedPercent: (row: T) => number;
    highUsageThreshold: number;
  },
): T[] {
  const {
    mode,
    detectionEnabled,
    lastActiveProviderId,
    usedPercent,
    highUsageThreshold,
  } = options;
  if (mode === "pinned" || !detectionEnabled) {
    return pinned;
  }

  const byId = new Map(allEligible.map((row) => [row.providerId, row]));
  const active = lastActiveProviderId
    ? byId.get(lastActiveProviderId)
    : undefined;

  if (!active) {
    return pinned;
  }
  if (mode === "active") {
    return [active];
  }

  const next: T[] = [active];
  const seen = new Set<string>([active.providerId]);
  for (const row of pinned) {
    if (seen.has(row.providerId)) continue;
    if (usedPercent(row) + 1e-9 >= highUsageThreshold) {
      next.push(row);
      seen.add(row.providerId);
    }
  }
  return next.length > 0 ? next : pinned;
}
