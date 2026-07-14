import {
  useCallback,
  useEffect,
  useMemo,
  useRef,
  useState,
  type CSSProperties,
  type MouseEvent,
} from "react";
import { listen } from "@tauri-apps/api/event";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { useFormattedResetTime } from "../hooks/useFormattedResetTime";
import { useLocale } from "../hooks/useLocale";
import { useProviders } from "../hooks/useProviders";
import {
  getProviderLocalUsageSummary,
  getSettingsSnapshot,
  openSettingsWindow,
  refreshProvidersIfStale,
  updateSettings,
} from "../lib/tauri";
import { ProviderIcon } from "../components/providers/ProviderIcon";
import { getProviderIcon } from "../components/providers/providerIcons";
import type {
  BootstrapState,
  CapacityEventPayload,
  ProviderLocalUsageSummary,
  ProviderUsageSnapshot,
  SettingsSnapshot,
} from "../types/bridge";
import {
  FLOAT_BAR_CONFIG_CHANGED_EVENT,
  hideFloatBar,
  resizeFloatBar,
  setFloatBarClickThrough,
} from "./api";
import FloatBarMenu from "./FloatBarMenu";
import {
  capacityFreshness,
  constrainingWindow,
  activePromoBoosts,
  type CapacityFreshness,
  type ConstrainingWindow,
} from "../lib/capacityPresentation";
import "./FloatBar.css";

function ResetIcon({ size }: { size: number }) {
  return (
    <svg
      className="floatbar__reset-icon-svg"
      width={size}
      height={size}
      viewBox="0 0 16 16"
      fill="none"
      aria-hidden="true"
    >
      <path
        d="M12.9 7.1a5 5 0 1 0-1.2 3.9"
        stroke="currentColor"
        strokeWidth="1.6"
        strokeLinecap="round"
      />
      <path
        d="M12.9 3.8v3.3H9.6"
        stroke="currentColor"
        strokeWidth="1.6"
        strokeLinecap="round"
        strokeLinejoin="round"
      />
    </svg>
  );
}

function inlineResetTime(resetText: string): string {
  const normalized = resetText.trim();
  if (/^reset(?:s|ting)?(?:\s+due)?\s*(?:now)?$/i.test(normalized)) {
    return "now";
  }
  return normalized
    .replace(/^resets?\s+in\s+/i, "")
    .replace(/^resets?\s+/i, "")
    .trim();
}

type FloatBarCostSummary = {
  key: string;
  providerId: string;
  displayName: string;
  todayCost: number | null;
  thirtyDayCost: number | null;
};

type FloatBarCostTarget = {
  key: string;
  providerId: string;
  displayName: string;
};

function providerCostKey(provider: ProviderUsageSnapshot): string {
  return `${provider.providerId}:${provider.accountEmail ?? ""}`;
}

function hasLocalCost(summary: ProviderLocalUsageSummary | null): summary is ProviderLocalUsageSummary {
  return summary?.todayCost != null || summary?.thirtyDayCost != null;
}

function formatUsd(value: number | null): string | null {
  if (value == null || !Number.isFinite(value)) return null;
  return `$${value.toFixed(2)}`;
}

/**
 * Cursor exposes a total plan meter alongside Auto/API lanes. The compact
 * strip should headline that account-wide total; the tray detail remains the
 * place to compare the individual lanes.
 */
function floatBarWindow(provider: ProviderUsageSnapshot): ConstrainingWindow {
  if (provider.providerId === "cursor") {
    return {
      id: "primary",
      label: "Total",
      window: provider.primary,
    };
  }
  return constrainingWindow(provider);
}

function CostPill({
  summary,
  scale,
  todayLabel,
  thirtyDayLabel,
}: {
  summary: FloatBarCostSummary;
  scale: number;
  todayLabel: string;
  thirtyDayLabel: string;
}) {
  const today = formatUsd(summary.todayCost);
  const thirtyDay = formatUsd(summary.thirtyDayCost);
  const iconSize = Math.round(10 * scale);
  const brand = getProviderIcon(summary.providerId).brandColor;
  const title = [
    today ? `${todayLabel} ${today}` : null,
    thirtyDay ? `${thirtyDayLabel} ${thirtyDay}` : null,
  ]
    .filter(Boolean)
    .join(" / ");

  return (
    <div
      className="floatbar__cost-pill"
      title={`${summary.displayName}: ${title}`}
      data-tauri-drag-region
      style={{ "--brand": brand } as CSSProperties}
    >
      <span className="floatbar__provider-icon" data-tauri-drag-region>
        <ProviderIcon providerId={summary.providerId} size={iconSize} />
      </span>
      <span className="floatbar__cost-items" data-tauri-drag-region>
        {today && (
          <span className="floatbar__cost-item" data-tauri-drag-region>
            <span className="floatbar__cost-label" data-tauri-drag-region>
              {todayLabel}
            </span>
            <span className="floatbar__cost-value" data-tauri-drag-region>
              {today}
            </span>
          </span>
        )}
        {thirtyDay && (
          <span className="floatbar__cost-item" data-tauri-drag-region>
            <span className="floatbar__cost-label" data-tauri-drag-region>
              {thirtyDayLabel}
            </span>
            <span className="floatbar__cost-value" data-tauri-drag-region>
              {thirtyDay}
            </span>
          </span>
        )}
      </span>
    </div>
  );
}
/**
 * The capacity pill shown for a single provider.
 *
 * Hero % is the primary plan pool (same as overview cards). A hot companion
 * lane (Auto/API/5-hour) appears as a quiet secondary chip. Tone still
 * follows the constraining window so pressure isn't hidden.
 */
function ProviderPill({
  provider,
  highRemaining,
  critRemaining,
  showAsUsed,
  scale,
  showResetInline,
  resetRelative,
  usedSuffix,
  remainingSuffix,
  capacityEventKind,
}: {
  provider: ProviderUsageSnapshot;
  highRemaining: number;
  critRemaining: number;
  showAsUsed: boolean;
  scale: number;
  showResetInline: boolean;
  resetRelative: boolean;
  usedSuffix: string;
  remainingSuffix: string;
  capacityEventKind?: CapacityEventPayload["kind"];
}) {
  // Keep the number, label, and tone tied to the same displayed window.
  const hero = floatBarWindow(provider);
  const freshness = capacityFreshness(provider);
  const boosts = activePromoBoosts(provider);
  const remaining = Math.max(0, Math.min(100, hero.window.remainingPercent));
  const used = Math.max(0, Math.min(100, hero.window.usedPercent));
  const displayPercent = showAsUsed ? used : remaining;
  const displaySuffix = showAsUsed ? usedSuffix : remainingSuffix;
  const pressureRemaining = Math.max(
    0,
    Math.min(100, hero.window.remainingPercent),
  );
  const exhausted =
    hero.window.isExhausted || !!provider.error;
  let tone: "ok" | "warn" | "crit" = "ok";
  if (exhausted || pressureRemaining <= critRemaining) tone = "crit";
  else if (pressureRemaining <= highRemaining) tone = "warn";

  const brand = getProviderIcon(provider.providerId).brandColor;
  const label = provider.error ? "—" : `${Math.round(displayPercent)}%`;
  const resetText = useFormattedResetTime(
    hero.window.resetsAt,
    hero.window.resetDescription,
    resetRelative,
  );
  const inlineReset = resetText ? inlineResetTime(resetText) : null;
  const iconSize = Math.round(14 * scale);
  const resetIconSize = Math.round(10 * scale);
  const stateChip = freshnessChipLabel(freshness);
  const boostTitle = boosts[0]?.title ?? null;
  // Strip always shows reset when depleted; otherwise honor the setting.
  const showReset = !!inlineReset && (showResetInline || exhausted);
  const titleBits = [
    `${provider.displayName}: ${label} ${displaySuffix}`,
    hero.label,
    boostTitle ? `promo ${boostTitle}` : null,
    stateChip ? `state ${stateChip}` : null,
    resetText,
  ]
    .filter(Boolean)
    .join("\n");

  return (
    <div
      className={[
        "floatbar__pill",
        `floatbar__pill--${tone}`,
        freshness !== "live" ? `floatbar__pill--${freshness}` : null,
        capacityEventKind ? "floatbar__pill--capacity-event" : null,
        capacityEventKind ? `floatbar__pill--${capacityEventKind}` : null,
      ]
        .filter(Boolean)
        .join(" ")}
      title={titleBits}
      data-tauri-drag-region
      style={{ "--brand": brand } as CSSProperties}
    >
      <span className="floatbar__provider-icon" data-tauri-drag-region>
        <ProviderIcon providerId={provider.providerId} size={iconSize} />
      </span>
      <span className="floatbar__text" data-tauri-drag-region>
        <span className="floatbar__pct" data-tauri-drag-region>
          {label}
        </span>
        <span className="floatbar__window" data-tauri-drag-region>
          {hero.label}
        </span>
        {stateChip && (
          <span
            className={`floatbar__chip floatbar__chip--${freshness}`}
            data-tauri-drag-region
          >
            {stateChip}
          </span>
        )}
        {showReset && (
          <span
            className={`floatbar__reset${exhausted ? " floatbar__reset--emphasis" : ""}`}
            title={resetText ?? undefined}
            aria-label={resetText ?? undefined}
            data-tauri-drag-region
          >
            <ResetIcon size={resetIconSize} />
            <span className="floatbar__reset-time" data-tauri-drag-region>
              {inlineReset}
            </span>
          </span>
        )}
      </span>
    </div>
  );
}

function freshnessChipLabel(freshness: CapacityFreshness): string | null {
  switch (freshness) {
    case "stale":
      return "stale";
    case "error":
      return "error";
    case "lifted":
      return "lifted";
    case "live":
      return null;
  }
}

/**
 * The always-on-top floating capacity bar.
 *
 * Renders a tiny strip of provider pills. Listens to the same provider
 * refresh cycle as the rest of the app via `useProviders`, and reacts to
 * setting changes (filter list, orientation) live without a reload.
 */
/** localStorage key persisting the float bar's "lock in place" state. */
const FLOATBAR_LOCK_KEY = "ceiling.floatbar.locked";

export default function FloatBar({ state }: { state: BootstrapState }) {
  const { t } = useLocale();
  const { providers } = useProviders({
    refreshOnMount: false,
  });
  const [locked, setLocked] = useState(
    () => localStorage.getItem(FLOATBAR_LOCK_KEY) === "1",
  );
  const lockedRef = useRef(locked);
  lockedRef.current = locked;
  // Right-click opens an in-place action row (see FloatBarMenu). Kept in a ref
  // too so the capture-phase drag guard can read it without re-subscribing.
  const [menuOpen, setMenuOpen] = useState(false);
  const menuOpenRef = useRef(menuOpen);
  menuOpenRef.current = menuOpen;

  const startDrag = useCallback((event: MouseEvent<HTMLElement>) => {
    if (event.button !== 0 || lockedRef.current || menuOpenRef.current) return;
    void getCurrentWindow().startDragging().catch(() => {});
  }, []);

  const toggleLock = useCallback(() => {
    setLocked((prev) => {
      const next = !prev;
      try {
        localStorage.setItem(FLOATBAR_LOCK_KEY, next ? "1" : "0");
      } catch {
        /* storage disabled — the lock just won't persist across restarts */
      }
      return next;
    });
  }, []);

  // Replace the webview's generic right-click menu with a purposeful action row.
  // While locked, the native `data-tauri-drag-region` drag is suppressed so the
  // bar can't be nudged; menu clicks are always let through.
  useEffect(() => {
    const onContextMenu = (event: globalThis.MouseEvent) => {
      event.preventDefault();
      setMenuOpen((prev) => !prev);
    };
    const onKeyDown = (event: globalThis.KeyboardEvent) => {
      if (event.key === "Escape") setMenuOpen(false);
    };
    const onMouseDownCapture = (event: globalThis.MouseEvent) => {
      // Never eat clicks aimed at the action row.
      if ((event.target as HTMLElement | null)?.closest(".floatbar__menu")) {
        return;
      }
      if (lockedRef.current && event.button === 0) {
        // Pre-empt Tauri's drag-region handler before it starts a move.
        event.preventDefault();
        event.stopPropagation();
      }
    };
    document.addEventListener("contextmenu", onContextMenu);
    document.addEventListener("keydown", onKeyDown);
    document.addEventListener("mousedown", onMouseDownCapture, true);
    return () => {
      document.removeEventListener("contextmenu", onContextMenu);
      document.removeEventListener("keydown", onKeyDown);
      document.removeEventListener("mousedown", onMouseDownCapture, true);
    };
  }, []);

  // Mark the body so our CSS can strip the dark theme background — the
  // floatbar window is meant to be fully transparent around the pills.
  useEffect(() => {
    document.body.classList.add("floatbar-window");
    return () => {
      document.body.classList.remove("floatbar-window");
    };
  }, []);

  // The floatbar window is detached, so it doesn't share React state
  // with the Settings tab. Listen for the Rust-side config-changed event
  // and re-pull the snapshot when fired.
  const [settings, setSettings] = useState<SettingsSnapshot>(state.settings);
  const [localCosts, setLocalCosts] = useState<Record<string, FloatBarCostSummary>>({});
  const [capacityEvents, setCapacityEvents] = useState<
    Record<string, CapacityEventPayload["kind"]>
  >({});
  const capacityEventTimers = useRef<Record<string, number>>({});

  // The detached floatbar should keep usage fresh, but it must not open or
  // focus any other surface. Refresh data only; provider-updated events feed
  // this window when the backend completes.
  useEffect(() => {
    const intervalMs = Math.max(60_000, settings.refreshIntervalSecs * 1000);
    const tick = () => {
      void refreshProvidersIfStale().catch(() => {});
    };
    tick();
    const id = setInterval(tick, intervalMs);
    return () => clearInterval(id);
  }, [settings.refreshIntervalSecs]);

  useEffect(() => {
    const unlisten = listen(FLOAT_BAR_CONFIG_CHANGED_EVENT, () => {
      void getSettingsSnapshot().then(setSettings).catch(() => {});
    });
    return () => {
      void unlisten.then((fn) => fn());
    };
  }, []);

  useEffect(() => {
    const unlisten = listen<CapacityEventPayload>("capacity-event", ({ payload }) => {
      if (!settings.enableAnimations) return;
      const providerId = payload.providerId;
      const previousTimer = capacityEventTimers.current[providerId];
      if (previousTimer !== undefined) window.clearTimeout(previousTimer);
      setCapacityEvents((current) => ({ ...current, [providerId]: payload.kind }));
      capacityEventTimers.current[providerId] = window.setTimeout(() => {
        setCapacityEvents((current) => {
          const next = { ...current };
          delete next[providerId];
          return next;
        });
        delete capacityEventTimers.current[providerId];
      }, 2200);
    });
    return () => {
      void unlisten.then((fn) => fn());
    };
  }, [settings.enableAnimations]);

  useEffect(
    () => () => {
      for (const timer of Object.values(capacityEventTimers.current)) {
        window.clearTimeout(timer);
      }
    },
    [],
  );

  // Orientation flips re-lay-out the bar without recreating the window.
  const orientation: "horizontal" | "vertical" =
    settings.floatBarOrientation === "vertical" ? "vertical" : "horizontal";
  const style = settings.floatBarStyle === "taskbar" ? "taskbar" : "floating";
  const filterIds = settings.floatBarProviderIds;
  const scale = Math.max(0.75, Math.min(2, settings.floatBarScale / 100));
  const showResetInline = settings.floatBarShowResetInline;
  const showCost = settings.floatBarShowCost;
  const visible = useMemo(() => {
    const enabled = new Set(settings.enabledProviders);
    let list = providers.filter((p) => enabled.has(p.providerId));
    if (filterIds && filterIds.length > 0) {
      const wanted = new Set(filterIds);
      list = list.filter((p) => wanted.has(p.providerId));
    }
    return [...list].sort(
      (a, b) =>
        floatBarWindow(b).window.usedPercent -
        floatBarWindow(a).window.usedPercent,
    );
  }, [providers, settings.enabledProviders, filterIds]);

  const visibleCostTargetKey = visible
    .map((p) => `${providerCostKey(p)}:${p.providerId}:${p.displayName}`)
    .join("|");
  const visibleCostTargets = useMemo<FloatBarCostTarget[]>(
    () =>
      showCost
        ? visible.map((provider) => ({
            key: providerCostKey(provider),
            providerId: provider.providerId,
            displayName: provider.displayName,
          }))
        : [],
    [showCost, visibleCostTargetKey],
  );

  useEffect(() => {
    let cancelled = false;
    const targets = visibleCostTargets;

    if (targets.length === 0) {
      setLocalCosts({});
      return () => {
        cancelled = true;
      };
    }

    Promise.allSettled(
      targets.map(async (target) => {
        const localUsage = await getProviderLocalUsageSummary(target.providerId);
        if (!hasLocalCost(localUsage)) return null;
        return {
          key: target.key,
          providerId: target.providerId,
          displayName: target.displayName,
          todayCost: localUsage.todayCost,
          thirtyDayCost: localUsage.thirtyDayCost,
        } satisfies FloatBarCostSummary;
      }),
    )
      .then((results) => {
        if (cancelled) return;
        const next: Record<string, FloatBarCostSummary> = {};
        for (const result of results) {
          if (result.status === "fulfilled" && result.value) {
            next[result.value.key] = result.value;
          }
        }
        setLocalCosts(next);
      })
      .catch(() => {
        if (!cancelled) setLocalCosts({});
      });

    return () => {
      cancelled = true;
    };
  }, [visibleCostTargets]);

  const visibleCosts = visible
    .map((provider) => localCosts[providerCostKey(provider)])
    .filter((summary): summary is FloatBarCostSummary => Boolean(summary));
  const visibleCostValuesKey = visibleCosts
    .map((summary) => `${summary.key}:${summary.todayCost ?? ""}:${summary.thirtyDayCost ?? ""}`)
    .join("|");
  // Keep the native floatbar window fitted when late data/fonts/icons change layout.
  const lastResizeRef = useRef<{ w: number; h: number } | null>(null);
  const resizeRafRef = useRef<number | null>(null);
  const resizeToContent = useCallback(() => {
    const el = document.querySelector<HTMLElement>(".floatbar");
    if (!el) return;
    if (resizeRafRef.current !== null) {
      cancelAnimationFrame(resizeRafRef.current);
    }
    resizeRafRef.current = requestAnimationFrame(() => {
      resizeRafRef.current = null;
      const rect = el.getBoundingClientRect();
      const padding = 8;
      const w = Math.ceil(rect.width + padding);
      const h = Math.ceil(rect.height + padding);
      const last = lastResizeRef.current;
      if (last && Math.abs(last.w - w) <= 1 && Math.abs(last.h - h) <= 1) return;
      lastResizeRef.current = { w, h };
      void resizeFloatBar(w, h).catch(() => {});
    });
  }, []);

  useEffect(() => {
    resizeToContent();
  }, [
    resizeToContent,
    visible.length,
    visibleCostValuesKey,
    orientation,
    style,
    scale,
    showResetInline,
    settings.resetTimeRelative,
    menuOpen,
  ]);

  useEffect(() => {
    const el = document.querySelector<HTMLElement>(".floatbar");
    if (!el || typeof ResizeObserver === "undefined") return;
    const observer = new ResizeObserver(resizeToContent);
    observer.observe(el);
    return () => observer.disconnect();
  }, [resizeToContent]);

  useEffect(
    () => () => {
      if (resizeRafRef.current !== null) {
        cancelAnimationFrame(resizeRafRef.current);
      }
    },
    [],
  );

  const highRemaining = 100 - settings.highUsageThreshold;
  const critRemaining = 100 - settings.criticalUsageThreshold;
  const opacityFraction = Math.max(0.3, Math.min(1, settings.floatBarOpacity / 100));

  const handleToggleLock = useCallback(() => {
    toggleLock();
    setMenuOpen(false);
  }, [toggleLock]);
  const handleToggleClickThrough = useCallback(() => {
    const next = !settings.floatBarClickThrough;
    void setFloatBarClickThrough(next).catch(() => {});
    void updateSettings({ floatBarClickThrough: next }).catch(() => {});
    setMenuOpen(false);
  }, [settings.floatBarClickThrough]);
  const handleOpenSettings = useCallback(() => {
    void openSettingsWindow("menuBar").catch(() => {});
    setMenuOpen(false);
  }, []);
  const handleHide = useCallback(() => {
    void hideFloatBar().catch(() => {});
    void updateSettings({ floatBarEnabled: false }).catch(() => {});
    setMenuOpen(false);
  }, []);

  return (
    <div
      className={`floatbar floatbar--${orientation} floatbar--${style}${settings.floatBarDarkText ? " floatbar--light-bg" : ""}${locked ? " floatbar--locked" : ""}${menuOpen ? " floatbar--menu-open" : ""}`}
      data-tauri-drag-region
      onMouseDown={startDrag}
      style={
        {
          opacity: opacityFraction,
          "--floatbar-scale": scale,
        } as CSSProperties
      }
    >
      {menuOpen ? (
        <FloatBarMenu
          locked={locked}
          clickThrough={settings.floatBarClickThrough}
          onToggleLock={handleToggleLock}
          onToggleClickThrough={handleToggleClickThrough}
          onOpenSettings={handleOpenSettings}
          onHide={handleHide}
        />
      ) : visible.length === 0 ? (
        <div className="floatbar__empty" data-tauri-drag-region>
          {t("FloatBarNoProviders")}
        </div>
      ) : (
        // One unified glass capsule; each provider is a segment split by a
        // hairline divider, with the drag grip on the left. Reads as a single
        // premium widget instead of scattered pills.
        <div className="floatbar__bar" data-tauri-drag-region>
          <div className="floatbar__handle" data-tauri-drag-region aria-hidden />
          {locked && (
            <span
              className="floatbar__lock"
              aria-hidden
              title="Locked in place — right-click to unlock"
            >
              <svg
                viewBox="0 0 24 24"
                width="11"
                height="11"
                fill="none"
                stroke="currentColor"
                strokeWidth={2.2}
                strokeLinecap="round"
                strokeLinejoin="round"
              >
                <rect x="5" y="11" width="14" height="9" rx="2" />
                <path d="M8 11V8a4 4 0 0 1 8 0v3" />
              </svg>
            </span>
          )}
          {visible.map((p) => (
            <ProviderPill
              key={providerCostKey(p)}
              provider={p}
              highRemaining={highRemaining}
              critRemaining={critRemaining}
              showAsUsed={settings.showAsUsed}
              scale={scale}
              showResetInline={showResetInline}
              resetRelative={settings.resetTimeRelative}
              usedSuffix={t("PanelUsedSuffix")}
              remainingSuffix={t("FloatBarRemainingSuffix")}
              capacityEventKind={capacityEvents[p.providerId]}
            />
          ))}
          {visibleCosts.map((summary) => (
            <CostPill
              key={`cost:${summary.key}`}
              summary={summary}
              scale={scale}
              todayLabel={t("PanelToday")}
              thirtyDayLabel={t("FloatBarThirtyDayShort")}
            />
          ))}
        </div>
      )}
    </div>
  );
}
