import type {
  LocalUsageWindowRequest,
  ProviderUsageSnapshot,
  RateWindowSnapshot,
} from "../types/bridge";

const PROVIDER_CHART_DATA_IDS = new Set(["claude", "codex", "cursor", "openai"]);
const RESET_BOUNDARY_PRECISION_MS = 1_000;

export function providerSupportsChartData(providerId: string): boolean {
  return PROVIDER_CHART_DATA_IDS.has(providerId.toLowerCase());
}

/**
 * Creates a standardized display label for a usage window.
 *
 * @param label - The source label used to identify the window duration.
 * @returns A display label using standardized wording for five-hour and weekly windows.
 */
function currentWindowLabel(label: string): string {
  const normalized = label.toLowerCase();
  if (normalized.includes("5h") || normalized.includes("5-hour")) {
    return "Current 5h window";
  }
  if (normalized.includes("week")) return "Current weekly window";
  return `Current ${label.toLowerCase()} window`;
}

/**
 * Builds a local usage window request from a provider rate-window snapshot.
 *
 * @param id - Identifier for the usage window
 * @param label - Human-readable label for the usage window
 * @param window - Provider snapshot containing the reset time and window duration
 * @returns A usage window request with normalized ISO boundaries, or `null` if the snapshot is invalid
 */
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
  // Provider countdowns are sampled at slightly different instants on every
  // refresh. Claude in particular can report the same 5-hour boundary a few
  // hundred milliseconds either side of the exact second. Treating those as
  // distinct ranges creates a new chart cache entry and restarts an expensive
  // transcript scan every refresh. Snap to the nearest second while keeping
  // the real provider reset minute intact.
  const stableEndsAtMs = Math.round(endsAtMs / RESET_BOUNDARY_PRECISION_MS)
    * RESET_BOUNDARY_PRECISION_MS;
  return {
    id,
    label: currentWindowLabel(label),
    startsAt: new Date(stableEndsAtMs - window.windowMinutes * 60_000).toISOString(),
    endsAt: new Date(stableEndsAtMs).toISOString(),
  };
}

/**
 * Builds local log ranges for the supported provider's live reset windows.
 *
 * @param provider - The provider snapshot whose primary and secondary windows are converted.
 * @returns The valid local log window requests, or an empty array when the provider is unsupported or unavailable.
 */
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
