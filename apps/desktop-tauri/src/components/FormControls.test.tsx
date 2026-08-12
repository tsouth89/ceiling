import { fireEvent, render, screen } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";

import { Field, NumberInput, Select, TextInput, Toggle } from "./FormControls";

const options = [
  { value: "a", label: "Alpha" },
  { value: "b", label: "Beta" },
];

describe("Field", () => {
  it("gives a bare Toggle a reachable name from the field label", () => {
    render(
      <Field label="Start at login" leading>
        <Toggle checked={false} onChange={() => {}} />
      </Field>,
    );
    expect(screen.getByLabelText("Start at login")).toBeTruthy();
  });

  it("gives a Select a reachable name from the field label", () => {
    render(
      <Field label="Interface language">
        <Select value="a" options={options} onChange={() => {}} />
      </Field>,
    );
    expect(screen.getByLabelText("Interface language")).toBeTruthy();
  });

  it("gives a NumberInput a reachable name from the field label", () => {
    render(
      <Field label="Refresh interval">
        <NumberInput value={5} onChange={() => {}} />
      </Field>,
    );
    expect(screen.getByLabelText("Refresh interval")).toBeTruthy();
  });

  it("gives a TextInput a reachable name from the field label", () => {
    render(
      <Field label="Custom endpoint">
        <TextInput value="" onChange={() => {}} />
      </Field>,
    );
    expect(screen.getByLabelText("Custom endpoint")).toBeTruthy();
  });

  it("does not override a control's own ariaLabel", () => {
    render(
      <Field label="Notification test">
        <TextInput value="" onChange={() => {}} ariaLabel="Custom name" />
      </Field>,
    );
    expect(screen.getByLabelText("Custom name")).toBeTruthy();
    expect(screen.queryByLabelText("Notification test")).toBeNull();
  });

  it("does not override a Toggle's own visible label", () => {
    render(
      <Field label="Sound">
        <Toggle checked disabled={false} onChange={() => {}} label="Enable sound" />
      </Field>,
    );
    expect(screen.getByLabelText("Enable sound")).toBeTruthy();
  });

  it("still fires onChange normally after receiving the injected name", () => {
    const onChange = vi.fn();
    render(
      <Field label="Start minimized" leading>
        <Toggle checked={false} onChange={onChange} />
      </Field>,
    );
    fireEvent.click(screen.getByLabelText("Start minimized"));
    expect(onChange).toHaveBeenCalledWith(true);
  });

  it("gives a bare native input a reachable name using aria-label, not the ariaLabel prop", () => {
    render(
      <Field label="Codex log paths">
        <input type="text" value="" onChange={() => {}} />
      </Field>,
    );
    const input = screen.getByLabelText("Codex log paths");
    expect(input.getAttribute("aria-label")).toBe("Codex log paths");
    // Guard against regressing back to the invalid camelCase attribute name.
    expect(input.getAttribute("arialabel")).toBeNull();
  });

  it("leaves a layout wrapper unnamed rather than emitting invalid ARIA", () => {
    // GeneralTab wraps two Fields around a `sound-enabled-row` div. `aria-label`
    // is prohibited on the implicit `generic` role, so naming the wrapper would
    // be markup assistive technology discards — and it would still leave the
    // button inside anonymous. The controls in there carry their own names.
    const { container } = render(
      <Field label="Test notification" leading>
        <div className="sound-enabled-row">
          <button type="button">Send test</button>
        </div>
      </Field>,
    );
    const wrapper = container.querySelector(".sound-enabled-row");
    expect(wrapper).not.toBeNull();
    expect(wrapper?.getAttribute("aria-label")).toBeNull();
  });
});

describe("TextInput", () => {
  it("supports its own ariaLabel when used standalone, outside Field", () => {
    render(<TextInput value="" onChange={() => {}} ariaLabel="Search" />);
    expect(screen.getByLabelText("Search")).toBeTruthy();
  });
});
