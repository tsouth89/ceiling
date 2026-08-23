import { describe, expect, it } from "vitest";
import providerSource from "../../../../rust/src/core/provider.rs?raw";
import {
  TEST_PROVIDER_CATALOG,
  liveProviderCatalogFrom,
} from "./providerCatalog";

describe("provider catalog contract (SBS-1048)", () => {
  const live = liveProviderCatalogFrom(providerSource);

  it("pins TEST_PROVIDER_CATALOG to ProviderId::all", () => {
    expect(TEST_PROVIDER_CATALOG).toEqual(live);
  });

  it("includes wayfinder with the live display name", () => {
    expect(live).toContainEqual(["wayfinder", "Wayfinder"]);
  });

  it("keeps catalog ids unique and non-empty", () => {
    const ids = live.map(([id]) => id);
    expect(ids.every(Boolean)).toBe(true);
    expect(new Set(ids).size).toBe(ids.length);
  });

  it("fails closed when ProviderId::all cannot be parsed", () => {
    expect(() => liveProviderCatalogFrom("enum ProviderId {}")).toThrow(
      /pub fn all\(/,
    );
  });
});
