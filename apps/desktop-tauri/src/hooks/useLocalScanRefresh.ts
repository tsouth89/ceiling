import { useEffect, useState } from "react";
import { listen } from "@tauri-apps/api/event";

/**
 * A counter that ticks when a local-transcript scan finishes refreshing.
 *
 * The API-value and heatmap cards read gigabytes of local logs, so a cached
 * answer up to five minutes old is served immediately and the rescan runs
 * behind it. Without this an open card would keep showing the stale figures
 * until it was remounted. Depend on the counter to refetch once the fresh
 * numbers exist.
 *
 * `scan` names which cache to follow, so the heatmap does not refetch because
 * the API-value card rescanned.
 */
export function useLocalScanRefresh(scan: "api-value" | "activity-heatmap"): number {
  const [refreshes, setRefreshes] = useState(0);

  useEffect(() => {
    let stop: (() => void) | null = null;
    let unmounted = false;

    listen<string>("local-scan-refreshed", (event) => {
      if (event.payload === scan) setRefreshes((count) => count + 1);
    })
      .then((unlisten) => {
        // Unmounting before the listener resolved: drop it straight away.
        if (unmounted) unlisten();
        else stop = unlisten;
      })
      // No Tauri host to listen to (tests, a plain browser). The card still
      // works, it just never learns about a background refresh.
      .catch(() => {});

    return () => {
      unmounted = true;
      stop?.();
    };
  }, [scan]);

  return refreshes;
}
