import { constrainingWindow } from "./capacityPresentation";
import { maskIdentity } from "./privacy";
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

/**
 * How hot an account reads on a one-tile-per-provider strip.
 *
 * The strip shows the *constraining* window, so ranking accounts by their
 * primary window disagreed with it: a Claude seat whose 5h session is fresh but
 * whose weekly is maxed ranks last on primary and first on the strip. That is
 * what made the flyout badge "On strip" the wrong account while the tile showed
 * the right one.
 *
 * Mirrors `select_strip_snapshot` in `taskbar_widget.rs`.
 */
function stripHeat(provider: Pick<ProviderUsageSnapshot, "primary">): number {
  if (!provider.primary) return -1;
  const constraining = constrainingWindow(provider as ProviderUsageSnapshot);
  // A placeholder 0% is not heat — same as a missing primary (SBS-876).
  if (constraining.namedState) return -1;
  return constraining.window.usedPercent;
}

/**
 * Account-id order, matching Rust's `String: Ord` so the strip tie-break lands
 * on the same seat as the native one.
 *
 * Deliberately not `localeCompare`: collation sorts "a" before "B" where a byte
 * comparison does the reverse, so a mixed-case id could tie-break one way in
 * the flyout and the other way on the tile.
 */
export function compareAccountIds(
  a: string | null | undefined,
  b: string | null | undefined,
): number {
  const left = a ?? "";
  const right = b ?? "";
  if (left === right) return 0;
  return left < right ? -1 : 1;
}

/**
 * Which account the compact taskbar/float strip should show for a provider.
 *
 * Matches the native strip: an explicit pin wins when that account is present,
 * otherwise the account closest to its constraining limit. Ties resolve on the
 * lowest account id so the choice is stable between refreshes rather than
 * flickering with fetch order.
 */
export function selectStripAccount<T extends ProviderUsageSnapshot>(
  candidates: T[],
  preferredAccountId?: string | null,
): T | undefined {
  if (candidates.length === 0) return undefined;
  const want = preferredAccountId?.trim();
  if (want) {
    const hit = candidates.find((row) => row.accountId === want);
    if (hit) return hit;
  }
  return [...candidates].sort((a, b) => {
    const delta = stripHeat(b) - stripHeat(a);
    if (delta !== 0) return delta;
    return compareAccountIds(a.accountId, b.accountId);
  })[0];
}

/**
 * Flyout list order: strip providers in strip order, and within each multi-
 * account provider put the strip account first so it matches the tile.
 */
export function orderFlyoutProviders<T extends ProviderUsageSnapshot>(
  providers: T[],
  stripProviderIds: string[],
  pinnedAccounts: Record<string, string>,
): T[] {
  const byProvider = new Map<string, T[]>();
  for (const row of providers) {
    const group = byProvider.get(row.providerId) ?? [];
    group.push(row);
    byProvider.set(row.providerId, group);
  }

  const providerOrder =
    stripProviderIds.length > 0
      ? stripProviderIds.filter((id) => byProvider.has(id))
      : [...byProvider.keys()];

  // Append any remaining providers not in the strip list (should be rare after
  // the flyout's own filter, but keeps the helper total).
  for (const id of byProvider.keys()) {
    if (!providerOrder.includes(id)) providerOrder.push(id);
  }

  const result: T[] = [];
  for (const providerId of providerOrder) {
    const group = byProvider.get(providerId) ?? [];
    const strip = selectStripAccount(group, pinnedAccounts[providerId]);
    if (!strip) continue;
    result.push(strip);
    const rest = group
      .filter((row) => providerRowKey(row) !== providerRowKey(strip))
      .sort((a, b) => {
        const delta = stripHeat(b) - stripHeat(a);
        if (delta !== 0) return delta;
        return compareAccountIds(a.accountId, b.accountId);
      });
    result.push(...rest);
  }
  return result;
}

/**
 * How an account is named on a usage card: its email, plus plan when known.
 *
 * Deliberately the email, not `accountLabel`. The label is what the user typed
 * when adding the account ("Work"), or an auto-derived "email (plan)" when they
 * did not — so two accounts could show two different *kinds* of thing, one an
 * email and one a nickname. The email is the real identity and is consistent.
 * The custom label still names the account on the Accounts page.
 */
export function accountIdentityLabel(
  provider: Pick<
    ProviderUsageSnapshot,
    "accountEmail" | "accountLabel" | "planName"
  >,
  hideEmail = false,
): string | null {
  const identity = provider.accountEmail ?? provider.accountLabel ?? null;
  if (!identity) return null;
  const labelled = provider.planName
    ? `${identity} (${provider.planName})`
    : identity;
  // Masked here rather than at each call site. Every surface that shows an
  // account went through this function, and every one of them printed the raw
  // address while "Hide Personal Info" was on; making the mask opt-out at the
  // source is what stops the next surface from reintroducing the leak. Note it
  // masks the whole string, because `accountLabel` falls back to an
  // auto-derived "email (plan)" and is a second copy of the address.
  return maskIdentity(labelled, hideEmail);
}
