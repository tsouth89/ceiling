import { useCallback, useEffect, useMemo, useRef } from "react";
import { getCurrentWindow, LogicalPosition, LogicalSize } from "@tauri-apps/api/window";
import type { BootstrapState, ProviderUsageSnapshot } from "../types/bridge";
import { setSurfaceMode, openSettingsWindow, quitApp as quitApplication } from "../lib/tauri";
import { useProviders } from "../hooks/useProviders";
import { useSettings } from "../hooks/useSettings";
import { useUpdateState } from "../hooks/useUpdateState";
import { useLocale } from "../hooks/useLocale";
import MenuCard from "../components/MenuCard";
import MenuSurface, {
  MenuSummary,
  MenuEmpty,
  type MenuFooterRow,
} from "../components/MenuSurface";
import UpdateBanner from "../components/UpdateBanner";
import { DEMO_ENABLED, DEMO_PROVIDERS } from "../lib/demoProviders";

/** Sort: highest primary used% first, then alphabetical by name. */
function sortProviders(
  list: ProviderUsageSnapshot[],
): ProviderUsageSnapshot[] {
  return [...list].sort((a, b) => {
    const diff = b.primary.usedPercent - a.primary.usedPercent;
    if (Math.abs(diff) > 0.01) return diff;
    return a.displayName.localeCompare(b.displayName);
  });
}

/**
 * Pop-out window — dashboard shows the full card stack; provider deep-links
 * show the selected provider only so the target is unambiguous.
 */
export default function PopOutPanel({
  state,
  providerId,
}: {
  state: BootstrapState;
  providerId?: string;
}) {
  const {
    providers: realProviders,
    isRefreshing,
    refresh,
    lastRefresh,
    hasCachedData,
  } = useProviders();
  const providers = DEMO_ENABLED ? DEMO_PROVIDERS : realProviders;
  const { settings } = useSettings(state.settings);
  const { updateState, checkNow, download, apply, dismiss, openRelease } =
    useUpdateState();
  const { t } = useLocale();

  const sorted = useMemo(() => {
    const ordered = sortProviders(providers);
    if (!providerId) return ordered;
    const selected = ordered.find((p) => p.providerId === providerId);
    if (!selected) return ordered;
    return [selected];
  }, [providers, providerId]);
  const cardRefs = useRef(new Map<string, HTMLDivElement>());
  const errorCount = useMemo(
    () => sorted.filter((p) => p.error !== null).length,
    [sorted],
  );

  useEffect(() => {
    const win = getCurrentWindow();
    const screenWidth = window.screen.availWidth || window.innerWidth || 420;
    const screenHeight = window.screen.availHeight || window.innerHeight || 680;
    const width = Math.max(320, Math.min(420, screenWidth - 16));
    // Leave room for native borders/title bars on Windows; the body scrolls.
    const height = Math.max(320, Math.min(680, screenHeight - 88));
    const screenOrigin = window.screen as Screen & {
      availLeft?: number;
      availTop?: number;
    };
    const left = screenOrigin.availLeft ?? 0;
    const top = screenOrigin.availTop ?? 0;

    void win.setSize(new LogicalSize(width, height)).then(() =>
      win.setPosition(
        new LogicalPosition(
          left + Math.max(8, screenWidth - width - 8),
          top + 8,
        ),
      ),
    ).catch(() => {});
  }, []);

  useEffect(() => {
    if (!providerId || sorted.length === 0) return;

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
  }, [providerId, sorted]);

  const openSettings = useCallback(() => {
    openSettingsWindow("general");
  }, []);
  const goTray = useCallback(() => {
    setSurfaceMode("trayPanel", { kind: "summary" });
  }, []);
  const openAbout = useCallback(() => {
    openSettingsWindow("about");
  }, []);
  const quitApp = useCallback(() => {
    void quitApplication();
  }, []);

  const headerActions = [
    { icon: "⊟", title: t("TooltipBackToTray"), onClick: goTray },
  ];

  const footerRows: MenuFooterRow[] = [
    { icon: "⚙", label: t("TooltipSettings"), shortcut: "Ctrl+,", onClick: openSettings },
    { icon: "ℹ", label: "About CodexBar", onClick: openAbout },
    { icon: "✕", label: "Quit", shortcut: "Ctrl+Q", onClick: quitApp },
  ];

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

  if (sorted.length === 0) {
    return (
      <MenuSurface
        variant="popout"
        onRefresh={refresh}
        isRefreshing={isRefreshing}
        actions={headerActions}
        banner={banner}
        footerRows={footerRows}
      >
        <MenuEmpty
          isLoading={isRefreshing && !hasCachedData}
          onSettings={openSettings}
        />
      </MenuSurface>
    );
  }

  return (
    <MenuSurface
      variant="popout"
      onRefresh={refresh}
      isRefreshing={isRefreshing}
      actions={headerActions}
      banner={banner}
      footerRows={footerRows}
      summary={
        <MenuSummary
          total={sorted.length}
          errorCount={errorCount}
          isRefreshing={isRefreshing}
          lastRefresh={lastRefresh}
        />
      }
    >
      <div className="menu-stack">
        {sorted.map((p, idx) => (
          <div
            key={p.providerId}
            className="menu-stack__item"
            data-deeplinked={p.providerId === providerId || undefined}
            ref={(node) => {
              if (node) {
                cardRefs.current.set(p.providerId, node);
              } else {
                cardRefs.current.delete(p.providerId);
              }
            }}
          >
            {idx > 0 && <div className="menu-stack__sep" />}
            <MenuCard
              provider={p}
              hideEmail={settings.hidePersonalInfo}
              resetTimeRelative={settings.resetTimeRelative}
            />
          </div>
        ))}
      </div>
    </MenuSurface>
  );
}
