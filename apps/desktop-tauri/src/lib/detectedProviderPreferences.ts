const IGNORED_PROVIDER_IDS_KEY = "ceiling.detected-accounts.ignored-providers.v1";

export function getIgnoredDetectedProviderIds(): Set<string> {
  try {
    const stored = JSON.parse(window.localStorage.getItem(IGNORED_PROVIDER_IDS_KEY) ?? "[]");
    if (!Array.isArray(stored)) return new Set();
    return new Set(stored.filter((value): value is string => typeof value === "string"));
  } catch {
    return new Set();
  }
}

export function setDetectedProviderIgnored(providerId: string, ignored: boolean): void {
  try {
    const providerIds = getIgnoredDetectedProviderIds();
    if (ignored) providerIds.add(providerId);
    else providerIds.delete(providerId);
    window.localStorage.setItem(IGNORED_PROVIDER_IDS_KEY, JSON.stringify([...providerIds]));
  } catch {
    // Tracking still toggles if WebView storage is unavailable. The discovery
    // suggestion may return after restart in that uncommon case.
  }
}
