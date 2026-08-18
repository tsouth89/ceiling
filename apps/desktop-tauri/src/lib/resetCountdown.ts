/**
 * Shared remaining-time breakdown for reset countdowns.
 *
 * Floor one total of minutes, clamp a still-future sub-minute remainder
 * to 1 so the last 59s never read as "0m", and cut days at 1440 minutes
 * so 24h 0m is "1d 0h" rather than "24h 0m". Same rule as Rust
 * `format_remaining_countdown` (SBS-927 / SBS-619).
 */
export function remainingCountdownParts(
  diffMs: number,
): { days: number; hours: number; minutes: number } | null {
  if (!Number.isFinite(diffMs) || diffMs <= 0) return null;
  const totalMinutes = Math.max(1, Math.floor(diffMs / 60_000));
  return {
    days: Math.floor(totalMinutes / 1440),
    hours: Math.floor((totalMinutes % 1440) / 60),
    minutes: totalMinutes % 60,
  };
}

/** Compact two-unit form used by the flyout, matching tray/CLI/tooltip. */
export function formatResetCountdown(diffMs: number): string | null {
  const parts = remainingCountdownParts(diffMs);
  if (!parts) return null;
  if (parts.days > 0) return `${parts.days}d ${parts.hours}h`;
  if (parts.hours > 0) return `${parts.hours}h ${parts.minutes}m`;
  return `${parts.minutes}m`;
}
