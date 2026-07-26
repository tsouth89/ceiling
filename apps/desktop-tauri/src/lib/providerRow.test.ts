import { describe, expect, it } from "vitest";
import type { ProviderUsageSnapshot } from "../types/bridge";
import {
  hasMultipleAccounts,
  orderFlyoutProviders,
  representativeForProvider,
  providerIdFromRowKey,
  providerRowKey,
  rowKeyIsProvider,
  selectStripAccount,
} from "./providerRow";

const row = (providerId: string, accountId?: string) =>
  ({ providerId, accountId: accountId ?? null }) as never;

const snap = (
  providerId: string,
  accountId: string | null,
  used: number,
  // Second lane, e.g. Claude's weekly beside its 5h session.
  secondaryUsed?: number,
) =>
  ({
    providerId,
    accountId,
    primary: { usedPercent: used },
    secondary:
      secondaryUsed === undefined ? null : { usedPercent: secondaryUsed },
  }) as unknown as ProviderUsageSnapshot;

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

describe("representativeForProvider", () => {
  it("picks the most-constrained account", () => {
    const rows = [
      snap("codex", "acct-personal", 12),
      snap("codex", "acct-work", 91),
      snap("claude", "acct-c", 99),
    ];

    // The seat about to run out is the one worth summarising.
    expect(representativeForProvider(rows, "codex")?.accountId).toBe("acct-work");
  });

  it("is stable across refreshes when usage ties", () => {
    const a = [snap("codex", "acct-b", 50), snap("codex", "acct-a", 50)];
    const b = [snap("codex", "acct-a", 50), snap("codex", "acct-b", 50)];

    // Order of arrival must not change what is shown, or the row flickers.
    expect(representativeForProvider(a, "codex")?.accountId).toBe(
      representativeForProvider(b, "codex")?.accountId,
    );
  });

  it("returns null when the provider has no reading", () => {
    expect(representativeForProvider([], "codex")).toBeNull();
    expect(
      representativeForProvider([snap("claude", null, 10)], "codex"),
    ).toBeNull();
  });
});

describe("selectStripAccount", () => {
  it("defaults to the hottest account", () => {
    const rows = [
      snap("codex", "personal", 20),
      snap("codex", "work", 80),
    ];
    expect(selectStripAccount(rows)?.accountId).toBe("work");
  });

  it("honors an explicit pin when present", () => {
    const rows = [
      snap("codex", "personal", 20),
      snap("codex", "work", 80),
    ];
    expect(selectStripAccount(rows, "personal")?.accountId).toBe("personal");
  });

  it("falls back to hottest when the pin is missing from cache", () => {
    const rows = [
      snap("codex", "personal", 20),
      snap("codex", "work", 80),
    ];
    expect(selectStripAccount(rows, "gone")?.accountId).toBe("work");
  });

  it("ranks on the constraining window, not the primary one", () => {
    // The strip tile shows the constraining window, so ranking seats by their
    // primary made the flyout badge "On strip" the account the tile was not
    // showing: a maxed weekly outranks a freshly reset 5h session.
    const rows = [
      snap("claude", "maxed-weekly", 0, 100),
      snap("claude", "busy-session", 40, 0),
    ];
    expect(selectStripAccount(rows)?.accountId).toBe("maxed-weekly");
  });

  it("breaks ties on the lowest account id, as the native strip does", () => {
    const forward = [snap("codex", "a", 50), snap("codex", "z", 50)];
    const reversed = [snap("codex", "z", 50), snap("codex", "a", 50)];

    expect(selectStripAccount(forward)?.accountId).toBe("a");
    expect(selectStripAccount(reversed)?.accountId).toBe("a");
  });
});

describe("orderFlyoutProviders", () => {
  it("puts the strip account first within a multi-account provider", () => {
    const rows = [
      snap("codex", "work", 80),
      snap("codex", "personal", 20),
      snap("claude", "only", 10),
    ];
    const ordered = orderFlyoutProviders(rows, ["codex", "claude"], {
      codex: "personal",
    });
    expect(ordered.map((row) => row.accountId)).toEqual([
      "personal",
      "work",
      "only",
    ]);
  });
});
