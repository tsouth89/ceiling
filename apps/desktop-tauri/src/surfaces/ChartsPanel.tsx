import { useEffect, useMemo, useState } from "react";
import type { ProviderUsageSnapshot } from "../types/bridge";
import { useLocale } from "../hooks/useLocale";
import { useTabListKeyboard } from "../hooks/useTabListKeyboard";
import { ProviderIcon } from "../components/providers/ProviderIcon";
import { providerSupportsChartData } from "../lib/providerCharts";
import { ChartsSection } from "./settings/providers/sections/charts/ChartsSection";
import ProviderComparison from "./ProviderComparison";
import { TotalApiValueCard } from "../components/TotalApiValueCard";
import {
  onePerProvider,
  representativeForProvider,
} from "../lib/providerRow";

const COMPARE_ID = "compare";

export function chartSectionKey(provider: ProviderUsageSnapshot): string {
  const identity =
    provider.accountId ??
    provider.accountEmail ??
    provider.accountOrganization ??
    "ambient";
  return `${provider.providerId}:${identity}`;
}

/**
 * Charts tab: quota, local token, credits, and usage history per provider.
 *
 * Providers with chart history (Codex, Claude, Cursor, OpenAI, Grok, …) get a
 * tab. Codex/Claude also scan local transcripts; others use snapshot samples
 * Ceiling records while they are enabled. Reuses ChartsSection for the body.
 */
export default function ChartsPanel({
  providers,
}: {
  providers: ProviderUsageSnapshot[];
}) {
  const { t } = useLocale();

  const supported = useMemo(
    () =>
      onePerProvider(
        providers.filter(
          (p) => providerSupportsChartData(p.providerId) && !p.error,
        ),
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
      prev &&
      (supported.some((p) => p.providerId === prev) ||
        (prev === COMPARE_ID && comparisonProviders))
        ? prev
        : comparisonProviders
          ? COMPARE_ID
          : supported[0].providerId,
    );
  }, [supported, comparisonProviders]);

  const tabIds = useMemo(
    () => [
      ...(comparisonProviders ? [COMPARE_ID] : []),
      ...supported.map((p) => p.providerId),
    ],
    [comparisonProviders, supported],
  );

  const comparing = selectedId === COMPARE_ID && comparisonProviders !== null;
  // The selection settles in an effect, so until it does the strip reports what
  // the body actually renders: the first supported provider.
  const activeTabId = comparing
    ? COMPARE_ID
    : (tabIds.includes(selectedId ?? "") ? selectedId : supported[0]?.providerId) ?? null;

  // Manual activation: picking a provider remounts ChartsSection, which loads
  // that provider's history. Arrowing across the strip should not fire a load
  // per tab it passes over.
  const { tabListProps, getTabProps, getPanelProps } = useTabListKeyboard({
    tabIds,
    selectedId: activeTabId,
    onSelect: setSelectedId,
    activation: "manual",
  });

  if (supported.length === 0) {
    // The API-value card loads its own local totals, so keep it visible even
    // when no provider reports chart-series data (or a snapshot errored).
    return (
      <div className="charts-panel">
        <TotalApiValueCard />
        <div className="charts-empty">
          <strong>No charts yet</strong>
          Limits and local usage history show up here for providers Ceiling can
          chart — Codex, Claude, Cursor, OpenAI, and Grok (weekly pool + local sessions).
        </div>
      </div>
    );
  }

  const selected =
    supported.find((p) => p.providerId === selectedId) ?? supported[0];
  const tabCount = tabIds.length;

  return (
    <div className="charts-panel">
      <TotalApiValueCard />
      {tabCount > 1 && (
        <div className="charts-provider-tabs" {...tabListProps} aria-label="Provider">
          {comparisonProviders && (
            <button
              type="button"
              {...getTabProps(COMPARE_ID)}
              className="charts-provider-tab charts-provider-tab--compare"
              data-active={comparing ? "true" : "false"}
              onClick={() => setSelectedId(COMPARE_ID)}
            >
              <span className="charts-provider-tab__compare-mark" aria-hidden>↔</span>
              <span>Compare</span>
            </button>
          )}
          {supported.map((p) => {
            const isActive = p.providerId === activeTabId;
            return (
              <button
                key={p.providerId}
                type="button"
                {...getTabProps(p.providerId)}
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
      <div
        {...(tabCount > 1 ? getPanelProps() : {})}
        className="charts-panel__body"
      >
        {comparing ? (
          <>
            <p className="charts-compare-note">
              Compares all Codex usage against all Claude usage on this machine,
              across every account.
            </p>
            <ProviderComparison providers={[comparisonProviders[0], comparisonProviders[1]]} />
          </>
        ) : (
          <ChartsSection
            key={chartSectionKey(selected)}
            providerId={selected.providerId}
            accountEmail={selected.accountEmail}
            accountId={selected.accountId}
            providerSnapshot={selected}
            t={t}
          />
        )}
      </div>
    </div>
  );
}
