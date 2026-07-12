import { useEffect, useState } from "react";
import { useLocale } from "../../hooks/useLocale";

/**
 * Parse an RFC-3339 / ISO-8601 timestamp and return a live-updating
 * "Resets in 3h 42m" style countdown string. Refreshes every 30s.
 *
 * Falls back to `fallback` (typically the backend-provided `resetDescription`)
 * when `resetsAt` is absent or unparseable.
 *
 * Matches the egui pop-out countdown format, which the Rust locale exposes
 * as `ResetsInDaysHours` and `ResetsInHoursMinutes`.
 */
export function useResetCountdown(
  resetsAt: string | null,
  fallback: string | null,
): string | null {
  const { t } = useLocale();
  const [now, setNow] = useState(() => Date.now());

  useEffect(() => {
    if (!resetsAt) return;
    const id = window.setInterval(() => setNow(Date.now()), 30_000);
    return () => window.clearInterval(id);
  }, [resetsAt]);

  if (!resetsAt) return fallback;
  const target = Date.parse(resetsAt);
  if (Number.isNaN(target)) return fallback;

  const diffMs = target - now;
  if (diffMs <= 0) return t("TrayResetsDueNow");

  const totalMinutes = Math.floor(diffMs / 60_000);
  const days = Math.floor(totalMinutes / 1440);
  const hours = Math.floor((totalMinutes % 1440) / 60);
  const minutes = totalMinutes % 60;

  if (days > 0) {
    return t("ResetsInDaysHours")
      .replace("{}", String(days))
      .replace("{}", String(hours));
  }
  if (hours === 0) {
    return t("ResetsInMinutes").replace("{}", String(minutes));
  }
  return t("ResetsInHoursMinutes")
    .replace("{}", String(hours))
    .replace("{}", String(minutes));
}
