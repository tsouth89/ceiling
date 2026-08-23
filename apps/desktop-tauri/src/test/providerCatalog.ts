import providerSource from "../../../../rust/src/core/provider.rs?raw";

/**
 * Live `[cli_name, display_name]` pairs from `ProviderId::all()`, in the same
 * order `provider_catalog_for` emits. Generated from the Rust source of truth
 * so a new `ProviderId` cannot sit outside Frontend CI (SBS-1048, same class
 * as SBS-872).
 */
export function liveProviderCatalogFrom(
  src: string,
): Array<[string, string]> {
  const variants = providerIdAll(src);
  const cliNames = providerIdStringMap(src, "cli_name");
  const displayNames = providerIdStringMap(src, "display_name");
  return variants.map((variant) => {
    const id = cliNames.get(variant);
    const displayName = displayNames.get(variant);
    if (!id || !displayName) {
      throw new Error(
        `ProviderId::${variant} missing cli_name or display_name`,
      );
    }
    return [id, displayName];
  });
}

function rustMethod(src: string, name: string): string {
  const header = `pub fn ${name}(`;
  const start = src.indexOf(header);
  if (start < 0) {
    throw new Error(`${header} not found in provider.rs`);
  }
  const from = src.slice(start);
  const end = from.indexOf("\n    }\n");
  if (end < 0) {
    throw new Error(`${name} method body not closed`);
  }
  return from.slice(0, end);
}

function providerIdAll(src: string): string[] {
  const variants = [...rustMethod(src, "all").matchAll(/ProviderId::(\w+)/g)].map(
    (match) => match[1],
  );
  if (variants.length === 0) {
    throw new Error("ProviderId::all() listed no variants");
  }
  return variants;
}

function providerIdStringMap(src: string, name: string): Map<string, string> {
  const map = new Map<string, string>();
  for (const match of rustMethod(src, name).matchAll(
    /ProviderId::(\w+)\s*=>\s*"([^"]*)"/g,
  )) {
    map.set(match[1], match[2]);
  }
  if (map.size === 0) {
    throw new Error(`ProviderId::${name} listed no string arms`);
  }
  return map;
}

export const TEST_PROVIDER_CATALOG = liveProviderCatalogFrom(providerSource);
