import { describe, expect, it } from "vitest";
import { formatLocale } from "./formatLocale";

describe("formatLocale", () => {
  it("replaces placeholders in order", () => {
    expect(formatLocale("Remove the {} token account from {}?", "Work", "Copilot")).toBe(
      "Remove the Work token account from Copilot?",
    );
  });

  it("leaves leftover placeholders when args run out", () => {
    expect(formatLocale("Hello {}")).toBe("Hello {}");
  });
});
