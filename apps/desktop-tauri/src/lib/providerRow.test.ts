import { describe, expect, it } from "vitest";
import {
  hasMultipleAccounts,
  providerIdFromRowKey,
  providerRowKey,
  rowKeyIsProvider,
} from "./providerRow";

const row = (providerId: string, accountId?: string) =>
  ({ providerId, accountId: accountId ?? null }) as never;

describe("providerRowKey", () => {
  it("separates two accounts on one provider", () => {
    const personal = providerRowKey(row("codex", "acct-personal"));
    const work = providerRowKey(row("codex", "acct-work"));

    // Collapsing these is what made a second account replace the first.
    expect(personal).not.toEqual(work);
  });

  it("is just the provider id while following the CLI", () => {
    // No accounts configured: nothing changes for the majority of users.
    expect(providerRowKey(row("codex"))).toBe("codex");
  });

  it("does not collide across providers", () => {
    expect(providerRowKey(row("codex", "a"))).not.toEqual(
      providerRowKey(row("claude", "a")),
    );
  });

  it("recovers the provider from a row key", () => {
    expect(providerIdFromRowKey(providerRowKey(row("codex", "acct")))).toBe(
      "codex",
    );
    expect(providerIdFromRowKey(providerRowKey(row("codex")))).toBe("codex");
    expect(rowKeyIsProvider(providerRowKey(row("codex", "acct")), "codex")).toBe(
      true,
    );
    expect(rowKeyIsProvider(providerRowKey(row("codex", "acct")), "claude")).toBe(
      false,
    );
  });
});

describe("hasMultipleAccounts", () => {
  it("is false for a single account, so its name stays hidden", () => {
    const providers = [row("codex", "acct"), row("claude", "other")];

    expect(hasMultipleAccounts(providers, "codex")).toBe(false);
  });

  it("is true once a provider has two rows", () => {
    const providers = [row("codex", "a"), row("codex", "b"), row("claude")];

    expect(hasMultipleAccounts(providers, "codex")).toBe(true);
    expect(hasMultipleAccounts(providers, "claude")).toBe(false);
  });
});
