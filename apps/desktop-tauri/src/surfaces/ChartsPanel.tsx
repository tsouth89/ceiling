import { useEffect, useMemo, useState } from "react";
import type { ProviderUsageSnapshot } from "../types/bridge";
import { useLocale } from "../hooks/useLocale";
import { ProviderIcon } from "../components/providers/ProviderIcon";
import { providerSupportsChartData } from "../lib/providerCharts";
import { ChartsSection } from "./settings/providers/sections/charts/ChartsSection";
import ProviderComparison from "./ProviderComparison";

const COMPARE_ID = "compare";

/**
 * Displays historical charts for supported providers.
 *
 * Provides provider selection and comparison for Codex and Claude when both
 * providers have chart data available.
 *
 * @param providers - Provider usage snapshots used to populate the charts
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
    const codex = supported.find((provider) => provider.providerId === "codex");
    const claude = supported.find((provider) => provider.providerId === "claude");
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
    return (
      <div className="charts-empty">
        <strong>No charts yet</strong>
        Limits and local usage history shows up here for providers that report it —
        Codex, Claude, and OpenAI.
      </div>
    );
  }

  const comparing = selectedId === COMPARE_ID && comparisonProviders !== null;
  const selected = supported.find((p) => p.providerId === selectedId) ?? supported[0];
  const tabCount = supported.length + (comparisonProviders ? 1 : 0);

  return (
    <div className="charts-panel">
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
            const isActive = !comparing && p.providerId === selected.providerId;
            return (
              <button
                key={p.providerId}
                type="button"
                role="tab"
                aria-selected={isActive}
                className="charts-provider-tab"
                data-active={isActive ? "true" : "false"}
                onClick={() => setSelectedId(p.providerId)}
              >
                <ProviderIcon
                  providerId={p.providerId}
                  size={16}
                  className="charts-provider-tab__icon"
                  title={p.displayName}
                />
                <span>{p.displayName}</span>
              </button>
            );
          })}
        </div>
      )}
      {comparing ? (
        <ProviderComparison providers={[comparisonProviders[0], comparisonProviders[1]]} />
      ) : (
        <ChartsSection
          key={selected.providerId}
          providerId={selected.providerId}
          accountEmail={selected.accountEmail}
          providerSnapshot={selected}
          t={t}
        />
      )}
    </div>
  );
}
