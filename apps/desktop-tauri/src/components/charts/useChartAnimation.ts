import { useEffect, useState } from "react";

/**
 * Tracks only the lifecycle of a chart entrance animation.
 *
 * Geometry is rendered at its final size and CSS/SVG performs the animation.
 * Keeping requestAnimationFrame out of React avoids rebuilding every point in
 * every visible chart once per frame, which was especially costly when quota
 * history contained several long series.
 */
export const TOTAL_ANIMATION_MS = 520;
export const STAGGER_PER_BAR_MS = 14;
export const MAX_STAGGER_MS = 180;

export interface ChartAnimation {
  running: boolean;
  enabled: boolean;
  durationMs: number;
  delayFor: (index: number) => number;
}

export function useChartAnimation(
  count: number,
  enabled: boolean,
  deps: ReadonlyArray<unknown> = [],
): ChartAnimation {
  const prefersReduced = usePrefersReducedMotion();
  const active = enabled && !prefersReduced && count > 0;
  const [running, setRunning] = useState(active);

  useEffect(() => {
    if (!active) {
      setRunning(false);
      return;
    }

    setRunning(true);
    const maxDelay = Math.min(Math.max(0, count - 1) * STAGGER_PER_BAR_MS, MAX_STAGGER_MS);
    const timer = window.setTimeout(() => setRunning(false), TOTAL_ANIMATION_MS + maxDelay);
    return () => window.clearTimeout(timer);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [active, count, ...deps]);

  return {
    running,
    enabled: active,
    durationMs: TOTAL_ANIMATION_MS,
    delayFor: (index) => Math.min(index * STAGGER_PER_BAR_MS, MAX_STAGGER_MS),
  };
}

function usePrefersReducedMotion(): boolean {
  const [reduced, setReduced] = useState(false);

  useEffect(() => {
    if (typeof window === "undefined" || !window.matchMedia) return;
    const mq = window.matchMedia("(prefers-reduced-motion: reduce)");
    const update = () => setReduced(mq.matches);
    update();
    mq.addEventListener?.("change", update);
    return () => mq.removeEventListener?.("change", update);
  }, []);

  return reduced;
}
