import { useEffect, useMemo, useState } from "react";
import type { ProviderUsageSnapshot } from "../types/bridge";
import { useLocale } from "../hooks/useLocale";
import { ProviderIcon } from "../components/providers/ProviderIcon";
import { providerSupportsChartData } from "../lib/providerCharts";
import { ChartsSection } from "./settings/providers/sections/charts/ChartsSection";
import ProviderComparison from "./ProviderComparison";
import { TotalApiValueCard } from "../components/TotalApiValueCard";
import { providerRowKey, representativeForProvider } from "../lib/providerRow";

const COMPARE_ID = "compare";

/**
 * Charts tab: quota, local token, credits, and usage history per provider.
 *
 * Only a few providers report historical chart data (Codex, Claude, OpenAI),
 * so this shows a provider selector across the supported ones and reuses the
 * existing, tested ChartsSection (which owns the limits/credits/usage sub-tabs)
 * for the selected provider. Unlike the Activity timeline — built from the live
 * snapshot — this is genuine time-series history from the backend.
 */
export default function ChartsPanel({
  providers,
}: {
  providers: ProviderUsageSnapshot[];
}) {
  const { t } = useLocale();

  const supported = useMemo(
    () =>
      providers.filter(
        (p) => providerSupportsChartData(p.providerId) && !p.error,
      ),
    [providers],
  );

  const [selectedId, setSelectedId] = useState<string | null>(null);
  const comparisonProviders = useMemo(() => {
    // Compare is provider-versus-provider, so it needs one reading each. Taking
    // the first was arbitrary once a provider could have two accounts.
    const codex = representativeForProvider(supported, "codex");
    const claude = representativeForProvider(supported, "claude");
    return codex && claude ? [codex, claude] as const : null;
  }, [supported]);

  // Keep the selection valid as the provider set changes. Comparison is the
  // useful starting point whenever both local-log providers are available.
  useEffect(() => {
    if (supported.length === 0) {
      setSelectedId(null);
      return;
    }
    setSelectedId((prev) =>
      prev && (supported.some((p) => p.providerId === prev) || (prev === COMPARE_ID && comparisonProviders))
        ? prev
        : comparisonProviders ? COMPARE_ID : supported[0].providerId,
    );
  }, [supported, comparisonProviders]);

  if (supported.length === 0) {
    // The API-value card loads its own local totals, so keep it visible even
    // when no provider reports chart-series data (or a snapshot errored).
    return (
      <div className="charts-panel">
        <TotalApiValueCard />
        <div className="charts-empty">
          <strong>No charts yet</strong>
          Limits and local usage history shows up here for providers that report it —
          Codex, Claude, and OpenAI.
        </div>
      </div>
    );
  }

  const comparing = selectedId === COMPARE_ID && comparisonProviders !== null;
  const selected =
    supported.find((p) => providerRowKey(p) === selectedId) ?? supported[0];
  const tabCount = supported.length + (comparisonProviders ? 1 : 0);

  return (
    <div className="charts-panel">
      <TotalApiValueCard />
      {tabCount > 1 && (
        <div className="charts-provider-tabs" role="tablist" aria-label="Provider">
          {comparisonProviders && (
            <button
              type="button"
              role="tab"
              aria-selected={comparing}
              className="charts-provider-tab charts-provider-tab--compare"
              data-active={comparing ? "true" : "false"}
              onClick={() => setSelectedId(COMPARE_ID)}
            >
              <span className="charts-provider-tab__compare-mark" aria-hidden>↔</span>
              <span>Compare</span>
            </button>
          )}
          {supported.map((p) => {
            const isActive =
              !comparing && providerRowKey(p) === providerRowKey(selected);
            return (
              <button
                key={providerRowKey(p)}
                type="button"
                role="tab"
                aria-selected={isActive}
                className="charts-provider-tab"
                data-active={isActive ? "true" : "false"}
                onClick={() => setSelectedId(providerRowKey(p))}
              >
                <ProviderIcon
                  providerId={p.providerId}
                  size={16}
                  className="charts-provider-tab__icon"
                  title={p.displayName}
                />
                <span>{p.accountLabel ?? p.displayName}</span>
              </button>
            );
          })}
        </div>
      )}
      {comparing ? (
        <ProviderComparison providers={[comparisonProviders[0], comparisonProviders[1]]} />
      ) : (
        <ChartsSection
          key={providerRowKey(selected)}
          providerId={selected.providerId}
          accountEmail={selected.accountEmail}
          providerSnapshot={selected}
          t={t}
        />
      )}
    </div>
  );
}
