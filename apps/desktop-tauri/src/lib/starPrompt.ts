import type { ProviderUsageSnapshot } from "../types/bridge";

/**
 * One-off GitHub star prompt (SOU-311).
 *
 * The whole point of this file is restraint. An ask that shows up twice and
 * then never again is a nudge; the same ask on a timer is spam, and spam from
 * a tray app that people keep open all day is worse than not asking at all.
 * The rules below are therefore all upper bounds, and every one of them fails
 * closed: an unreadable store, a missing version, a reading we are not sure
 * about — each returns "do not ask" rather than "ask anyway".
 *
 * Two asks, ever:
 *   1. The first time Ceiling shows a real reading, which is the first moment
 *      it has actually done anything for the user. Asking during onboarding
 *      would be asking before the app has earned it.
 *   2. Once after a later version has been running, and only if the first ask
 *      went unanswered. A version bump alone is not enough — see MIN_GAP_MS.
 *
 * Clicking through to GitHub ends it for good, whether or not the star
 * actually happened. Ceiling cannot see the user's GitHub identity and is not
 * going to ask for it, so intent is the only signal available and the honest
 * thing to do is treat it as final.
 */

const STORAGE_KEY = "ceiling.star-prompt.v1";

/** Hard ceiling on lifetime asks. Not a tunable. */
export const MAX_ASKS = 2;

/**
 * Floor between the two asks. Without it, a user who updates the day after
 * their first ask gets the second one immediately, which reads as nagging even
 * though the lifetime count is still honoured.
 */
export const MIN_GAP_MS = 7 * 24 * 60 * 60 * 1000;

/**
 * How long a real reading must have been on screen before the first ask. Long
 * enough that the numbers, not the prompt, are the first thing the user reads.
 */
export const SETTLE_MS = 20_000;

export type StarPromptReason = "firstValue" | "afterUpdate";

export interface StarPromptState {
  /** Lifetime count of asks actually shown. */
  asked: number;
  /** The user clicked through to GitHub. Terminal. */
  starred: boolean;
  /** App version the last ask was shown under, for the version-bump gate. */
  lastAskedVersion: string | null;
  /** Epoch ms of the last ask, for MIN_GAP_MS. */
  lastAskedAt: number | null;
  /**
   * The record exists but could not be read or trusted. Terminal, and the
   * reason a failed read cannot just return a zeroed state: a record we cannot
   * read may already say "asked twice" or "starred", and a storage layer that
   * keeps throwing would hand back that zeroed state on every launch, turning
   * a two-ask lifetime cap into an ask every time the app starts.
   */
  unreadable: boolean;
}

const EMPTY: StarPromptState = {
  asked: 0,
  starred: false,
  lastAskedVersion: null,
  lastAskedAt: null,
  unreadable: false,
};

const UNREADABLE: StarPromptState = { ...EMPTY, unreadable: true };

/** A field is acceptable when it is absent entirely, or is the type it should be. */
function optional(value: unknown, check: (v: unknown) => boolean): boolean {
  return value === undefined || value === null || check(value);
}

/**
 * Read the stored state. A missing key is the only fresh-install case;
 * anything present but unreadable fails closed rather than being repaired or
 * zeroed, because a half-written record cannot tell us whether the user has
 * already been asked or has already starred.
 */
export function readStarPromptState(): StarPromptState {
  let raw: string | null;
  try {
    raw = localStorage.getItem(STORAGE_KEY);
  } catch {
    // Storage is unavailable (private mode, disabled storage). Nothing can be
    // recorded either, so asking would repeat on every launch.
    return { ...UNREADABLE };
  }
  if (raw === null) return { ...EMPTY };

  let parsed: unknown;
  try {
    parsed = JSON.parse(raw);
  } catch {
    return { ...UNREADABLE };
  }
  if (typeof parsed !== "object" || parsed === null || Array.isArray(parsed)) {
    return { ...UNREADABLE };
  }

  const record = parsed as Record<string, unknown>;
  const { asked, starred, lastAskedVersion, lastAskedAt } = record;
  const wellFormed =
    optional(
      asked,
      (v) => typeof v === "number" && Number.isFinite(v) && v >= 0,
    ) &&
    optional(starred, (v) => typeof v === "boolean") &&
    optional(lastAskedVersion, (v) => typeof v === "string") &&
    optional(lastAskedAt, (v) => typeof v === "number" && Number.isFinite(v));
  if (!wellFormed) return { ...UNREADABLE };

  return {
    asked: typeof asked === "number" ? Math.trunc(asked) : 0,
    starred: starred === true,
    lastAskedVersion:
      typeof lastAskedVersion === "string" ? lastAskedVersion : null,
    lastAskedAt: typeof lastAskedAt === "number" ? lastAskedAt : null,
    unreadable: false,
  };
}

/** Write the record and confirm it survived. False means it did not persist. */
function writeStarPromptState(state: StarPromptState): boolean {
  const { unreadable: _unreadable, ...persisted } = state;
  try {
    localStorage.setItem(STORAGE_KEY, JSON.stringify(persisted));
  } catch {
    return false;
  }
  // Read the record back and compare rather than trust the write. A storage
  // layer that accepts a `setItem` and drops it would otherwise leave an ask
  // shown but uncounted, which is an ask that comes back on every launch.
  const back = readStarPromptState();
  return (
    !back.unreadable &&
    back.asked === persisted.asked &&
    back.starred === persisted.starred &&
    back.lastAskedVersion === persisted.lastAskedVersion &&
    back.lastAskedAt === persisted.lastAskedAt
  );
}

/**
 * Record that an ask is about to be shown. Returns false when the record did
 * not persist, in which case the caller must not show the prompt: an ask that
 * cannot be counted is an ask with no cap on it.
 */
export function recordStarPromptShown(version: string, now: number): boolean {
  const state = readStarPromptState();
  if (state.unreadable) return false;
  return writeStarPromptState({
    ...state,
    asked: state.asked + 1,
    lastAskedVersion: version,
    lastAskedAt: now,
  });
}

/** Record that the user clicked through to GitHub. Ends all future asks. */
export function recordStarPromptStarred(): void {
  const state = readStarPromptState();
  // An unreadable record already blocks every future ask, so there is nothing
  // to preserve and overwriting it would be guessing at the counts.
  if (state.unreadable) return;
  writeStarPromptState({ ...state, starred: true });
}

/**
 * Compare two `X.Y.Z` versions. Null when either side is not a plain triplet,
 * which is what a prerelease or a locally built version string looks like, and
 * which the caller treats as "do not ask".
 */
export function isLaterVersion(
  version: string,
  previous: string,
): boolean | null {
  const next = parseTriplet(version);
  const prior = parseTriplet(previous);
  if (!next || !prior) return null;
  for (let i = 0; i < 3; i += 1) {
    if (next[i] !== prior[i]) return next[i] > prior[i];
  }
  return false;
}

function parseTriplet(version: string): [number, number, number] | null {
  const match = /^(\d+)\.(\d+)\.(\d+)$/.exec(version.trim());
  if (!match) return null;
  return [Number(match[1]), Number(match[2]), Number(match[3])];
}

/**
 * True when a snapshot represents a reading the user can act on, as opposed to
 * a provider that is enabled but errored, signed out, or reporting nothing.
 *
 * A fresh account sitting at 0% used still counts: 100% remaining is a real
 * answer to "how much is left", and it is the answer most new users see first.
 * What does not count is the both-zero shape, which is what an unavailable
 * window looks like once it reaches this layer.
 */
export function hasRealReading(providers: ProviderUsageSnapshot[]): boolean {
  return providers.some((provider) => {
    if (provider.error) return false;
    const { usedPercent, remainingPercent } = provider.primary;
    if (usedPercent > 0 || remainingPercent > 0) return true;
    // A provider that reports only money (no metered window) is still a real
    // reading; Cursor on-demand and the local cost scanners land here.
    return provider.cost != null;
  });
}

export interface StarPromptInput {
  state: StarPromptState;
  /** Current app version, or null while `get_app_info` is still in flight. */
  version: string | null;
  /** Whether a real reading is on screen right now. */
  hasReading: boolean;
  /** Epoch ms at which a real reading was first seen this session. */
  readingSince: number | null;
  now: number;
}

/**
 * Decide whether to show the prompt, and why. `null` means stay quiet, which
 * is the answer for every case this function is not certain about.
 */
export function starPromptReason({
  state,
  version,
  hasReading,
  readingSince,
  now,
}: StarPromptInput): StarPromptReason | null {
  if (state.unreadable) return null;
  if (state.starred) return null;
  if (state.asked >= MAX_ASKS) return null;
  // Without a version there is no way to honour the version-bump gate on the
  // second ask, so nothing is shown rather than risk a repeat.
  if (!version) return null;
  if (!hasReading || readingSince == null) return null;
  if (now - readingSince < SETTLE_MS) return null;

  if (state.asked === 0) return "firstValue";

  // Second ask: a genuinely later version, and not hard on the heels of the
  // first. A missing timestamp on a prior ask is treated as "too soon".
  if (state.lastAskedVersion == null) return null;
  // Later by release order, not merely different: rolling back to an older
  // build is not an update, and must not spend the second ask.
  if (isLaterVersion(version, state.lastAskedVersion) !== true) return null;
  if (state.lastAskedAt == null) return null;
  if (now - state.lastAskedAt < MIN_GAP_MS) return null;
  return "afterUpdate";
}
