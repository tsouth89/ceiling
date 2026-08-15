#!/usr/bin/env node
// Native control theming check (SBS-225) — guards the rules in styles.css that
// pin the three parts of a form field the engine draws for us: the
// number-input spinner arrows, the text caret, and the selection highlight.
//
// None of this is reachable from a React test. jsdom does not paint a caret, a
// spinner, or a selection, and the properties that make the pin work are easy
// to lose in a stylesheet sweep. So the stylesheet text is checked directly,
// the same way `check-locale-drift.mjs` checks the locale lists.
//
// Invoked from `pnpm run check-native-controls` and automatically from
// `pnpm run prebuild`.
//
// Exit codes:
//   0 — all rules present
//   1 — a rule is missing or has drifted
//   2 — parse failure (file missing / section markers not found)

import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { dirname, resolve } from "node:path";

const here = dirname(fileURLToPath(import.meta.url));
const cssPath = resolve(here, "..", "src", "styles.css");

function die(code, msg) {
  console.error(`[check-native-controls] ${msg}`);
  process.exit(code);
}

let css;
try {
  css = readFileSync(cssPath, "utf8").replace(/\r\n/g, "\n");
} catch (err) {
  die(2, `failed to read ${cssPath}: ${err.message}`);
}

// ── Slice out the SBS-225 section ────────────────────────────────────
const START = "Native control theming (SBS-225)";
const END = "Provider sidebar (Preferences";
const start = css.indexOf(START);
if (start === -1) {
  die(2, `could not find the "${START}" banner in styles.css`);
}
const end = css.indexOf(END, start);
if (end === -1) {
  die(2, `could not find the "${END}" banner after the SBS-225 section`);
}

// Drop every comment, including the tail of the banner this slice starts
// inside. The prose explains the rules and names the same properties, so
// leaving it in would let a comment satisfy a check.
const raw = css.slice(start, end);
const block = raw.slice(raw.indexOf("*/") + 2).replace(/\/\*[\s\S]*?\*\//g, "");

const forcedAt = block.indexOf("@media (forced-colors: active)");
if (forcedAt === -1) {
  die(1, "missing the `@media (forced-colors: active)` block");
}
const normal = block.slice(0, forcedAt);
const forced = block.slice(forcedAt);

// The four controls whose engine-drawn parts are pinned, as one selector list.
const CONTROLS = [".select", ".number-input", ".text-input", "textarea"];
const CONTROL_LIST = CONTROLS.join(",\n");
const LIGHT_CONTROL_LIST = CONTROLS.map((c) => `[data-theme="light"] ${c}`).join(",\n");

const failures = [];
function want(scope, label, source, matcher) {
  const ok =
    typeof matcher === "string" ? source.includes(matcher) : matcher.test(source);
  if (!ok) failures.push(`${scope}: ${label}`);
}
function reject(scope, label, source, matcher) {
  const hit =
    typeof matcher === "string" ? source.includes(matcher) : matcher.test(source);
  if (hit) failures.push(`${scope}: ${label}`);
}

// ── Spinner arrows and caret, pinned on the controls themselves ──────
// The root already carries `color-scheme`, but the app theme and the Windows
// theme are independent settings; pinning on the controls means a later
// refactor of the root cannot silently hand them back to Windows.
want("controls", "the four controls share one selector list", normal, CONTROL_LIST);
want("controls", "a [data-theme=\"light\"] override for the same four", normal, LIGHT_CONTROL_LIST);
want("controls", "color-scheme: dark", normal, /color-scheme:\s*dark/);
want("controls", "color-scheme: light", normal, /color-scheme:\s*light/);
want("controls", "caret-color: var(--accent)", normal, /caret-color:\s*var\(--accent\)/);

// ── Selection ────────────────────────────────────────────────────────
// A bare document rule is not guaranteed to reach the form-control shadow tree
// in Blink, so the controls are named alongside it.
want("selection", "an unscoped ::selection rule", normal, /\n::selection,/);
want("selection", "input::selection", normal, "input::selection,");
want("selection", "textarea::selection", normal, "textarea::selection {");

// Opaque fill, so the ink sits on the fill rather than on whatever surface is
// underneath and one unscoped rule stays legible everywhere. It also leaves
// nothing for an older WebView2 to drop: a `color-mix()` it cannot parse takes
// the whole declaration with it and selection silently does nothing.
want("selection", "an opaque background-color: var(--accent)", normal, /background-color:\s*var\(--accent\)/);
reject("selection", "no color-mix() in the selection fill", normal, /color-mix/);
reject("selection", "no translucent rgba() selection fill", normal, /background(-color)?:\s*rgba/);

// Blink paints form-control glyphs through -webkit-text-fill-color, which beats
// `color`; without it the digits in a number field keep their unselected fill.
want("selection", "color: var(--selection-ink)", normal, /\scolor:\s*var\(--selection-ink\)/);
want("selection", "-webkit-text-fill-color: var(--selection-ink)", normal, /-webkit-text-fill-color:\s*var\(--selection-ink\)/);
// Reusing the unselected text colour means the glyphs never change.
reject("selection", "selection ink is not var(--text-primary)", normal, /\scolor:\s*var\(--text-primary\)/);

// ── Windows Contrast Themes ──────────────────────────────────────────
// Highlight pseudos are not reliably force-adjusted, so an app that paints its
// own selection can survive a yellow-on-black contrast theme.
want("forced-colors", "::selection reset", forced, /\n {2}::selection,/);
want("forced-colors", "input::selection reset", forced, "input::selection,");
want("forced-colors", "textarea::selection reset", forced, "textarea::selection {");
want("forced-colors", "background-color: Highlight", forced, /background-color:\s*Highlight/);
want("forced-colors", "color: HighlightText", forced, /\scolor:\s*HighlightText/);
want("forced-colors", "-webkit-text-fill-color: HighlightText", forced, /-webkit-text-fill-color:\s*HighlightText/);
want("forced-colors", "caret-color: CanvasText", forced, /caret-color:\s*CanvasText/);
for (const control of CONTROLS) {
  want("forced-colors", `${control} hands its caret back`, forced, control);
}

// ── Per-theme selection ink ──────────────────────────────────────────
// The ink has to differ between themes, or one of the two reuses the body text
// colour it is meant to contrast against.
const lightBlockAt = css.indexOf('\n[data-theme="light"] {');
if (lightBlockAt === -1) {
  die(2, 'could not find the `[data-theme="light"]` token block');
}
const inkRe = /--selection-ink:\s*(#[0-9a-fA-F]{3,8})/;
const darkInk = inkRe.exec(css.slice(0, lightBlockAt))?.[1];
const lightInk = inkRe.exec(css.slice(lightBlockAt))?.[1];
if (!darkInk) failures.push("tokens: no --selection-ink in the dark token block");
if (!lightInk) failures.push('tokens: no --selection-ink in the [data-theme="light"] block');
if (darkInk && lightInk && darkInk === lightInk) {
  failures.push(`tokens: --selection-ink is ${darkInk} in both themes`);
}

if (failures.length) {
  console.error("[check-native-controls] ERROR: styles.css drifted from the SBS-225 rules");
  for (const f of failures) console.error(`  - ${f}`);
  process.exit(1);
}

console.log(
  `[check-native-controls] OK — spinner, caret, and selection pinned on ${CONTROLS.length} controls; ` +
    `selection ink ${darkInk} dark / ${lightInk} light; forced-colors reset present`,
);
