import type { ProviderCatalogEntry, ProviderUsageSnapshot } from "../types/bridge";

export function orderProviderSnapshots(
  providers: ProviderUsageSnapshot[],
  catalog: ProviderCatalogEntry[],
  enabledProviderIds: string[],
  providerOrder: string[] = [],
): ProviderUsageSnapshot[] {
  const enabled = new Set(enabledProviderIds);
  const order = new Map<string, number>();
  const orderedIds = providerOrder.length > 0
    ? providerOrder
    : catalog.map((provider) => provider.id);
  for (const [index, providerId] of orderedIds.entries()) {
    order.set(providerId, index);
  }
  for (const [index, providerId] of enabledProviderIds.entries()) {
    if (!order.has(providerId)) {
      order.set(providerId, orderedIds.length + index);
    }
  }

  return providers.filter((provider) => enabled.has(provider.providerId)).sort((a, b) => {
    const aOrder = order.get(a.providerId);
    const bOrder = order.get(b.providerId);
    if (aOrder != null && bOrder != null && aOrder !== bOrder) return aOrder - bOrder;
    // Both ordered and equal means two accounts of one provider. Falling through
    // to the `aOrder != null` branch returned -1 for compare(a,b) *and*
    // compare(b,a), which is not a valid comparator: sort output became
    // implementation-defined and the two rows swapped between refreshes as
    // fetches finished in different orders.
    if (aOrder != null && bOrder != null) {
      return (a.accountId ?? "").localeCompare(b.accountId ?? "");
    }
    if (aOrder != null) return -1;
    if (bOrder != null) return 1;
    const byName = a.displayName.localeCompare(b.displayName);
    return byName !== 0
      ? byName
      : (a.accountId ?? "").localeCompare(b.accountId ?? "");
  });
}
