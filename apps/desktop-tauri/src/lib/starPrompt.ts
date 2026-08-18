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
}

const EMPTY: StarPromptState = {
  asked: 0,
  starred: false,
  lastAskedVersion: null,
  lastAskedAt: null,
};

/**
 * Read the stored state. Anything malformed is treated as a fresh install
 * rather than repaired: the only cost is one extra ask over the app's life,
 * and guessing at a half-written record risks the opposite.
 */
export function readStarPromptState(): StarPromptState {
  try {
    const raw = localStorage.getItem(STORAGE_KEY);
    if (!raw) return { ...EMPTY };
    const parsed = JSON.parse(raw) as Partial<StarPromptState>;
    return {
      asked:
        typeof parsed.asked === "number" && Number.isFinite(parsed.asked)
          ? Math.max(0, Math.trunc(parsed.asked))
          : 0,
      starred: parsed.starred === true,
      lastAskedVersion:
        typeof parsed.lastAskedVersion === "string"
          ? parsed.lastAskedVersion
          : null,
      lastAskedAt:
        typeof parsed.lastAskedAt === "number" &&
        Number.isFinite(parsed.lastAskedAt)
          ? parsed.lastAskedAt
          : null,
    };
  } catch {
    return { ...EMPTY };
  }
}

function writeStarPromptState(state: StarPromptState): void {
  try {
    localStorage.setItem(STORAGE_KEY, JSON.stringify(state));
  } catch {
    // Storage can be unavailable (private mode, disabled storage). Losing the
    // record means at most one extra ask later, which is the safe direction to
    // fail in; it is not worth surfacing to the user.
  }
}

/** Record that an ask was shown. Called once, when the toast first appears. */
export function recordStarPromptShown(version: string, now: number): void {
  const state = readStarPromptState();
  writeStarPromptState({
    ...state,
    asked: state.asked + 1,
    lastAskedVersion: version,
    lastAskedAt: now,
  });
}

/** Record that the user clicked through to GitHub. Ends all future asks. */
export function recordStarPromptStarred(): void {
  writeStarPromptState({ ...readStarPromptState(), starred: true });
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
  if (state.lastAskedVersion === version) return null;
  if (state.lastAskedAt == null) return null;
  if (now - state.lastAskedAt < MIN_GAP_MS) return null;
  return "afterUpdate";
}
