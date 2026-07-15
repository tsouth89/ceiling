import { useCallback, useEffect, useRef, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import type { SettingsSnapshot, SettingsUpdate } from "../types/bridge";
import { getSettingsSnapshot, updateSettings } from "../lib/tauri";

interface UseSettingsReturn {
  settings: SettingsSnapshot;
  saving: boolean;
  error: string | null;
  update: (patch: SettingsUpdate) => Promise<void>;
}

/**
 * Manages the current settings state and exposes a mutation helper that
 * persists changes through the Tauri bridge and refreshes the local copy.
 */
export function useSettings(initial: SettingsSnapshot): UseSettingsReturn {
  const [settings, setSettings] = useState<SettingsSnapshot>(initial);
  const [saving, setSaving] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const updateQueue = useRef<Promise<void>>(Promise.resolve());
  const pendingUpdates = useRef(0);

  useEffect(() => {
    let cancelled = false;

    setSettings(initial);

    getSettingsSnapshot()
      .then((fresh) => {
        if (!cancelled) {
          setSettings(fresh);
        }
      })
      .catch(() => {
        // Keep the bootstrap snapshot if the background sync fails.
      });

    return () => {
      cancelled = true;
    };
  }, [initial]);

  // Live-sync when settings change in ANOTHER window. The detached Settings
  // window and the main/PopOut window are separate webviews with separate
  // React state, so the in-window CustomEvent below never reaches them. Rust
  // broadcasts "settings-changed" after every persisted update; re-fetch the
  // snapshot so this surface (e.g. the PopOut window scale) re-renders live.
  useEffect(() => {
    let active = true;
    let unlisten: (() => void) | undefined;
    // `Promise.resolve` tolerates test mocks that return a bare unlisten fn (or
    // undefined) instead of a promise; the `active` flag handles unmounting
    // before the listener finishes registering.
    Promise.resolve(
      listen("settings-changed", () => {
        getSettingsSnapshot()
          .then((fresh) => setSettings(fresh))
          .catch(() => {
            // Keep the current copy if the refresh fails.
          });
      }),
    )
      .then((fn) => {
        if (active) {
          unlisten = fn;
        } else {
          fn?.();
        }
      })
      .catch(() => {});
    return () => {
      active = false;
      unlisten?.();
    };
  }, []);

  const update = useCallback((patch: SettingsUpdate): Promise<void> => {
    pendingUpdates.current += 1;
    setSaving(true);
    const run = updateQueue.current.then(async () => {
      setError(null);
      try {
        const next = await updateSettings(patch);
        setSettings(next);
        if (typeof window !== "undefined") {
          window.dispatchEvent(
            new CustomEvent<SettingsSnapshot>("codexbar:settings-updated", {
              detail: next,
            }),
          );
        }
      } catch (err: unknown) {
        const msg = err instanceof Error ? err.message : String(err);
        setError(msg);
        // Re-fetch to stay in sync with disk state on failure.
        try {
          const fresh = await getSettingsSnapshot();
          setSettings(fresh);
        } catch {
          // Ignore a secondary failure and retain the current snapshot.
        }
      } finally {
        pendingUpdates.current -= 1;
        if (pendingUpdates.current === 0) setSaving(false);
      }
    });

    // Keep the queue usable even if a future implementation lets `run` reject.
    updateQueue.current = run.catch(() => {});
    return run;
  }, []);

  return { settings, saving, error, update };
}
