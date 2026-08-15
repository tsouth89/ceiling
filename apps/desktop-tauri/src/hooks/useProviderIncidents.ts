import { useEffect, useState } from "react";
import { getProviderIncidents } from "../lib/tauri";
import type { ProviderIncident } from "../types/bridge";

/**
 * Providers currently reporting a status-page incident (SBS-280).
 *
 * Every provider card wants this map, so the fetch is shared at module scope
 * rather than fired once per card. The backend already caches each status page
 * for fifteen minutes; this second layer only stops a grid of eight cards from
 * making eight identical IPC calls the moment it mounts.
 *
 * An empty map is the answer whenever the feature is off, every provider is
 * operational, or a status page could not be read. The badge is never a guess.
 */

export type IncidentMap = Record<string, ProviderIncident>;

/** Matches the backend's own reading TTL, so the UI never re-asks sooner. */
const REFRESH_MS = 15 * 60 * 1000;

/**
 * Matches the backend's error backoff.
 *
 * An empty map is ambiguous: it means "nothing is wrong" most of the time, but
 * it is also what comes back when every status page failed its first read and
 * there was no earlier answer to carry forward. Holding that for the full
 * quarter hour would sit on a first-open timeout long past the point the
 * backend itself is willing to retry, so an empty answer ages out on the short
 * window instead.
 */
const EMPTY_REFRESH_MS = 5 * 60 * 1000;

function refreshWindowMs(): number {
  return Object.keys(cached).length === 0 ? EMPTY_REFRESH_MS : REFRESH_MS;
}

function isStale(): boolean {
  return Date.now() - loadedAt >= refreshWindowMs();
}

let cached: IncidentMap = {};
let loadedAt = 0;
let inFlight: Promise<IncidentMap> | null = null;
const listeners = new Set<(map: IncidentMap) => void>();

function load(): Promise<IncidentMap> {
  if (inFlight) return inFlight;
  inFlight = getProviderIncidents()
    .then((map) => {
      cached = map ?? {};
      loadedAt = Date.now();
      for (const listener of listeners) listener(cached);
      return cached;
    })
    .catch(() => {
      // A failed poll leaves the previous answer in place. Clearing it would
      // make a badge flicker away while the incident is still live.
      //
      // loadedAt is deliberately not advanced: a first-open timeout would
      // otherwise serve an empty map for the full refresh window, well past
      // the backend's own five-minute error backoff.
      return cached;
    })
    .finally(() => {
      inFlight = null;
    });
  return inFlight;
}

/** Drop the shared reading. Exported for tests and for settings changes. */
export function resetProviderIncidentsCache() {
  cached = {};
  loadedAt = 0;
  inFlight = null;
}

export function useProviderIncidents(enabled: boolean): IncidentMap {
  const [incidents, setIncidents] = useState<IncidentMap>(cached);

  useEffect(() => {
    if (!enabled) {
      setIncidents({});
      return;
    }
    let live = true;
    const listener = (map: IncidentMap) => {
      if (live) setIncidents(map);
    };
    listeners.add(listener);
    if (isStale()) {
      void load();
    } else {
      setIncidents(cached);
    }
    // Ticks on the short window and lets `isStale` decide, so a held incident
    // still waits the full TTL while an empty answer re-asks sooner.
    const timer = window.setInterval(() => {
      if (isStale()) void load();
    }, EMPTY_REFRESH_MS);
    return () => {
      live = false;
      listeners.delete(listener);
      window.clearInterval(timer);
    };
  }, [enabled]);

  return enabled ? incidents : {};
}
