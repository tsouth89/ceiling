import { fireEvent, render, screen } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import { SecretField } from "./SecretField";

function renderField(
  value = "sk-secret",
  onChange = vi.fn(),
) {
  return {
    onChange,
    ...render(
      <SecretField
        label="Cookie header"
        value={value}
        onChange={onChange}
        placeholder="Paste cookie header value…"
        revealLabel="Reveal"
        hideLabel="Hide"
      />,
    ),
  };
}

describe("SecretField", () => {
  it("masks the value by default and names the field", () => {
    renderField();
    const field = screen.getByLabelText("Cookie header");
    expect(field).toHaveValue("sk-secret");
    expect(field).toHaveClass("secret-field__input--masked");
    expect(field).toHaveAttribute("autocomplete", "off");
    expect(field).toHaveAttribute("spellcheck", "false");
    expect(field).toHaveAttribute("autocapitalize", "none");
    expect(screen.getByRole("button", { name: "Reveal" })).toHaveAttribute(
      "aria-pressed",
      "false",
    );
  });

  it("reveals and hides without changing the submitted value", () => {
    const { onChange } = renderField();
    const field = screen.getByLabelText("Cookie header");
    fireEvent.click(screen.getByRole("button", { name: "Reveal" }));
    expect(field).not.toHaveClass("secret-field__input--masked");
    expect(screen.getByRole("button", { name: "Hide" })).toHaveAttribute(
      "aria-pressed",
      "true",
    );
    fireEvent.click(screen.getByRole("button", { name: "Hide" }));
    expect(field).toHaveClass("secret-field__input--masked");
    expect(onChange).not.toHaveBeenCalled();
    expect(field).toHaveValue("sk-secret");
  });

  it("forwards typed and pasted values unchanged", () => {
    const { onChange, rerender } = renderField("");
    const field = screen.getByLabelText("Cookie header");
    fireEvent.change(field, { target: { value: "session=abc" } });
    expect(onChange).toHaveBeenCalledWith("session=abc");
    rerender(
      <SecretField
        label="Cookie header"
        value="session=abc"
        onChange={onChange}
        revealLabel="Reveal"
        hideLabel="Hide"
      />,
    );
    expect(screen.getByLabelText("Cookie header")).toHaveValue("session=abc");
  });
});
