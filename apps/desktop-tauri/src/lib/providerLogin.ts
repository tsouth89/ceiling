import type { LocaleKey } from "../i18n/keys";

export type ProviderLoginPhase =
  | "idle"
  | "requesting"
  | "waitingBrowser"
  | "complete"
  | "failed";

export type ProviderLoginPhaseChangedPayload = {
  providerId: string;
  phase: ProviderLoginPhase;
  /** The device-flow user code to display, when the phase has one to show. */
  code?: string;
};

export function providerLoginPhaseKey(
  phase: ProviderLoginPhase | null,
): LocaleKey | null {
  switch (phase) {
    case "requesting":
      return "LoginPhaseRequesting";
    case "waitingBrowser":
      return "LoginPhaseWaitingBrowser";
    case "complete":
      return "LoginPhaseComplete";
    case "failed":
      return "LoginPhaseFailed";
    case "idle":
    case null:
      return null;
  }
}
