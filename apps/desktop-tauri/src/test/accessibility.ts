import { axe } from "vitest-axe";
import type { AxeMatchers } from "vitest-axe";
import { expect } from "vitest";

export async function expectNoAccessibilityViolations(
  container: Element,
): Promise<void> {
  const results = await axe(container, {
    rules: {
      // jsdom does not implement the canvas API axe uses for contrast.
      "color-contrast": { enabled: false },
    },
  });
  (
    expect(results) as unknown as ReturnType<typeof expect> & AxeMatchers
  ).toHaveNoViolations();
}
