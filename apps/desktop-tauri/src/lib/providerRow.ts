import type { ProviderUsageSnapshot } from "../types/bridge";

/**
 * Identity of one displayed row.
 *
 * A provider used to be one row, so `providerId` served as React key, Map key,
 * sort key and selection id everywhere. With several accounts configured the
 * same provider produces one reading per account, and keying on `providerId`
 * alone silently collapsed them: the second account replaced the first instead
 * of appearing beside it.
 *
 * While Ceiling follows whichever account the CLI is signed in as there is no
 * account id, so this is just the provider id and nothing changes.
 */
export type ProviderRowKey = string;

export function providerRowKey(
  provider: Pick<ProviderUsageSnapshot, "providerId" | "accountId">,
): ProviderRowKey {
  return provider.accountId
    ? `${provider.providerId}::${provider.accountId}`
    : provider.providerId;
}

/** The provider a row key belongs to, for provider-level lookups. */
export function providerIdFromRowKey(key: ProviderRowKey): string {
  const separator = key.indexOf("::");
  return separator === -1 ? key : key.slice(0, separator);
}

/** Whether a row key refers to `providerId`, regardless of account. */
export function rowKeyIsProvider(
  key: ProviderRowKey,
  providerId: string,
): boolean {
  return providerIdFromRowKey(key) === providerId;
}

/**
 * Whether more than one row exists for a provider, i.e. whether an account name
 * is needed to tell rows apart. With a single account the name is noise.
 */
export function hasMultipleAccounts(
  providers: Pick<ProviderUsageSnapshot, "providerId" | "accountId">[],
  providerId: string,
): boolean {
  return providers.filter((entry) => entry.providerId === providerId).length > 1;
}

/**
 * Collapse to one entry per provider, keeping the first.
 *
 * For surfaces that switch *providers* rather than list readings: the tray grid
 * shows one icon per provider, so two accounts must not produce two icons.
 * Selecting that provider then reveals every account beneath it.
 */
export function onePerProvider<
  T extends Pick<ProviderUsageSnapshot, "providerId">,
>(providers: T[]): T[] {
  const seen = new Set<string>();
  return providers.filter((entry) => {
    if (seen.has(entry.providerId)) return false;
    seen.add(entry.providerId);
    return true;
  });
}

/**
 * The account that best represents a provider on a provider-level summary.
 *
 * Some surfaces are about the provider rather than a reading of it: the
 * Settings providers list configures "Codex", not one of its accounts. Those
 * still need one snapshot to summarise, and building a Map keyed by provider
 * silently picked whichever account happened to be last.
 *
 * Picks the most-constrained account, matching how the tray already chooses
 * which window to surface: the seat about to run out is the one worth showing.
 * Ties resolve on account id so the choice is stable between refreshes rather
 * than flickering.
 */
export function representativeForProvider<
  T extends Pick<ProviderUsageSnapshot, "providerId" | "accountId"> & {
    // Only the used percentage matters here, so do not demand a whole
    // RateWindowSnapshot from callers that have less.
    primary?: { usedPercent: number } | null;
  },
>(providers: T[], providerId: string): T | null {
  const rows = providers.filter((entry) => entry.providerId === providerId);
  if (rows.length === 0) return null;

  return rows.reduce((best, candidate) => {
    const bestUsed = best.primary?.usedPercent ?? -1;
    const candidateUsed = candidate.primary?.usedPercent ?? -1;
    if (candidateUsed !== bestUsed) return candidateUsed > bestUsed ? candidate : best;
    return (candidate.accountId ?? "") < (best.accountId ?? "") ? candidate : best;
  });
}
