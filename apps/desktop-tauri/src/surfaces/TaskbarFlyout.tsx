import { useCallback, useEffect, useMemo, useRef, useState, type CSSProperties } from "react";
import { getCurrentWindow, LogicalSize } from "@tauri-apps/api/window";
import { CeilingMark } from "../components/CeilingMark";
import { ProviderIcon } from "../components/providers/ProviderIcon";
import { useProviders } from "../hooks/useProviders";
import { useSettings } from "../hooks/useSettings";
import { getProviderIcon } from "../components/providers/providerIcons";
import { orderProviderSnapshots } from "../lib/providerOrder";
import {
  dismissTrayPanel,
  getTaskbarSurfaceColor,
  reanchorTrayPanel,
  revealTrayPanelWindow,
  setSurfaceMode,
} from "../lib/tauri";
import type { BootstrapState, ProviderUsageSnapshot, RateWindowSnapshot } from "../types/bridge";

const FLYOUT_WIDTH = 344;
const MAX_VISIBLE_PROVIDERS = 6;

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

function earliestReset(providers: ProviderUsageSnapshot[]): string | null {
  let earliest: string | null = null;
  let earliestTime = Number.POSITIVE_INFINITY;
  for (const provider of providers) {
    const candidate = provider.primary.resetsAt;
    if (!candidate) continue;
    const time = Date.parse(candidate);
    if (Number.isFinite(time) && time > Date.now() && time < earliestTime) {
      earliest = candidate;
      earliestTime = time;
    }
  }
  return earliest;
}

function ProviderRow({ provider, showAsUsed, now }: {
  provider: ProviderUsageSnapshot;
  showAsUsed: boolean;
  now: number;
}) {
  const icon = getProviderIcon(provider.providerId);
  const percent = valueFor(provider.primary, showAsUsed);
  const reset = compactDuration(provider.primary.resetsAt, provider.primary.resetDescription, now);
  const label = provider.primaryLabel || (provider.primary.windowMinutes ? `${Math.round(provider.primary.windowMinutes / 60)}h` : "Limit");
  return (
    <div className="taskbar-flyout__provider" style={{ "--provider-brand": icon.brandColor } as CSSProperties}>
      <ProviderIcon providerId={provider.providerId} size={27} className="taskbar-flyout__provider-icon" />
      <div className="taskbar-flyout__provider-content">
        <div className="taskbar-flyout__provider-topline">
          <span className="taskbar-flyout__provider-name">{provider.displayName}</span>
          <span className="taskbar-flyout__provider-percent">{percent}%</span>
        </div>
        <div className="taskbar-flyout__provider-bottomline">
          <div className="taskbar-flyout__track" aria-label={`${provider.displayName} ${percent}%`}>
            <span style={{ width: `${percent}%` }} />
          </div>
          <span className="taskbar-flyout__reset">{label} · {reset}</span>
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

  const visibleProviders = useMemo(() => orderProviderSnapshots(
    providers,
    state.providers,
    settings.enabledProviders,
    settings.providerOrder,
  ).slice(0, MAX_VISIBLE_PROVIDERS), [providers, settings.enabledProviders, settings.providerOrder, state.providers]);

  const nextReset = earliestReset(visibleProviders);
  const nextResetText = nextReset ? `Next reset in ${compactDuration(nextReset, null, now)}` : "Usage at a glance";

  useEffect(() => {
    const timer = window.setInterval(() => setNow(Date.now()), 30_000);
    void getTaskbarSurfaceColor().then(setSurfaceColor).catch(() => {});
    return () => window.clearInterval(timer);
  }, []);

  useEffect(() => {
    if (!hasLoadedCache && visibleProviders.length === 0) return;
    const frame = window.requestAnimationFrame(() => {
      // Windows owns the rounded outer edge; size directly to the content so
      // no transparent or CSS-border gutter can appear around that shape.
      const height = Math.max(174, Math.ceil(surfaceRef.current?.scrollHeight ?? 174));
      void (async () => {
        await getCurrentWindow().setSize(new LogicalSize(FLYOUT_WIDTH, height)).catch(() => {});
        await reanchorTrayPanel().catch(() => {});
        await revealTrayPanelWindow().catch(() => {});
      })();
    });
    return () => window.cancelAnimationFrame(frame);
  }, [hasLoadedCache, visibleProviders.length]);

  const openCeiling = useCallback(() => {
    void setSurfaceMode("popOut", { kind: "dashboard" })
      .finally(() => dismissTrayPanel().catch(() => {}));
  }, []);

  useEffect(() => {
    const onKeyDown = (event: KeyboardEvent) => {
      if (event.key === "Escape") void dismissTrayPanel().catch(() => {});
    };
    window.addEventListener("keydown", onKeyDown);
    return () => window.removeEventListener("keydown", onKeyDown);
  }, []);

  return (
    <main className="taskbar-flyout-frame" style={{ "--taskbar-surface": surfaceColor ?? "#073b78" } as CSSProperties}>
      <section className="taskbar-flyout" ref={surfaceRef} aria-label="Ceiling usage at a glance">
        <header className="taskbar-flyout__header">
          <CeilingMark size={32} className="taskbar-flyout__mark" />
          <div>
            <div className="taskbar-flyout__title">Ceiling</div>
            <div className="taskbar-flyout__subtitle">{nextResetText}</div>
          </div>
        </header>

        <div className="taskbar-flyout__providers">
          {visibleProviders.map((provider) => (
            <ProviderRow key={provider.providerId} provider={provider} showAsUsed={settings.showAsUsed} now={now} />
          ))}
          {visibleProviders.length === 0 && (
            <div className="taskbar-flyout__empty">Syncing provider usage…</div>
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
