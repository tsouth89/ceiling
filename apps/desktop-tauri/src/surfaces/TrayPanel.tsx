import { Fragment, useCallback, useEffect, useMemo, useRef, useState } from "react";
import { getCurrentWindow, LogicalSize } from "@tauri-apps/api/window";
import type { BootstrapState, ProviderUsageSnapshot } from "../types/bridge";
import { setSurfaceMode, openSettingsWindow, quitApp as quitApplication } from "../lib/tauri";
import { getWorkAreaRect, reanchorTrayPanel } from "../lib/tauri";
import { useProviders } from "../hooks/useProviders";
import { useSettings } from "../hooks/useSettings";
import { useUpdateState } from "../hooks/useUpdateState";
import { useLocale } from "../hooks/useLocale";
import { useSurfaceTarget } from "../hooks/useSurfaceMode";
import MenuCard from "../components/MenuCard";
import MenuSurface, {
  MenuEmpty,
  type MenuFooterRow,
} from "../components/MenuSurface";
import UpdateBanner from "../components/UpdateBanner";
import ProviderGrid, { prioritizeProviders } from "../components/ProviderGrid";
import { openProviderDashboard, openProviderStatusPage } from "../lib/tauri";
import { DEMO_ENABLED, DEMO_PROVIDERS } from "../lib/demoProviders";

/** Provider IDs that have a dashboard URL in the backend */
const HAS_DASHBOARD = new Set([
  "abacus", "alibaba", "alibabatokenplan", "amp", "augment",
  "azureopenai", "bedrock", "claude", "codex", "codebuff",
  "commandcode", "copilot", "crof", "cursor", "deepgram", "deepseek",
  "doubao", "elevenlabs", "factory", "gemini", "grok", "groq",
  "infini", "jetbrains", "kilo", "kimi", "kimik2", "kiro", "manus",
  "mimo", "minimax", "mistral", "nanogpt", "ollama", "openaiapi",
  "opencode", "opencodego", "openrouter", "perplexity", "stepfun",
  "synthetic", "t3chat", "venice", "vertexai", "warp", "windsurf",
  "zai",
]);
/** Provider IDs that have a status page URL in the backend */
const HAS_STATUS_PAGE = new Set([
  "alibabatokenplan", "amp", "augment", "azureopenai", "bedrock",
  "claude", "codex", "copilot", "deepgram", "deepseek", "elevenlabs",
  "gemini", "grok", "groq", "kiro", "mistral", "openaiapi",
  "openrouter", "vertexai", "windsurf",
]);

function getProviderStatus(
  p: ProviderUsageSnapshot,
): "ok" | "warning" | "exhausted" | "error" {
  if (p.error) return "error";
  if (p.primary.isExhausted) return "exhausted";
  if (p.primary.usedPercent > 80) return "warning";
  return "ok";
}
void getProviderStatus;

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
 * Tray popover surface — two modes like macOS CodexBar:
 * 1. Overview (default): provider grid + all cards stacked
 * 2. Detail: click a provider in grid → show only that provider's card
 */
export default function TrayPanel({ state }: { state: BootstrapState }) {
  const {
    providers: realProviders,
    isRefreshing,
    refresh,
    hasCachedData,
  } = useProviders();
  const providers = DEMO_ENABLED ? DEMO_PROVIDERS : realProviders;
  const { settings } = useSettings(state.settings);
  const { updateState, checkNow, download, apply, dismiss, openRelease } =
    useUpdateState();
  const { t } = useLocale();
  const surfaceTarget = useSurfaceTarget("trayPanel");

  const sorted = useMemo(() => sortProviders(providers), [providers]);
  const initialProviderId =
    surfaceTarget?.kind === "provider" ? surfaceTarget.providerId : null;

  // null = overview (all providers), string = single provider detail
  const [selectedProviderId, setSelectedProviderId] = useState<string | null>(
    initialProviderId,
  );
  const [gridExpanded, setGridExpanded] = useState(false);

  useEffect(() => {
    setSelectedProviderId(initialProviderId);
  }, [initialProviderId]);

  // Hide panel during the initial resize+reposition dance so the user
  // doesn't see the window jump around.  Revealed after first layout pass.
  const [layoutReady, setLayoutReady] = useState(false);
  const layoutReadyRef = useRef(false);
  const resizeRunRef = useRef(0);

  // Cards to display based on mode
  // Overview: all providers in the grid — non-error first, then errors
  // Detail: only the selected provider's card (macOS shows single provider)
  const visibleProviders = useMemo(() => {
    if (selectedProviderId === null) {
      if (DEMO_ENABLED) {
        return ["codex", "claude"]
          .map((id) => providers.find((p) => p.providerId === id))
          .filter((p): p is ProviderUsageSnapshot => p !== undefined);
      }
      // Overview: show all providers (they have data, email, or error), non-error first
      if (sorted.length + 1 > 32 && !gridExpanded) {
        return prioritizeProviders(sorted, null).slice(0, 4);
      }
      const normal = sorted.filter((p) => !p.error);
      const errors = sorted.filter((p) => !!p.error);
      return [...normal, ...errors];
    }
    // Detail: show ONLY the selected provider (macOS behavior — no appended errors)
    const match = sorted.find((p) => p.providerId === selectedProviderId);
    if (!match) {
      const normal = sorted.filter((p) => !p.error);
      const errors = sorted.filter((p) => !!p.error);
      return [...normal, ...errors];
    }
    return [match];
  }, [sorted, selectedProviderId, gridExpanded]);

  // Dynamically size the Tauri window to fit content, capped at 800px.
  // The first pass can grow the hidden window for a complete measurement.
  // Later content updates measure in-place so the visible panel does not
  // bounce to max height and back while providers finish refreshing.
  useEffect(() => {
    const TRAY_WIDTH = 310;
    const MAX_MEASURE_HEIGHT = 920;
    const MIN_HEIGHT = 200;

    const resize = async () => {
      const run = ++resizeRunRef.current;
      const win = getCurrentWindow();
      const surface = document.querySelector<HTMLElement>(".menu-surface--tray");
      if (!surface) return;
      const html = document.documentElement;
      const pageBody = document.body;
      const workArea = await getWorkAreaRect().catch(() => null);
      const maxHeight = Math.max(
        MIN_HEIGHT,
        Math.min(MAX_MEASURE_HEIGHT, (workArea?.height ?? MAX_MEASURE_HEIGHT) - 16),
      );

      const body = surface.querySelector<HTMLElement>(".menu-surface__body");
      const stack = surface.querySelector<HTMLElement>(".menu-stack");

      const previous = {
        htmlOverflow: html.style.overflow,
        bodyOverflow: pageBody.style.overflow,
        bodyMinHeight: pageBody.style.minHeight,
        surfaceMaxHeight: surface.style.maxHeight,
        surfaceOverflow: surface.style.overflow,
        bodyInnerOverflow: body?.style.overflow,
        bodyFlex: body?.style.flex,
        stackOverflow: stack?.style.overflow,
      };
      let committedHeight = false;

      html.style.overflow = "visible";
      pageBody.style.overflow = "visible";
      pageBody.style.minHeight = "0";
      surface.style.maxHeight = "none";
      surface.style.overflow = "visible";
      if (body) { body.style.overflow = "visible"; body.style.flex = "0 0 auto"; }
      if (stack) { stack.style.overflow = "visible"; }

      const revealPanel = () => {
        if (run === resizeRunRef.current) {
          layoutReadyRef.current = true;
          setLayoutReady(true);
        }
      };

      try {
        if (!layoutReadyRef.current) {
          await win.setSize(new LogicalSize(TRAY_WIDTH, maxHeight));
          for (let i = 0; i < 20; i++) {
            await new Promise<void>((r) => setTimeout(r, 50));
            if (html.clientHeight >= maxHeight - 20) break;
          }
        }

        await new Promise<void>((r) => requestAnimationFrame(() => r()));
        await new Promise<void>((r) => requestAnimationFrame(() => r()));

        if (run !== resizeRunRef.current) return;

        // Scan all descendants to find true content extent
        const surfaceRect = surface.getBoundingClientRect();
        let maxBottom = surfaceRect.bottom;
        for (const el of surface.querySelectorAll("*")) {
          const r = (el as HTMLElement).getBoundingClientRect();
          if (r.height > 0 && r.bottom > maxBottom) maxBottom = r.bottom;
        }

        // Also check the footer explicitly — it may lay out below the
        // surface border-box when body flex overflows the auto-height parent.
        const footer = surface.querySelector<HTMLElement>(".menu-surface__footer");
        const footerRect = footer?.getBoundingClientRect();
        if (footerRect && footerRect.height > 0 && footerRect.bottom > maxBottom) {
          maxBottom = footerRect.bottom;
        }

        const contentHeight = Math.ceil(maxBottom - surfaceRect.top) + 4;
        const height = Math.min(Math.max(contentHeight, MIN_HEIGHT), maxHeight);

        // Lock surface to measured content height.
        surface.style.maxHeight = `${height}px`;
        committedHeight = true;

        await win.setSize(new LogicalSize(TRAY_WIDTH, height));
        await reanchorTrayPanel().catch(() => {});

        // First layout pass complete — reveal the panel.
        revealPanel();
      } catch (error) {
        console.warn("CodexBar tray panel resize failed", error);
        // If Windows refuses a transient resize/reanchor request, prefer a
        // visible slightly-imperfect panel over an unusable invisible one.
        revealPanel();
      } finally {
        if (!committedHeight) {
          surface.style.maxHeight = previous.surfaceMaxHeight;
        }
        surface.style.overflow = previous.surfaceOverflow;
        html.style.overflow = previous.htmlOverflow;
        pageBody.style.overflow = previous.bodyOverflow;
        pageBody.style.minHeight = previous.bodyMinHeight;
        if (body) {
          body.style.overflow = previous.bodyInnerOverflow ?? "";
          body.style.flex = previous.bodyFlex ?? "";
        }
        if (stack) {
          stack.style.overflow = previous.stackOverflow ?? "";
        }
      }
    };

    const t0 = setTimeout(() => void resize(), layoutReadyRef.current ? 50 : 100);

    return () => {
      clearTimeout(t0);
      resizeRunRef.current += 1;
    };
  }, [visibleProviders, providers]);

  const openSettings = useCallback(() => {
    void openSettingsWindow("general").finally(() => {
      void getCurrentWindow().close();
    });
  }, []);
  const openPopOut = useCallback(() => {
    setSurfaceMode("popOut", { kind: "dashboard" });
  }, []);
  const openAbout = useCallback(() => {
    void openSettingsWindow("about").finally(() => {
      void getCurrentWindow().close();
    });
  }, []);
  const quitApp = useCallback(() => {
    void quitApplication();
  }, []);

  const headerActions = [
    { icon: "⧉", title: t("TooltipPopOut"), onClick: openPopOut },
  ];

  const footerRows: MenuFooterRow[] = [
    { icon: "↻", label: "Refresh", shortcut: "Ctrl+R", onClick: refresh },
    { icon: "⚙", label: "Settings\u2026", shortcut: "Ctrl+,", onClick: openSettings },
    { icon: "ⓘ", label: "About CodexBar", onClick: openAbout },
    { icon: "⌧", label: "Quit", shortcut: "Ctrl+Q", onClick: quitApp },
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

  const handleGridClick = useCallback(
    (providerId: string | null) => {
      setSelectedProviderId(providerId);
    },
    [],
  );
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
      <div className={`tray-panel-reveal${layoutReady ? " tray-panel-reveal--ready" : ""}`}>
      <MenuSurface
        variant="tray"
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
      </div>
    );
  }

  return (
    <div className={`tray-panel-reveal${layoutReady ? " tray-panel-reveal--ready" : ""}`}>
    <MenuSurface
      variant="tray"
      onRefresh={refresh}
      isRefreshing={isRefreshing}
      actions={headerActions}
      banner={banner}
      footerRows={footerRows}
    >
      <ProviderGrid
        providers={providers}
        selectedProviderId={selectedProviderId}
        showAsUsed={settings.showAsUsed}
        expanded={gridExpanded}
        onExpandedChange={setGridExpanded}
        onSelect={handleGridClick}
      />
      <div className="provider-grid__divider" />
      <div className="menu-stack">
        {visibleProviders.map((p, idx) => {
          const isSelected =
            selectedProviderId !== null && p.providerId === selectedProviderId;
          return (
            <Fragment key={p.providerId}>
              {idx > 0 && <div className="menu-stack__sep" />}
              <div
                className={`menu-stack__item${isSelected ? " menu-stack__item--selected" : ""}`}
                id={`card-${p.providerId}`}
              >
                <MenuCard
                  provider={p}
                  hideEmail={settings.hidePersonalInfo}
                  resetTimeRelative={settings.resetTimeRelative}
                  showAsUsed={settings.showAsUsed}
                  compactMetrics={selectedProviderId === null}
                />
              </div>
            </Fragment>
          );
        })}
      </div>
      {/* Context actions — detail mode only, matches macOS actionsSection */}
      {selectedProviderId && (HAS_DASHBOARD.has(selectedProviderId) || HAS_STATUS_PAGE.has(selectedProviderId)) && (
        <div className="context-actions">
          <div className="context-actions__divider" />
          {HAS_DASHBOARD.has(selectedProviderId) && (
            <button
              type="button"
              className="context-actions__btn"
              onClick={() => void openProviderDashboard(selectedProviderId)}
            >
              <span className="context-actions__icon" aria-hidden>
                <svg width="13" height="13" viewBox="0 0 16 16" fill="none" xmlns="http://www.w3.org/2000/svg">
                  <rect x="2" y="9" width="2.5" height="5" rx="0.6" fill="currentColor" />
                  <rect x="6.75" y="6" width="2.5" height="8" rx="0.6" fill="currentColor" />
                  <rect x="11.5" y="3" width="2.5" height="11" rx="0.6" fill="currentColor" />
                </svg>
              </span>
              Usage Dashboard
            </button>
          )}
          {HAS_STATUS_PAGE.has(selectedProviderId) && (
            <button
              type="button"
              className="context-actions__btn"
              onClick={() => void openProviderStatusPage(selectedProviderId)}
            >
              <span className="context-actions__icon" aria-hidden>
                <svg width="14" height="13" viewBox="0 0 18 14" fill="none" xmlns="http://www.w3.org/2000/svg">
                  <path d="M1 7H4L5.5 3L8 11L10.5 5L12 7H17" stroke="currentColor" strokeWidth="1.4" strokeLinecap="round" strokeLinejoin="round" fill="none" />
                </svg>
              </span>
              Status Page
            </button>
          )}
        </div>
      )}
    </MenuSurface>
    </div>
  );
}
