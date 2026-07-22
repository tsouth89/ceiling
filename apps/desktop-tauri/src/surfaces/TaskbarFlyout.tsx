import { useCallback, useEffect, useMemo, useRef, useState, type CSSProperties } from "react";
import { getCurrentWindow, LogicalSize } from "@tauri-apps/api/window";
import { CeilingMark } from "../components/CeilingMark";
import { ProviderIcon } from "../components/providers/ProviderIcon";
import { useProviders } from "../hooks/useProviders";
import { useSettings } from "../hooks/useSettings";
import { getProviderIcon } from "../components/providers/providerIcons";
import { orderProviderSnapshots } from "../lib/providerOrder";
import { allMeasuredWindows, codexResetCredits, type ConstrainingWindow } from "../lib/capacityPresentation";
import {
  dismissTrayPanel,
  getTaskbarSurfaceColor,
  reanchorTrayPanel,
  revealTrayPanelWindow,
  setSurfaceMode,
} from "../lib/tauri";
import type { BootstrapState, ProviderUsageSnapshot, RateWindowSnapshot } from "../types/bridge";
import {
  accountIdentityLabel,
  hasMultipleAccounts,
  providerRowKey,
} from "../lib/providerRow";

const FLYOUT_WIDTH = 344;
const MAX_VISIBLE_PROVIDERS = 6;
const MAX_VISIBLE_WINDOWS_PER_PROVIDER = 4;

function compactDuration(resetsAt: string | null, fallback: string | null, now: number): string {
  if (!resetsAt) return fallback?.replace(/^resets?\s+(in\s+)?/i, "") ?? "No reset";
  const target = Date.parse(resetsAt);
  if (!Number.isFinite(target)) return fallback ?? "No reset";
  const minutes = Math.max(0, Math.ceil((target - now) / 60_000));
  if (minutes === 0) return "Now";
  const days = Math.floor(minutes / 1440);
  const hours = Math.floor((minutes % 1440) / 60);
  const mins = minutes % 60;
  if (days > 0) return `${days}d ${hours}h`;
  if (hours > 0) return `${hours}h ${mins}m`;
  return `${mins}m`;
}

function valueFor(window: RateWindowSnapshot, showAsUsed: boolean): number {
  return Math.max(0, Math.min(100, Math.round(showAsUsed ? window.usedPercent : window.remainingPercent)));
}

function meterLevel(window: RateWindowSnapshot): "normal" | "warning" | "critical" {
  if (window.usedPercent >= 95) return "critical";
  if (window.usedPercent >= 85) return "warning";
  return "normal";
}

function earliestReset(providers: ProviderUsageSnapshot[], now: number): string | null {
  let earliest: string | null = null;
  let earliestTime = Number.POSITIVE_INFINITY;
  for (const provider of providers) {
    if (provider.error) continue;
    const windows = allMeasuredWindows(provider).filter(isUtilityWindow);
    for (const { window } of windows) {
      const candidate = window?.resetsAt;
      if (!candidate) continue;
      const time = Date.parse(candidate);
      if (Number.isFinite(time) && time > now && time < earliestTime) {
        earliest = candidate;
        earliestTime = time;
      }
    }
  }
  return earliest;
}

function isUtilityWindow(window: ConstrainingWindow): boolean {
  const identity = `${window.id} ${window.label}`.toLowerCase();
  return ![
    "promotional",
    "on-demand",
    "on demand",
    "ondemand",
  ].some((noise) => identity.includes(noise));
}

function flyoutWindows(provider: ProviderUsageSnapshot): ConstrainingWindow[] {
  const windows = allMeasuredWindows(provider).filter(isUtilityWindow);
  if (provider.providerId !== "cursor") {
    return windows.slice(0, MAX_VISIBLE_WINDOWS_PER_PROVIDER);
  }

  // Cursor's three durable allowances are the useful comparison. Keep API in
  // the first three even if the provider inserts another auxiliary pool.
  const preferredIds = ["primary", "secondary", "extra-cursor-api"];
  const preferred = preferredIds
    .map((id) => windows.find((window) => window.id === id))
    .filter((window): window is ConstrainingWindow => Boolean(window));
  const remaining = windows.filter(
    (window) => !preferred.some((candidate) => candidate.id === window.id),
  );
  return [...preferred, ...remaining].slice(0, MAX_VISIBLE_WINDOWS_PER_PROVIDER);
}

function ProviderRow({ provider, showAccount, showAsUsed, now }: {
  provider: ProviderUsageSnapshot;
  // True when this provider has more than one account, so the account name is
  // needed to tell its rows apart. With one account it would be noise.
  showAccount: boolean;
  showAsUsed: boolean;
  now: number;
}) {
  const accountName = showAccount ? accountIdentityLabel(provider) : null;
  const icon = getProviderIcon(provider.providerId);
  if (provider.error) {
    return (
      <div className="taskbar-flyout__provider taskbar-flyout__provider--error" style={{ "--provider-brand": icon.brandColor } as CSSProperties}>
        <ProviderIcon providerId={provider.providerId} size={27} className="taskbar-flyout__provider-icon" />
        <div className="taskbar-flyout__provider-content">
          <div className="taskbar-flyout__provider-topline">
            <span className="taskbar-flyout__provider-name">{provider.displayName}</span>
            <span className="taskbar-flyout__provider-unavailable">Unavailable</span>
          </div>
          {accountName && (
            <div className="taskbar-flyout__provider-account" title={accountName}>
              {accountName}
            </div>
          )}
          <div className="taskbar-flyout__provider-status">Last sync failed · open Ceiling for details</div>
        </div>
      </div>
    );
  }
  const windows = flyoutWindows(provider);
  const resetCredits = codexResetCredits(provider);
  const hiddenWindowCount = Math.max(
    0,
    allMeasuredWindows(provider).filter(isUtilityWindow).length - windows.length,
  );
  return (
    <div className="taskbar-flyout__provider" style={{ "--provider-brand": icon.brandColor } as CSSProperties}>
      <ProviderIcon providerId={provider.providerId} size={27} className="taskbar-flyout__provider-icon" />
      <div className="taskbar-flyout__provider-content">
        <div className="taskbar-flyout__provider-topline">
          <span className="taskbar-flyout__provider-name">{provider.displayName}</span>
          {resetCredits != null && (
            <span
              className={`taskbar-flyout__reset-credit${resetCredits === 0 ? " taskbar-flyout__reset-credit--empty" : ""}`}
            >
              ↻ {resetCredits} {resetCredits === 1 ? "reset ready" : "resets ready"}
            </span>
          )}
        </div>
        {accountName && (
          <div className="taskbar-flyout__provider-account" title={accountName}>
            {accountName}
          </div>
        )}
        <div className="taskbar-flyout__meters">
          {windows.map(({ id, label, window }) => {
            const percent = valueFor(window, showAsUsed);
            const reset = compactDuration(window.resetsAt, window.resetDescription, now);
            const level = meterLevel(window);
            return (
              <div className="taskbar-flyout__meter" key={id} data-level={level}>
                <div className="taskbar-flyout__meter-meta">
                  <span className="taskbar-flyout__meter-label">{label}</span>
                  <span className="taskbar-flyout__meter-value" data-level={level}>{percent}%</span>
                  <span className="taskbar-flyout__reset">{reset}</span>
                </div>
                <div
                  className="taskbar-flyout__track"
                  data-level={level}
                  role="progressbar"
                  aria-label={`${provider.displayName} ${label} ${percent}%`}
                  aria-valuemin={0}
                  aria-valuemax={100}
                  aria-valuenow={percent}
                >
                  <span style={{ width: `${percent}%` }} />
                </div>
              </div>
            );
          })}
          {hiddenWindowCount > 0 && (
            <div className="taskbar-flyout__window-more">+{hiddenWindowCount} more limits in Ceiling</div>
          )}
        </div>
      </div>
    </div>
  );
}

export default function TaskbarFlyout({ state }: { state: BootstrapState }) {
  const { settings } = useSettings(state.settings);
  const { providers, hasLoadedCache } = useProviders({ initialRefreshDelayMs: 800 });
  const [now, setNow] = useState(Date.now());
  const [surfaceColor, setSurfaceColor] = useState<string | null>(null);
  const surfaceRef = useRef<HTMLElement>(null);

  const taskbarProviders = useMemo(() => {
    const ordered = orderProviderSnapshots(
      providers,
      state.providers,
      settings.enabledProviders,
      settings.providerOrder,
    );
    const selected = settings.floatBarProviderIds ?? [];
    if (selected.length === 0) return ordered;
    const selectedIds = new Set(selected);
    return ordered.filter((provider) => selectedIds.has(provider.providerId));
  }, [providers, settings.enabledProviders, settings.floatBarProviderIds, settings.providerOrder, state.providers]);
  const visibleProviders = taskbarProviders.slice(0, MAX_VISIBLE_PROVIDERS);
  const hiddenProviderCount = Math.max(0, taskbarProviders.length - visibleProviders.length);
  const visibleWindowCount = visibleProviders.reduce(
    (total, provider) => total + flyoutWindows(provider).length,
    0,
  );

  const nextReset = earliestReset(taskbarProviders, now);
  const nextResetText = nextReset ? `Next reset in ${compactDuration(nextReset, null, now)}` : "Usage at a glance";

  useEffect(() => {
    const timer = window.setInterval(() => setNow(Date.now()), 30_000);
    void getTaskbarSurfaceColor().then(setSurfaceColor).catch(() => {});
    return () => window.clearInterval(timer);
  }, []);

  useEffect(() => {
    if (!hasLoadedCache && visibleProviders.length === 0) return;
    let frame: number | null = null;
    const resize = () => {
      if (frame !== null) window.cancelAnimationFrame(frame);
      frame = window.requestAnimationFrame(() => {
        frame = null;
      // Windows owns the rounded outer edge; size directly to the content so
      // no transparent or CSS-border gutter can appear around that shape.
      const height = Math.max(174, Math.ceil(surfaceRef.current?.scrollHeight ?? 174));
      void (async () => {
        await getCurrentWindow().setSize(new LogicalSize(FLYOUT_WIDTH, height)).catch(() => {});
        await reanchorTrayPanel().catch(() => {});
        await revealTrayPanelWindow().catch(() => {});
      })();
      });
    };
    resize();
    const surface = surfaceRef.current;
    const observer = surface && typeof ResizeObserver !== "undefined"
      ? new ResizeObserver(resize)
      : null;
    if (surface) observer?.observe(surface);
    return () => {
      observer?.disconnect();
      if (frame !== null) window.cancelAnimationFrame(frame);
    };
  }, [hasLoadedCache, hiddenProviderCount, visibleProviders.length, visibleWindowCount]);

  const openCeiling = useCallback(() => {
    void setSurfaceMode("popOut", { kind: "dashboard" })
      .then(() => dismissTrayPanel())
      .catch(() => {
        // Keep the flyout available if the dashboard could not be opened.
      });
  }, []);

  useEffect(() => {
    const onKeyDown = (event: KeyboardEvent) => {
      if (event.key === "Escape") void dismissTrayPanel().catch(() => {});
    };
    window.addEventListener("keydown", onKeyDown);
    return () => window.removeEventListener("keydown", onKeyDown);
  }, []);

  useEffect(() => {
    const surface = surfaceRef.current;
    if (!surface) return;
    let cleanupTimer: number | undefined;
    const replayEntrance = () => {
      surface.classList.remove("taskbar-flyout--entering");
      void surface.offsetWidth;
      surface.classList.add("taskbar-flyout--entering");
      window.clearTimeout(cleanupTimer);
      cleanupTimer = window.setTimeout(
        () => surface.classList.remove("taskbar-flyout--entering"),
        150,
      );
    };
    replayEntrance();
    window.addEventListener("focus", replayEntrance);
    return () => {
      window.removeEventListener("focus", replayEntrance);
      window.clearTimeout(cleanupTimer);
    };
  }, []);

  return (
    <main className="taskbar-flyout-frame" style={{ "--taskbar-surface": surfaceColor ?? "#073b78" } as CSSProperties}>
      <section className="taskbar-flyout" ref={surfaceRef} aria-label="Ceiling usage at a glance">
        <header className="taskbar-flyout__header">
          <CeilingMark size={32} appearance="glass" className="taskbar-flyout__mark" />
          <div>
            <div className="taskbar-flyout__title">Ceiling</div>
            <div className="taskbar-flyout__subtitle">{nextResetText}</div>
          </div>
        </header>

        <div className="taskbar-flyout__providers">
          {visibleProviders.map((provider) => (
            <ProviderRow
              key={providerRowKey(provider)}
              provider={provider}
              showAccount={hasMultipleAccounts(taskbarProviders, provider.providerId)}
              showAsUsed={settings.showAsUsed}
              now={now}
            />
          ))}
          {visibleProviders.length === 0 && (
            <div className="taskbar-flyout__empty">Syncing provider usage…</div>
          )}
          {hiddenProviderCount > 0 && (
            <div className="taskbar-flyout__more">+{hiddenProviderCount} more in Ceiling</div>
          )}
        </div>

        <button type="button" className="taskbar-flyout__open" onClick={openCeiling}>
          <svg viewBox="0 0 16 16" aria-hidden><path d="M6.5 3H3.8A1.8 1.8 0 0 0 2 4.8v7.4A1.8 1.8 0 0 0 3.8 14h7.4a1.8 1.8 0 0 0 1.8-1.8V9.5M9 2h5v5M8 8l6-6" /></svg>
          <span>Open Ceiling</span>
        </button>
      </section>
    </main>
  );
}
