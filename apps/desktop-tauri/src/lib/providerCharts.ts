import type {
  LocalUsageWindowRequest,
  ProviderUsageSnapshot,
  RateWindowSnapshot,
} from "../types/bridge";

const PROVIDER_CHART_DATA_IDS = new Set(["claude", "codex", "cursor", "openai"]);

export function providerSupportsChartData(providerId: string): boolean {
  return PROVIDER_CHART_DATA_IDS.has(providerId.toLowerCase());
}

function currentWindowLabel(label: string): string {
  const normalized = label.toLowerCase();
  if (normalized.includes("5h") || normalized.includes("5-hour")) {
    return "Current 5h window";
  }
  if (normalized.includes("week")) return "Current weekly window";
  return `Current ${label.toLowerCase()} window`;
}

function usageWindowRequest(
  id: string,
  label: string,
  window: RateWindowSnapshot | null | undefined,
): LocalUsageWindowRequest | null {
  if (!window?.resetsAt || !window.windowMinutes || window.windowMinutes <= 0) {
    return null;
  }
  const endsAtMs = Date.parse(window.resetsAt);
  if (!Number.isFinite(endsAtMs)) return null;
  return {
    id,
    label: currentWindowLabel(label),
    startsAt: new Date(endsAtMs - window.windowMinutes * 60_000).toISOString(),
    endsAt: new Date(endsAtMs).toISOString(),
  };
}

/** Exact local-log ranges corresponding to the provider's live reset windows. */
export function providerLocalUsageWindows(
  provider: ProviderUsageSnapshot | null | undefined,
): LocalUsageWindowRequest[] {
  if (!provider || !["codex", "claude"].includes(provider.providerId.toLowerCase())) {
    return [];
  }
  return [
    usageWindowRequest(
      "primary",
      provider.primaryLabel?.trim() || "plan",
      provider.primary,
    ),
    usageWindowRequest(
      "secondary",
      provider.secondaryLabel?.trim() || "weekly",
      provider.secondary,
    ),
  ].filter((window): window is LocalUsageWindowRequest => window !== null);
}
