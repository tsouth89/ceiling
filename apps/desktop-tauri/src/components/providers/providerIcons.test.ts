import { describe, expect, it } from "vitest";
import { TEST_PROVIDER_CATALOG } from "../../test/providerCatalog";
import { PROVIDER_ICON_REGISTRY } from "./providerIcons";

/** Relative luminance per WCAG 2.x, for a `#rrggbb` string. */
function luminance(hex: string): number {
  const channels = [1, 3, 5].map((offset) => {
    const value = parseInt(hex.slice(offset, offset + 2), 16) / 255;
    return value <= 0.03928 ? value / 12.92 : ((value + 0.055) / 1.055) ** 2.4;
  });
  return 0.2126 * channels[0] + 0.7152 * channels[1] + 0.0722 * channels[2];
}

function contrast(a: string, b: string): number {
  const [hi, lo] = [luminance(a), luminance(b)].sort((x, y) => y - x);
  return (hi + 0.05) / (lo + 0.05);
}

// The two surfaces a provider icon actually sits on: the dark card and the
// light one. Both ship, so brandColor has to clear a floor against each.
const DARK_CARD = "#1c2026";
const LIGHT_CARD = "#f5f7fa";

describe("provider icon registry", () => {
  it("has explicit icon metadata for every provider in the catalog", () => {
    for (const [id] of TEST_PROVIDER_CATALOG) {
      expect(PROVIDER_ICON_REGISTRY[id], id).toBeDefined();
    }
  });

  /**
   * OpenCode ships one monochrome square-ring mark, black on white and white
   * on black. Neither end of that survives both themes, and `brandColor` is
   * written straight into an inline `color` that a `[data-theme]` rule cannot
   * override, so it has to be a middle shade. A near-white here scored about
   * 1.1:1 on the light dashboard and the mark simply disappeared.
   */
  it.each(["opencode", "opencodego"])(
    "keeps the %s mark legible on both the light and dark card",
    (id) => {
      const { brandColor } = PROVIDER_ICON_REGISTRY[id];
      expect(contrast(brandColor, DARK_CARD)).toBeGreaterThan(2);
      expect(contrast(brandColor, LIGHT_CARD)).toBeGreaterThan(2);
    },
  );

  /**
   * The bundled SVGs carried a hardcoded `fill="#211E1E"`. `tint()` only
   * rewrites *white* fills, so that black survived into the dark dashboard as a
   * near-invisible square. Every mark that ships an SVG has to reach
   * `currentColor` for `brandColor` to mean anything at all.
   */
  it.each(["opencode", "opencodego"])(
    "lets the %s SVG take its color from the registry",
    (id) => {
      const svg = PROVIDER_ICON_REGISTRY[id].svgPath;
      expect(svg).toBeDefined();
      expect(svg).toContain('fill="currentColor"');
      expect(svg).not.toMatch(/fill="#[0-9a-f]{3,8}"/i);
    },
  );
});
