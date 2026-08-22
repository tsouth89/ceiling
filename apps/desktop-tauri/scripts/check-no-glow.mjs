#!/usr/bin/env node
// No-glow check (SBS-158, SBS-974) — CEILING_UI.md states "no glow effects",
// and the promo boost chip had been drawing one anyway. This keeps the rule
// honest.
//
// SBS-974: `isGlow` stops at the first non-length token, so `var(--halo)`
// was waved through, and `.ts` / `.tsx` `boxShadow` literals were never read.
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

function listFiles(dir) {
  return readdirSync(dir, { recursive: true, withFileTypes: true })
    .filter((entry) => entry.isFile())
    .map((entry) => resolve(entry.parentPath ?? entry.path, entry.name))
    .sort();
}

// Discovered rather than listed, so a stylesheet or component added later
// is covered without anyone remembering to add it here.
const allFiles = listFiles(srcDir);
const sheets = allFiles.filter((path) => path.endsWith(".css"));
const scripts = allFiles.filter(
  (path) => path.endsWith(".ts") || path.endsWith(".tsx"),
);

if (sheets.length === 0) {
  console.error(`[check-css] no stylesheets found under ${srcDir}`);
  process.exit(2);
}

if (scripts.length === 0) {
  console.error(`[check-css] no .ts / .tsx files found under ${srcDir}`);
  process.exit(2);
}

// Blank out CSS block comments, keeping newlines so reported line numbers still
// point at the real line. Without this, a reviewer noting the old halo value
// next to a fixed rule turns prebuild red over a shadow nothing renders.
function stripComments(css) {
  return css.replace(/\/\*[\s\S]*?\*\//g, (match) => match.replace(/[^\n]/g, " "));
}

/**
 * Blank out JS/TS comments and regex-literal bodies, and report which offsets
 * sit inside a string literal.
 *
 * Three things have to survive this pass. A reviewer who pastes
 * `boxShadow: "0 0 8px red"` into a comment must not turn prebuild red. A
 * regex must not be read as a comment: `/^[a-z][a-z0-9+.-]*:\/\//` (already in
 * `src/test/tauriCsp.test.ts`) otherwise blanks the rest of its line from the
 * embedded `//`, and a regex containing `/*` blanks everything up to the next
 * `*\/` — silently hiding every later boxShadow in that module. And the
 * `boxShadow` key itself has to be distinguishable from the same word sitting
 * inside a string, so `const msg = 'use boxShadow: "0 0 8px red"'` does not
 * count as a painted glow.
 *
 * String *contents* are kept, because the value being looked for is a string.
 * Regex bodies are blanked, because nothing downstream wants them and blanking
 * removes any chance of their punctuation being read as code.
 *
 * @returns {{ code: string, inString: boolean[] }} `code` is the same length as
 * `src`, so every index and reported line number still lines up.
 */
function scanJs(src) {
  const code = new Array(src.length);
  const inString = new Array(src.length).fill(false);
  const blank = (i) => {
    code[i] = src[i] === "\n" ? "\n" : " ";
  };
  // A `/` opens a regex only in a value position. After an identifier, a
  // number, a closing bracket, or a string it is division.
  let lastSignificant = "";
  const regexAllowed = () =>
    lastSignificant === "" || !/[A-Za-z0-9_$)\]"'`]/.test(lastSignificant);

  let i = 0;
  while (i < src.length) {
    const two = src.slice(i, i + 2);
    if (two === "//") {
      while (i < src.length && src[i] !== "\n") blank(i++);
      continue;
    }
    if (two === "/*") {
      while (i + 1 < src.length && src.slice(i, i + 2) !== "*/") blank(i++);
      if (i < src.length) blank(i++);
      if (i < src.length) blank(i++);
      continue;
    }
    const quote = src[i];
    if (quote === '"' || quote === "'" || quote === "`") {
      code[i] = quote;
      i += 1;
      while (i < src.length) {
        if (src[i] === "\\") {
          inString[i] = true;
          code[i] = src[i];
          i += 1;
          if (i < src.length) {
            inString[i] = true;
            code[i] = src[i];
            i += 1;
          }
          continue;
        }
        if (src[i] === quote) {
          code[i] = src[i];
          i += 1;
          break;
        }
        inString[i] = true;
        code[i] = src[i];
        i += 1;
      }
      lastSignificant = quote;
      continue;
    }
    if (quote === "/" && regexAllowed()) {
      const closed = blankRegexBody(src, i, code, blank);
      if (closed !== null) {
        i = closed;
        lastSignificant = "/";
        continue;
      }
    }
    code[i] = src[i];
    if (!/\s/.test(src[i])) lastSignificant = src[i];
    i += 1;
  }
  return { code: code.join(""), inString };
}

/**
 * Blank the body of the regex literal starting at `open`, keeping the
 * delimiters and flags. Returns the index just past the literal, or null when
 * the `/` turns out not to open one (an unterminated run to end of line).
 */
function blankRegexBody(src, open, code, blank) {
  let i = open + 1;
  let inClass = false;
  while (i < src.length) {
    const char = src[i];
    if (char === "\n") return null;
    if (char === "\\") {
      blank(i);
      i += 1;
      if (i < src.length) blank(i);
      i += 1;
      continue;
    }
    if (char === "[") inClass = true;
    else if (char === "]") inClass = false;
    else if (char === "/" && !inClass) break;
    blank(i);
    i += 1;
  }
  if (i >= src.length) return null;
  code[open] = "/";
  code[i] = "/";
  i += 1;
  while (i < src.length && /[a-z]/.test(src[i])) {
    code[i] = src[i];
    i += 1;
  }
  return i;
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

/**
 * Custom-property declarations in this file (`--halo: 0 0 10px red`).
 *
 * A name can be declared more than once (theme blocks). Every value is kept:
 * if any of them is a glow, a box-shadow: var(--halo) that lands on that
 * block would render one. Last-declaration-wins would miss a glow sitting
 * only on the dark theme.
 */
const CUSTOM_PROP = /(--[A-Za-z0-9_-]+)\s*:\s*([^;{}]+)\s*(?:;|\})/g;

function collectCustomProperties(text) {
  const props = new Map();
  for (const match of text.matchAll(CUSTOM_PROP)) {
    const name = match[1];
    const value = match[2].trim();
    if (!props.has(name)) props.set(name, []);
    props.get(name).push(value);
  }
  return props;
}

function indexOfTopLevelComma(text) {
  let depth = 0;
  for (let i = 0; i < text.length; i += 1) {
    if (text[i] === "(") depth += 1;
    else if (text[i] === ")") depth -= 1;
    else if (text[i] === "," && depth === 0) return i;
  }
  return -1;
}

/**
 * Leftmost innermost var(--name) / var(--name, fallback).
 *
 * Innermost first so var(--halo, var(--ring, 0 0 8px red)) substitutes the
 * nested call before the outer one, matching how CSS resolves fallbacks.
 */
function findInnermostVar(value) {
  let i = 0;
  while (i < value.length) {
    const open = /^var\s*\(/i.exec(value.slice(i));
    if (!open) {
      i += 1;
      continue;
    }
    const innerStart = i + open[0].length;
    let depth = 1;
    let j = innerStart;
    while (j < value.length && depth > 0) {
      if (value[j] === "(") depth += 1;
      else if (value[j] === ")") depth -= 1;
      j += 1;
    }
    if (depth !== 0) {
      i += 1;
      continue;
    }
    const inner = value.slice(innerStart, j - 1);
    if (/var\s*\(/i.test(inner)) {
      i = innerStart;
      continue;
    }
    const comma = indexOfTopLevelComma(inner);
    const namePart = (comma === -1 ? inner : inner.slice(0, comma)).trim();
    const fallback = comma === -1 ? null : inner.slice(comma + 1);
    const nameMatch = /^(--[A-Za-z0-9_-]+)$/.exec(namePart);
    if (nameMatch) {
      return { start: i, end: j, name: nameMatch[1], fallback };
    }
    i = j;
  }
  return null;
}

/**
 * Every way `value` can look after substituting same-file custom properties.
 *
 * Missing names use the fallback when one is written; otherwise the var()
 * is left unresolved (an unknown — not treated as "no glow", but isGlow
 * still cannot prove a glow from an unresolved token). Cycles freeze the
 * looping call so the walk cannot run forever.
 */
function allResolutions(value, props, stack = new Set()) {
  const hit = findInnermostVar(value);
  if (!hit) return [value];
  if (stack.has(hit.name)) {
    const frozen = `${value.slice(0, hit.start)}unresolved${value.slice(hit.end)}`;
    return allResolutions(frozen, props, stack);
  }
  const defs = props.get(hit.name) ?? [];
  let replacements;
  if (defs.length > 0) replacements = defs;
  else if (hit.fallback != null) replacements = [hit.fallback];
  else {
    const frozen = `${value.slice(0, hit.start)}unresolved${value.slice(hit.end)}`;
    return allResolutions(frozen, props, stack);
  }
  const next = new Set(stack);
  next.add(hit.name);
  const out = [];
  for (const replacement of replacements) {
    out.push(
      ...allResolutions(
        `${value.slice(0, hit.start)}${replacement}${value.slice(hit.end)}`,
        props,
        next,
      ),
    );
  }
  return out;
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

function layerGlows(layer, props) {
  // Substitution can leave leading space (a var() fallback is everything
  // after the comma, including the space CSS requires). isGlow splits on
  // whitespace and treats a leading empty token as "not a length", so it
  // would miss `var(--missing, 0 0 8px red)` without the trim.
  return allResolutions(layer, props).some((resolved) => isGlow(resolved.trim()));
}

function glowLayersInCss(css) {
  const stripped = stripComments(css);
  const props = collectCustomProperties(stripped);
  const glows = [];
  for (const match of stripped.matchAll(BOX_SHADOW)) {
    for (const layer of shadowLayers(match[1])) {
      if (layerGlows(layer, props)) glows.push(layer);
    }
  }
  return glows;
}

/**
 * boxShadow: "..." / '...', the quoted-key form "boxShadow": "...", and the
 * DOM assignment form `el.style.boxShadow = "..."`, which is the usual way an
 * inline shadow is set and renders exactly the same glow.
 *
 * Template literals and identifiers (boxShadow: glow) are not string literals
 * and are not scanned.
 *
 * `inString` marks offsets inside a string literal. The key must sit outside
 * one, so prose that merely quotes the property name is not a painted glow.
 */
function jsBoxShadowLiterals(src, inString = []) {
  const out = [];
  const start = /(?:["']boxShadow["']|\bboxShadow\b)\s*[:=]\s*(["'])/g;
  let match;
  while ((match = start.exec(src))) {
    if (inString[match.index]) continue;
    const quote = match[1];
    let i = match.index + match[0].length;
    let value = "";
    while (i < src.length) {
      if (src[i] === "\\") {
        value += src[i] + (src[i + 1] ?? "");
        i += 2;
        continue;
      }
      if (src[i] === quote) break;
      value += src[i];
      i += 1;
    }
    out.push({ value, index: match.index });
  }
  return out;
}

function glowLayersInJs(src) {
  const { code, inString } = scanJs(src);
  const props = collectCustomProperties(code);
  const glows = [];
  for (const literal of jsBoxShadowLiterals(code, inString)) {
    for (const layer of shadowLayers(literal.value)) {
      if (layerGlows(layer, props)) glows.push({ layer, index: literal.index });
    }
  }
  return glows;
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
  // value isGlow never sees. The extra cases are the SBS-974 failure mode —
  // a glow that only exists after var(--x) is substituted, or that lives in
  // a JS string literal.
  const scanCssCases = [
    [".a { box-shadow: 0 0 8px red; }", 1, "semicolon-terminated"],
    // Valid CSS: the last declaration in a block may omit the semicolon.
    [".a { box-shadow: 0 0 8px red }", 1, "brace-terminated"],
    ["/* box-shadow: 0 0 8px red; */", 0, "inside a comment"],
    [".a { box-shadow: 0 1px 3px red; }", 0, "drop shadow"],
    // CSS property names are case-insensitive.
    [".a { BOX-SHADOW: 0 0 8px red; }", 1, "uppercase property"],
    [".a { Box-Shadow: 0 0 8px red; }", 1, "mixed-case property"],
    [
      ":root { --halo: 0 0 10px var(--accent); } .a { box-shadow: var(--halo); }",
      1,
      "glow behind a custom property",
    ],
    [
      ":root { --panel-shadow: 0 12px 40px rgba(0, 0, 0, 0.45); } .a { box-shadow: var(--panel-shadow); }",
      0,
      "drop shadow behind a custom property",
    ],
    [
      ":root { --blur: 8px; } .a { box-shadow: 0 0 var(--blur) red; }",
      1,
      "length token supplied by a custom property",
    ],
    [
      ".a { box-shadow: var(--missing, 0 0 8px red); }",
      1,
      "glow in a var() fallback",
    ],
    [
      ":root { --halo: 0 1px 3px red; --halo: 0 0 8px red; } .a { box-shadow: var(--halo); }",
      1,
      "glow on one of several declarations of the same name",
    ],
  ];
  for (const [css, expected, label] of scanCssCases) {
    if (glowLayersInCss(css).length !== expected) {
      console.error(`[check-css] scan is broken on: ${label}`);
      process.exit(2);
    }
  }

  const scanJsCases = [
    ['const x = { boxShadow: "0 0 8px red" };', 1, "tsx string literal"],
    ["const x = { boxShadow: '0 0 8px red' };", 1, "tsx single-quoted literal"],
    ['const x = { boxShadow: "0 1px 3px red" };', 0, "tsx drop shadow"],
    ['const x = { "boxShadow": "0 0 8px red" };', 1, "quoted boxShadow key"],
    ['// boxShadow: "0 0 8px red"', 0, "line-commented tsx literal"],
    ['/* boxShadow: "0 0 8px red" */', 0, "block-commented tsx literal"],
    [
      'const url = "http://example.test"; const x = { boxShadow: "0 1px 3px red" };',
      0,
      "url slashes are not a comment",
    ],
    // SBS-974: the DOM assignment form paints the same glow as the object form.
    [
      'el.style.boxShadow = "0 0 8px red";',
      1,
      "style.boxShadow assignment",
    ],
    [
      'el.style.boxShadow = "0 1px 3px red";',
      0,
      "style.boxShadow assignment, drop shadow",
    ],
    // A regex is not a comment. Both of these used to blank the glow that
    // follows them, the second one to the end of the file.
    [
      'const re = /^[a-z][a-z0-9+.-]*:\\/\\//; const x = { boxShadow: "0 0 8px red" };',
      1,
      "glow after a regex containing //",
    ],
    [
      'const re = /a\\/*b/; const x = { boxShadow: "0 0 8px red" };',
      1,
      "glow after a regex containing /*",
    ],
    // Division still is division, so the comment stripper must not swallow
    // the rest of the line as a regex body.
    [
      'const half = total / 2; const x = { boxShadow: "0 0 8px red" };',
      1,
      "division is not a regex",
    ],
    // The property name inside prose paints nothing.
    [
      'const msg = \'use boxShadow: "0 0 8px red"\';',
      0,
      "boxShadow named inside a string",
    ],
  ];
  for (const [src, expected, label] of scanJsCases) {
    if (glowLayersInJs(src).length !== expected) {
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

  const stripped = stripComments(css);
  const props = collectCustomProperties(stripped);
  const declarations = [...stripped.matchAll(BOX_SHADOW)];
  scanned += declarations.length;

  for (const declaration of declarations) {
    const line = css.slice(0, declaration.index).split("\n").length;
    for (const layer of shadowLayers(declaration[1])) {
      if (!layerGlows(layer, props)) continue;
      failed = true;
      console.error(
        `[check-css] glow at ${path}:${line} — ${layer.replace(/\s+/g, " ")}`,
      );
    }
  }
}

for (const path of scripts) {
  let src;
  try {
    src = readFileSync(path, "utf8");
  } catch (error) {
    console.error(`[check-css] cannot read ${path}: ${error.message}`);
    process.exit(2);
  }

  const { code: stripped, inString } = scanJs(src);
  const props = collectCustomProperties(stripped);
  const literals = jsBoxShadowLiterals(stripped, inString);
  scanned += literals.length;

  for (const literal of literals) {
    const line = src.slice(0, literal.index).split("\n").length;
    for (const layer of shadowLayers(literal.value)) {
      if (!layerGlows(layer, props)) continue;
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
  `[check-css] OK — no glow in ${scanned} box-shadow rules across ${sheets.length} stylesheets and ${scripts.length} scripts`,
);
