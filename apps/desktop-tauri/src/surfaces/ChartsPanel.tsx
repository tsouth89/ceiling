import { useEffect, useMemo, useState } from "react";
import type { ProviderUsageSnapshot } from "../types/bridge";
import { useLocale } from "../hooks/useLocale";
import { ProviderIcon } from "../components/providers/ProviderIcon";
import { providerSupportsChartData } from "../lib/providerCharts";
import { ChartsSection } from "./settings/providers/sections/charts/ChartsSection";

/**
 * Charts tab: cost / credits / usage-breakdown history per provider.
 *
 * Only a few providers report historical chart data (Codex, Claude, OpenAI),
 * so this shows a provider selector across the supported ones and reuses the
 * existing, tested ChartsSection (which owns the cost/credits/usage sub-tabs)
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
  // Keep the selection valid as the provider set changes (default to first).
  useEffect(() => {
    if (supported.length === 0) {
      setSelectedId(null);
      return;
    }
    setSelectedId((prev) =>
      prev && supported.some((p) => p.providerId === prev)
        ? prev
        : supported[0].providerId,
    );
  }, [supported]);

  if (supported.length === 0) {
    return (
      <div className="charts-empty">
        <strong>No charts yet</strong>
        Cost and usage history shows up here for providers that report it —
        Codex, Claude, and OpenAI.
      </div>
    );
  }

  const selected =
    supported.find((p) => p.providerId === selectedId) ?? supported[0];

  return (
    <div className="charts-panel">
      {supported.length > 1 && (
        <div className="charts-provider-tabs" role="tablist" aria-label="Provider">
          {supported.map((p) => {
            const isActive = p.providerId === selected.providerId;
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
      <ChartsSection
        key={selected.providerId}
        providerId={selected.providerId}
        accountEmail={selected.accountEmail}
        t={t}
      />
    </div>
  );
}
