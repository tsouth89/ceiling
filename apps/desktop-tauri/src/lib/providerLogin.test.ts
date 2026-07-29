import { describe, expect, it } from "vitest";
import { providerLoginPhaseKey } from "./providerLogin";

describe("providerLoginPhaseKey", () => {
  it("maps active and terminal phases to localized status keys", () => {
    expect(providerLoginPhaseKey("requesting")).toBe("LoginPhaseRequesting");
    expect(providerLoginPhaseKey("waitingBrowser")).toBe(
      "LoginPhaseWaitingBrowser",
    );
    expect(providerLoginPhaseKey("complete")).toBe("LoginPhaseComplete");
    expect(providerLoginPhaseKey("failed")).toBe("LoginPhaseFailed");
  });

  it("keeps idle and absent phases quiet", () => {
    expect(providerLoginPhaseKey("idle")).toBeNull();
    expect(providerLoginPhaseKey(null)).toBeNull();
  });
});
