import { Fragment, useCallback, useEffect, useMemo, useRef, useState, type ReactElement } from "react";
import { getCurrentWebviewWindow } from "@tauri-apps/api/webviewWindow";
import type { BootstrapState, ProviderUsageSnapshot } from "../types/bridge";
import { openSettingsWindow, quitApp as quitApplication } from "../lib/tauri";
import { useProviders } from "../hooks/useProviders";
import { useSettings } from "../hooks/useSettings";
import { useUpdateState } from "../hooks/useUpdateState";
import { useLocale } from "../hooks/useLocale";
import MenuCard from "../components/MenuCard";
import PlanStatusCard from "../components/PlanStatusCard";
import PopOutTitleBar from "../components/PopOutTitleBar";
import ActivityTimeline from "./ActivityTimeline";
import ChartsPanel from "./ChartsPanel";
import AccountsPanel from "./AccountsPanel";
import { MenuEmpty } from "../components/MenuSurface";
import UpdateBanner from "../components/UpdateBanner";
import DetectedAccountsCard from "../components/DetectedAccountsCard";
import { orderProviderSnapshots } from "../lib/providerOrder";
import { formatRelativeUpdated } from "../lib/relativeTime";

type DashboardSection = "overview" | "activity" | "accounts" | "charts";

const RAIL_ICON = {
  fill: "none",
  stroke: "currentColor",
  strokeWidth: 2,
  strokeLinecap: "round",
  strokeLinejoin: "round",
} as const;

function IconGrid() {
  return (
    <svg viewBox="0 0 24 24" aria-hidden {...RAIL_ICON}>
      <rect x="3" y="3" width="7" height="7" rx="1.5" />
      <rect x="14" y="3" width="7" height="7" rx="1.5" />
      <rect x="3" y="14" width="7" height="7" rx="1.5" />
      <rect x="14" y="14" width="7" height="7" rx="1.5" />
    </svg>
  );
}
function IconClock() {
  return (
    <svg viewBox="0 0 24 24" aria-hidden {...RAIL_ICON}>
      <circle cx="12" cy="12" r="9" />
      <path d="M12 7v5l3 2" />
    </svg>
  );
}
function IconUsers() {
  return (
    <svg viewBox="0 0 24 24" aria-hidden {...RAIL_ICON}>
      <circle cx="9" cy="8" r="3.2" />
      <path d="M3.5 20a5.5 5.5 0 0 1 11 0" />
      <path d="M16 5.2a3.2 3.2 0 0 1 0 5.6" />
      <path d="M17.5 20a5.5 5.5 0 0 0-2.3-4.4" />
    </svg>
  );
}
function IconBars() {
  return (
    <svg viewBox="0 0 24 24" aria-hidden {...RAIL_ICON}>
      <path d="M6 20V10" />
      <path d="M12 20V4" />
      <path d="M18 20v-7" />
    </svg>
  );
}
function IconGear() {
  return (
    <svg viewBox="0 0 24 24" aria-hidden {...RAIL_ICON}>
      <circle cx="12" cy="12" r="3" />
      <path d="M19.4 15a1.65 1.65 0 0 0 .33 1.82l.06.06a2 2 0 1 1-2.83 2.83l-.06-.06a1.65 1.65 0 0 0-1.82-.33 1.65 1.65 0 0 0-1 1.51V21a2 2 0 0 1-4 0v-.09A1.65 1.65 0 0 0 9 19.4a1.65 1.65 0 0 0-1.82.33l-.06.06a2 2 0 1 1-2.83-2.83l.06-.06a1.65 1.65 0 0 0 .33-1.82 1.65 1.65 0 0 0-1.51-1H3a2 2 0 0 1 0-4h.09A1.65 1.65 0 0 0 4.6 9a1.65 1.65 0 0 0-.33-1.82l-.06-.06a2 2 0 1 1 2.83-2.83l.06.06a1.65 1.65 0 0 0 1.82.33H9a1.65 1.65 0 0 0 1-1.51V3a2 2 0 0 1 4 0v.09a1.65 1.65 0 0 0 1 1.51 1.65 1.65 0 0 0 1.82-.33l.06-.06a2 2 0 1 1 2.83 2.83l-.06.06a1.65 1.65 0 0 0-.33 1.82V9a1.65 1.65 0 0 0 1.51 1H21a2 2 0 0 1 0 4h-.09a1.65 1.65 0 0 0-1.51 1z" />
    </svg>
  );
}
function IconSun() {
  return (
    <svg viewBox="0 0 24 24" aria-hidden {...RAIL_ICON}>
      <circle cx="12" cy="12" r="4" />
      <path d="M12 2v2M12 20v2M4.9 4.9l1.4 1.4M17.7 17.7l1.4 1.4M2 12h2M20 12h2M4.9 19.1l1.4-1.4M17.7 6.3l1.4-1.4" />
    </svg>
  );
}
function IconMoon() {
  return (
    <svg viewBox="0 0 24 24" aria-hidden {...RAIL_ICON}>
      <path d="M21 12.8A9 9 0 1 1 11.2 3a7 7 0 0 0 9.8 9.8z" />
    </svg>
  );
}

/**
 * Pop-out window — a windowed dashboard: left nav rail, an Overview of
 * plan-status glance cards (selecting a provider expands its full MenuCard),
 * and a bottom status bar. Mirrors the promo chrome.
 */
export default function PopOutPanel({
  state,
  providerId,
}: {
  state: BootstrapState;
  providerId?: string;
}) {
  const {
    providers,
    isRefreshing,
    refreshingProviderIds,
    refresh,
    hasCachedData,
  } = useProviders();
  const { settings, update } = useSettings(state.settings);
  const { updateState, checkNow, download, apply, dismiss, openRelease } =
    useUpdateState();
  const { t } = useLocale();

  const sorted = useMemo(() => {
    return orderProviderSnapshots(
      providers,
      state.providers,
      settings.enabledProviders,
      settings.providerOrder,
    );
  }, [providers, settings.enabledProviders, settings.providerOrder, state.providers]);
  const cachedProviderIds = useMemo(
    () => providers.map((provider) => provider.providerId),
    [providers],
  );
  const [selectedProviderId, setSelectedProviderId] = useState<string | null>(
    providerId ?? null,
  );
  const cardRefs = useRef(new Map<string, HTMLDivElement>());
  const windowScale = useMemo(() => {
    const scalePercent = Number(settings.windowScalePercent);
    return (
      Math.min(250, Math.max(100, Number.isFinite(scalePercent) ? scalePercent : 100)) / 100
    );
  }, [settings.windowScalePercent]);

  // Live "updated N ago" clock for the status bar; re-renders every 30s so the
  // relative time stays current without a refresh.
  const [nowMs, setNowMs] = useState(() => Date.now());
  useEffect(() => {
    const id = window.setInterval(() => setNowMs(Date.now()), 30_000);
    return () => window.clearInterval(id);
  }, []);
  const latestUpdatedMs = useMemo(() => {
    let latest: number | null = null;
    for (const p of sorted) {
      if (!p.updatedAt) continue;
      const ms = Date.parse(p.updatedAt);
      if (!Number.isNaN(ms) && (latest === null || ms > latest)) latest = ms;
    }
    return latest;
  }, [sorted]);

  // Scale the dashboard via the webview's native zoom (like a browser's Ctrl-+):
  // it reflows content at the real window width, so the side-by-side cards keep
  // filling the window at any scale — unlike CSS `zoom`, which overflows. The
  // main window is shared with the tray surface, so reset zoom to 1 on unmount.
  useEffect(() => {
    const webview = getCurrentWebviewWindow();
    void webview.setZoom(windowScale).catch(() => {});
    return () => {
      void webview.setZoom(1).catch(() => {});
    };
  }, [windowScale]);

  useEffect(() => {
    setSelectedProviderId(providerId ?? null);
  }, [providerId]);

  useEffect(() => {
    if (
      selectedProviderId !== null &&
      providers.length > 0 &&
      !sorted.some((provider) => provider.providerId === selectedProviderId)
    ) {
      setSelectedProviderId(null);
    }
  }, [providers.length, selectedProviderId, sorted]);

  const visibleProviders = useMemo(() => {
    if (selectedProviderId === null) {
      // The dashboard body scrolls, so show every provider (no compact cap).
      return sorted;
    }
    const match = sorted.find((p) => p.providerId === selectedProviderId);
    return match ? [match] : sorted;
  }, [sorted, selectedProviderId]);
  const providerOrderKey = useMemo(
    () => sorted.map((provider) => provider.providerId).join(","),
    [sorted],
  );

  const handleGridClick = useCallback((nextProviderId: string | null) => {
    setSelectedProviderId(nextProviderId);
  }, []);

  useEffect(() => {
    if (!providerId || selectedProviderId !== providerId || providerOrderKey.length === 0) return;

    let cancelled = false;
    const scrollToProvider = () => {
      if (cancelled) return;
      const target = cardRefs.current.get(providerId);
      if (!target) return;

      window.scrollTo(0, 0);
      if (document.scrollingElement) {
        document.scrollingElement.scrollTop = 0;
      }
      document.documentElement.scrollTop = 0;
      document.body.scrollTop = 0;

      for (const selector of [".menu-stack", ".menu-surface__body"]) {
        const container = target.closest<HTMLElement>(selector);
        if (!container) continue;
        container.scrollTop = 0;
        const targetRect = target.getBoundingClientRect();
        const containerRect = container.getBoundingClientRect();
        container.scrollTop += targetRect.top - containerRect.top;
      }
    };

    requestAnimationFrame(() => {
      requestAnimationFrame(scrollToProvider);
    });
    const timer = window.setTimeout(scrollToProvider, 100);
    const lateTimer = window.setTimeout(scrollToProvider, 350);
    return () => {
      cancelled = true;
      window.clearTimeout(timer);
      window.clearTimeout(lateTimer);
    };
  }, [providerId, selectedProviderId, providerOrderKey]);

  const openSettings = useCallback(() => {
    openSettingsWindow("general");
  }, []);
  const quitApp = useCallback(() => {
    void quitApplication();
  }, []);

  const enableDetectedProviders = useCallback(
    async (providerIds: string[]) => {
      const next = [...new Set([...settings.enabledProviders, ...providerIds])];
      await update({ enabledProviders: next });
      refresh();
    },
    [refresh, settings.enabledProviders, update],
  );

  const [activeSection, setActiveSection] = useState<DashboardSection>("overview");

  const isLight = settings.theme === "light";
  const toggleTheme = useCallback(() => {
    void update({ theme: isLight ? "dark" : "light" });
  }, [isLight, update]);

  // Keyboard shortcuts
  useEffect(() => {
    const handler = (e: KeyboardEvent) => {
      if (!e.ctrlKey || e.shiftKey || e.altKey || e.metaKey) return;
      switch (e.key.toLowerCase()) {
        case "r":
          e.preventDefault();
          refresh();
          break;
        case ",":
          e.preventDefault();
          openSettings();
          break;
        case "q":
          e.preventDefault();
          quitApp();
          break;
      }
    };
    window.addEventListener("keydown", handler);
    return () => window.removeEventListener("keydown", handler);
  }, [refresh, openSettings, quitApp]);

  const banner = (
    <UpdateBanner
      updateState={updateState}
      onCheck={checkNow}
      onDownload={download}
      onApply={apply}
      onDismiss={dismiss}
      onOpenRelease={openRelease}
    />
  );

  const railItems: { id: DashboardSection; label: string; icon: ReactElement }[] = [
    { id: "overview", label: "Overview", icon: <IconGrid /> },
    { id: "activity", label: "Activity", icon: <IconClock /> },
    { id: "accounts", label: "Accounts", icon: <IconUsers /> },
    { id: "charts", label: "Charts", icon: <IconBars /> },
  ];
  const sectionMeta: Record<
    DashboardSection,
    { title: string; sub: string; blurb: string }
  > = {
    overview: { title: "Overview", sub: "Usage at a glance", blurb: "" },
    activity: {
      title: "Activity",
      sub: "Upcoming resets",
      blurb: "A timeline of usage and resets across your providers.",
    },
    accounts: {
      title: "Accounts",
      sub: "Connected providers",
      blurb: "Manage the accounts and sources Ceiling reads from.",
    },
    charts: {
      title: "Charts",
      sub: "Usage over time",
      blurb: "Usage and cost trends, per provider.",
    },
  };
  const meta = sectionMeta[activeSection];

  return (
    <div className="popout-scale-shell dashboard-shell">
      <PopOutTitleBar />
      <div className="dashboard">
        <nav className="dashboard-rail" aria-label="Sections">
          {railItems.map((item) => (
            <button
              key={item.id}
              type="button"
              className={`dashboard-rail__btn${activeSection === item.id ? " dashboard-rail__btn--active" : ""}`}
              aria-label={item.label}
              aria-current={activeSection === item.id ? "page" : undefined}
              title={item.label}
              onClick={() => {
                setActiveSection(item.id);
                // Returning to Overview clears any focused provider so all
                // cards show again (the switcher used to do this).
                if (item.id === "overview") setSelectedProviderId(null);
              }}
            >
              {item.icon}
            </button>
          ))}
          <div className="dashboard-rail__spacer" />
          <button
            type="button"
            className="dashboard-rail__btn"
            aria-label={t("TooltipSettings")}
            title={t("TooltipSettings")}
            onClick={openSettings}
          >
            <IconGear />
          </button>
        </nav>

        <div className="dashboard-main">
          <header className="dashboard-header">
            <div className="dashboard-header__title">{meta.title}</div>
            <div className="dashboard-header__sub">{meta.sub}</div>
          </header>

          <div className="dashboard-body">
            {banner}
            {activeSection === "overview" ? (
              <>
                <DetectedAccountsCard
                  enabledProviderIds={settings.enabledProviders}
                  previouslyTrackedProviderIds={cachedProviderIds}
                  onEnable={enableDetectedProviders}
                  onManage={() => openSettingsWindow("providers")}
                />
                {sorted.length === 0 ? (
                  <MenuEmpty
                    isLoading={isRefreshing && !hasCachedData}
                    onSettings={openSettings}
                  />
                ) : (
                  <div className="menu-stack">
                    {visibleProviders.map((p, idx) => (
                      <Fragment key={p.providerId}>
                        {idx > 0 && <div className="menu-stack__sep" />}
                        <div
                          className={`menu-stack__item${selectedProviderId === p.providerId ? " menu-stack__item--selected" : ""}`}
                          ref={(node) => {
                            if (node) {
                              cardRefs.current.set(p.providerId, node);
                            } else {
                              cardRefs.current.delete(p.providerId);
                            }
                          }}
                        >
                          {selectedProviderId === null ? (
                            <PlanStatusCard
                              provider={p}
                              isRefreshing={refreshingProviderIds.has(p.providerId)}
                              resetTimeRelative={settings.resetTimeRelative}
                              showResetWhenExhausted={settings.showResetWhenExhausted}
                              showAsUsed={settings.showAsUsed}
                              onSelect={() => handleGridClick(p.providerId)}
                            />
                          ) : (
                            <MenuCard
                              provider={p}
                              isRefreshing={refreshingProviderIds.has(p.providerId)}
                              hideEmail={settings.hidePersonalInfo}
                              resetTimeRelative={settings.resetTimeRelative}
                              showResetWhenExhausted={settings.showResetWhenExhausted}
                              showAsUsed={settings.showAsUsed}
                              showActivitySection
                            />
                          )}
                        </div>
                      </Fragment>
                    ))}
                  </div>
                )}
              </>
            ) : activeSection === "activity" ? (
              <ActivityTimeline providers={sorted} />
            ) : activeSection === "charts" ? (
              <ChartsPanel providers={sorted} />
            ) : activeSection === "accounts" ? (
              <AccountsPanel
                providers={sorted}
                hideEmail={settings.hidePersonalInfo}
                onManage={() => openSettingsWindow("providers")}
              />
            ) : (
              <div className="dashboard-placeholder">
                <strong>{meta.title} — coming soon</strong>
                {meta.blurb} Ceiling is in its foundation phase; the Overview is
                live today.
              </div>
            )}
          </div>

          <footer className="dashboard-status">
            <button
              type="button"
              className="dashboard-status__toggle"
              onClick={toggleTheme}
              title="Toggle light / dark"
            >
              {isLight ? <IconSun /> : <IconMoon />}
              {isLight ? "Light" : "Dark"}
            </button>
            <span className="dashboard-status__center">
              <span
                className={`dashboard-status__dot${isRefreshing ? " dashboard-status__dot--busy" : ""}`}
                aria-hidden
              />
              {isRefreshing
                ? t("SummaryRefreshing")
                : formatRelativeUpdated(latestUpdatedMs, t, nowMs)}
            </span>
            <span>All times local</span>
          </footer>
        </div>
      </div>
    </div>
  );
}
