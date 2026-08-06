#!/usr/bin/env node
// Locale drift check — verifies that the TypeScript ALL_LOCALE_KEYS array
// matches the Rust locale_keys! list in rust/src/locale.rs exactly
// (same keys, same count). Invoked from `pnpm run check-locale` and
// automatically from `pnpm run prebuild`.
//
// Exit codes:
//   0 — lists match
//   1 — mismatch (prints a diff-style report)
//   2 — parse failure (file missing / regex produced zero matches)

import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { dirname, resolve } from "node:path";

const here = dirname(fileURLToPath(import.meta.url));
const repoRoot = resolve(here, "..", "..", "..");
const rustPath = resolve(repoRoot, "rust", "src", "locale.rs");
const tsPath = resolve(here, "..", "src", "i18n", "keys.ts");
const ftlPaths = {
  "en-US": resolve(repoRoot, "rust", "src", "locale", "en-US.ftl"),
  "zh-CN": resolve(repoRoot, "rust", "src", "locale", "zh-CN.ftl"),
};

function die(code, msg) {
  console.error(`[check-locale] ${msg}`);
  process.exit(code);
}

let rustSrc;
let tsSrc;
const ftlSrc = {};
try {
  rustSrc = readFileSync(rustPath, "utf8");
  tsSrc = readFileSync(tsPath, "utf8");
  for (const [name, path] of Object.entries(ftlPaths)) {
    ftlSrc[name] = readFileSync(path, "utf8");
  }
} catch (err) {
  die(2, `failed to read source files: ${err.message}`);
}

function ftlKeys(src) {
  return [...src.matchAll(/^([A-Za-z0-9_]+)\s*=/gm)].map((match) => match[1]);
}

// Extract Rust LocaleKey variants from the single source list.
const rustKeysMatch = rustSrc.match(
  /locale_keys!\s*\{([\s\S]*?)^\}/m,
);
if (!rustKeysMatch) {
  die(2, "could not locate `locale_keys!` block in rust/src/locale.rs");
}
const rustKeys = [];
const variantRe = /^\s*([A-Z][A-Za-z0-9]*)\s*,\s*$/gm;
let m;
while ((m = variantRe.exec(rustKeysMatch[1])) !== null) {
  rustKeys.push(m[1]);
}
if (rustKeys.length === 0) {
  die(2, "parsed zero variants from locale_keys!");
}

// Extract TS ALL_LOCALE_KEYS entries.
const tsBlockMatch = tsSrc.match(
  /export const ALL_LOCALE_KEYS\s*=\s*\[([\s\S]*?)\]\s*as const;/,
);
if (!tsBlockMatch) {
  die(2, "could not locate `export const ALL_LOCALE_KEYS` in keys.ts");
}
const tsKeys = [];
const tsKeyRe = /"(\w+)"/g;
while ((m = tsKeyRe.exec(tsBlockMatch[1])) !== null) {
  tsKeys.push(m[1]);
}
if (tsKeys.length === 0) {
  die(2, "parsed zero entries from ALL_LOCALE_KEYS");
}

const rustSet = new Set(rustKeys);
const tsSet = new Set(tsKeys);
const onlyInRust = rustKeys.filter((k) => !tsSet.has(k));
const onlyInTs = tsKeys.filter((k) => !rustSet.has(k));

if (rustKeys.length !== tsKeys.length || onlyInRust.length || onlyInTs.length) {
  console.error(
    `[check-locale] DRIFT DETECTED  rust=${rustKeys.length} ts=${tsKeys.length}`,
  );
  if (onlyInRust.length) {
    console.error(`  only in Rust (${onlyInRust.length}):`);
    for (const k of onlyInRust) console.error(`    - ${k}`);
  }
  if (onlyInTs.length) {
    console.error(`  only in TS   (${onlyInTs.length}):`);
    for (const k of onlyInTs) console.error(`    - ${k}`);
  }
  process.exit(1);
}

let catalogFailed = false;
const declared = new Set(rustKeys);
for (const [name, src] of Object.entries(ftlSrc)) {
  const keys = ftlKeys(src);
  if (keys.length === 0) {
    die(2, `parsed zero keys from ${name}.ftl`);
  }
  const catalogSet = new Set(keys);
  const missing = rustKeys.filter((k) => !catalogSet.has(k));
  const extra = keys.filter((k) => !declared.has(k));
  const label = name === "zh-CN" ? "WARN" : "ERROR";

  if (missing.length || extra.length) {
    if (name === "zh-CN") {
      console.warn(`[check-locale] ${label}: ${name} catalog drifted`);
    } else {
      console.error(`[check-locale] ${label}: ${name} catalog drifted`);
      catalogFailed = true;
    }
    if (missing.length) {
      console.error(`  missing from ${name} (${missing.length}):`);
      for (const k of missing) console.error(`    - ${k}`);
    }
    if (extra.length) {
      console.error(`  only in ${name} (${extra.length}):`);
      for (const k of extra) console.error(`    - ${k}`);
    }
  }
}

if (catalogFailed) {
  process.exit(1);
}

console.log(
  `[check-locale] OK — ${rustKeys.length} locale keys match between Rust, TS, and FTL catalogs`,
);
