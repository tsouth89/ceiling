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
      loadedAt = Date.now();
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
    if (Date.now() - loadedAt >= REFRESH_MS) {
      void load();
    } else {
      setIncidents(cached);
    }
    const timer = window.setInterval(() => void load(), REFRESH_MS);
    return () => {
      live = false;
      listeners.delete(listener);
      window.clearInterval(timer);
    };
  }, [enabled]);

  return enabled ? incidents : {};
}
