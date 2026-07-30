import { beforeEach, describe, expect, it, vi } from "vitest";

// vi.mock is hoisted above module scope, so the spy has to be hoisted with it.
const { invoke } = vi.hoisted(() => ({
  invoke: vi.fn(() => Promise.resolve({})),
}));
vi.mock("@tauri-apps/api/core", () => ({ invoke }));

import { getProviderDetail } from "./tauri";

/**
 * A command wrapper that accepts an argument and then forgets to send it fails
 * silently: the call succeeds, the backend applies its default, and the UI
 * looks inert. `getProviderDetail` shipped that way, so switching accounts on
 * the Providers page did nothing at all - every click re-fetched whichever
 * account the backend picks on its own.
 */
describe("getProviderDetail argument passing", () => {
  beforeEach(() => invoke.mockClear());

  it("sends the selected accountId to the backend", () => {
    void getProviderDetail("codex", "98942f4c-e615-47d8-b915-d6580f29be0a");

    expect(invoke).toHaveBeenCalledWith("get_provider_detail", {
      providerId: "codex",
      accountId: "98942f4c-e615-47d8-b915-d6580f29be0a",
    });
  });

  it("still sends the key explicitly when no account is chosen", () => {
    // Present-and-null is what tells the backend to choose for itself; an
    // absent key would be indistinguishable from the bug being reintroduced.
    void getProviderDetail("claude");

    expect(invoke).toHaveBeenCalledWith("get_provider_detail", {
      providerId: "claude",
      accountId: null,
    });
  });
});
