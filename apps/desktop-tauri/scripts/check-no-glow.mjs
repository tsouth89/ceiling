#!/usr/bin/env node
// No-glow check (SBS-158) — CEILING_UI.md states "no glow effects", and the
// promo boost chip had been drawing one anyway. This keeps the rule honest.
//
// A glow is a box-shadow layer with no offset and a non-zero blur, so it bleeds
// outward in every direction. Drop shadows (which carry a Y offset), inset
// shadows, and zero-blur rings such as `0 0 0 2px` are all allowed and are what
// the design system uses instead.
//
// This lives as a script rather than a vitest test because Vitest stubs CSS
// imports to empty strings by default, so a `?raw` import of a stylesheet reads
// as blank and the check would pass without inspecting anything.
//
// Invoked from `pnpm run check-css` and automatically from `pnpm run prebuild`.
//
// Exit codes:
//   0 — no glow
//   1 — at least one glow found
//   2 — a stylesheet was missing or contained no box-shadow at all

import { readFileSync, readdirSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { dirname, resolve } from "node:path";

const here = dirname(fileURLToPath(import.meta.url));
const srcDir = resolve(here, "..", "src");

// Discovered rather than listed, so a stylesheet added later is covered
// without anyone remembering to add it here.
const sheets = readdirSync(srcDir, { recursive: true, withFileTypes: true })
  .filter((entry) => entry.isFile() && entry.name.endsWith(".css"))
  .map((entry) => resolve(entry.parentPath ?? entry.path, entry.name))
  .sort();

if (sheets.length === 0) {
  console.error(`[check-css] no stylesheets found under ${srcDir}`);
  process.exit(2);
}

// Blank out CSS block comments, keeping newlines so reported line numbers still
// point at the real line. Without this, a reviewer noting the old halo value
// next to a fixed rule turns prebuild red over a shadow nothing renders.
function stripComments(css) {
  return css.replace(/\/\*[\s\S]*?\*\//g, (match) => match.replace(/[^\n]/g, " "));
}

/**
 * Every `box-shadow` declaration and its value.
 *
 * A declaration may be terminated by `}` instead of `;` — the last one in a
 * block is allowed to drop the semicolon, and `.x{box-shadow:0 0 8px red}` is
 * valid CSS that a `;`-only pattern never even inspects.
 */
// Case-insensitive: CSS property names are, so `BOX-SHADOW:` renders fine and
// a case-sensitive scan would report a clean sheet without reading it.
const BOX_SHADOW = /box-shadow\s*:\s*([^;{}]+)\s*(?:;|\})/gi;

/** Split a box-shadow value into layers, ignoring commas inside `color-mix(…)`. */
function shadowLayers(value) {
  const layers = [];
  let depth = 0;
  let current = "";
  for (const char of value) {
    if (char === "(") depth += 1;
    if (char === ")") depth -= 1;
    if (char === "," && depth === 0) {
      layers.push(current);
      current = "";
      continue;
    }
    current += char;
  }
  layers.push(current);
  return layers.map((layer) => layer.trim()).filter(Boolean);
}

// Blank out function calls so a colour argument cannot be read as a keyword.
// `0 0 8px var(--chip-inset)` is an outward glow, but testing the whole layer
// for the word sees the token name and waves it through — and this tree already
// says "inset" on plenty of shadows, so hoisting one into a variable is a
// realistic next edit.
function withoutFunctions(layer) {
  let out = layer;
  let previous;
  do {
    previous = out;
    out = out.replace(/\([^()]*\)/g, " ");
  } while (out !== previous);
  return out;
}

/** `0 0 8px <color>` is a glow; `0 0 0 1px <color>` and `0 1px 3px` are not. */
function isGlow(layer) {
  if (/\binset\b/i.test(withoutFunctions(layer))) return false;
  // Read the leading run of lengths and stop at the colour. A bare `0` is a
  // valid CSS length, so the unit is optional.
  const lengths = [];
  for (const token of layer.split(/\s+/)) {
    const match = /^(-?[\d.]+)(px|rem|em)?$/.exec(token);
    if (!match) break;
    lengths.push(Number.parseFloat(match[1]));
  }
  const [x, y, blur] = lengths;
  return lengths.length >= 3 && x === 0 && y === 0 && blur > 0;
}

/** Self-check: a rule that can never fire proves nothing. */
function assertDetectorWorks() {
  const cases = [
    ["0 0 8px red", true],
    ["0 0 0 2px red", false],
    ["0 1px 3px rgba(0, 0, 0, 0.3)", false],
    ["inset 0 0 6px red", false],
    ["INSET 0 0 6px red", false],
    ["0 0 9px color-mix(in srgb, red 20%, blue)", true],
    // The keyword only counts outside a function call.
    ["0 0 8px var(--chip-inset)", true],
    ["0 0 8px var(--inset-color)", true],
    ["0 0 0 1px var(--chip-inset)", false],
  ];
  for (const [layer, expected] of cases) {
    if (isGlow(layer) !== expected) {
      console.error(`[check-css] detector is broken on: ${layer}`);
      process.exit(2);
    }
  }

  // The scan is the other half of the gate: a value it never extracts is a
  // value isGlow never sees.
  const scan = (css) =>
    [...stripComments(css).matchAll(BOX_SHADOW)].flatMap((match) =>
      shadowLayers(match[1]).filter(isGlow),
    );
  const scanCases = [
    [".a { box-shadow: 0 0 8px red; }", 1, "semicolon-terminated"],
    // Valid CSS: the last declaration in a block may omit the semicolon.
    [".a { box-shadow: 0 0 8px red }", 1, "brace-terminated"],
    ["/* box-shadow: 0 0 8px red; */", 0, "inside a comment"],
    [".a { box-shadow: 0 1px 3px red; }", 0, "drop shadow"],
    // CSS property names are case-insensitive.
    [".a { BOX-SHADOW: 0 0 8px red; }", 1, "uppercase property"],
    [".a { Box-Shadow: 0 0 8px red; }", 1, "mixed-case property"],
  ];
  for (const [css, expected, label] of scanCases) {
    if (scan(css).length !== expected) {
      console.error(`[check-css] scan is broken on: ${label}`);
      process.exit(2);
    }
  }
}

assertDetectorWorks();

let failed = false;
// Counted across every sheet, not per sheet: a small stylesheet added later
// is normal, a total of zero means the scan read nothing.
let scanned = 0;
for (const path of sheets) {
  let css;
  try {
    css = readFileSync(path, "utf8");
  } catch (error) {
    console.error(`[check-css] cannot read ${path}: ${error.message}`);
    process.exit(2);
  }

  const declarations = [...stripComments(css).matchAll(BOX_SHADOW)];
  scanned += declarations.length;

  for (const declaration of declarations) {
    const line = css.slice(0, declaration.index).split("\n").length;
    for (const layer of shadowLayers(declaration[1])) {
      if (!isGlow(layer)) continue;
      failed = true;
      console.error(
        `[check-css] glow at ${path}:${line} — ${layer.replace(/\s+/g, " ")}`,
      );
    }
  }
}

if (scanned < 5) {
  console.error(
    `[check-css] only ${scanned} box-shadow rules across ${sheets.length} stylesheets — the scan is not reading the real files`,
  );
  process.exit(2);
}

if (failed) {
  console.error(
    "[check-css] CEILING_UI.md rules out glow. Use a zero-blur ring (0 0 0 Npx) or an offset drop shadow.",
  );
  process.exit(1);
}

console.log(
  `[check-css] OK — no glow in ${scanned} box-shadow rules across ${sheets.length} stylesheets`,
);
