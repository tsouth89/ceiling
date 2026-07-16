import { fireEvent, render, screen } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";

import { Dropdown } from "./Dropdown";

const options = [
  { value: "a", label: "Alpha" },
  { value: "b", label: "Beta" },
  { value: "c", label: "Gamma" },
];

describe("Dropdown", () => {
  it("shows the selected label and keeps the popup closed by default", () => {
    render(<Dropdown value="b" options={options} onChange={() => {}} ariaLabel="Metric" />);
    expect(screen.getByRole("button", { name: "Metric" }).textContent).toContain("Beta");
    expect(screen.queryByRole("listbox")).toBeNull();
  });

  it("opens on click and commits the clicked option", () => {
    const onChange = vi.fn();
    render(<Dropdown value="a" options={options} onChange={onChange} ariaLabel="Metric" />);
    fireEvent.click(screen.getByRole("button"));
    expect(screen.getByRole("listbox")).toBeTruthy();
    fireEvent.click(screen.getByRole("option", { name: "Gamma" }));
    expect(onChange).toHaveBeenCalledWith("c");
    expect(screen.queryByRole("listbox")).toBeNull();
  });

  it("marks the current value as the selected option", () => {
    render(<Dropdown value="b" options={options} onChange={() => {}} ariaLabel="Metric" />);
    fireEvent.click(screen.getByRole("button"));
    expect(screen.getByRole("option", { name: "Beta" }).getAttribute("aria-selected")).toBe("true");
    expect(screen.getByRole("option", { name: "Alpha" }).getAttribute("aria-selected")).toBe("false");
  });

  it("commits with the keyboard (open, ArrowDown, Enter)", () => {
    const onChange = vi.fn();
    render(<Dropdown value="a" options={options} onChange={onChange} ariaLabel="Metric" />);
    fireEvent.keyDown(screen.getByRole("button"), { key: "ArrowDown" });
    const list = screen.getByRole("listbox");
    fireEvent.keyDown(list, { key: "ArrowDown" });
    fireEvent.keyDown(list, { key: "Enter" });
    expect(onChange).toHaveBeenCalledWith("b");
  });

  it("closes on Escape without changing the value", () => {
    const onChange = vi.fn();
    render(<Dropdown value="a" options={options} onChange={onChange} ariaLabel="Metric" />);
    fireEvent.click(screen.getByRole("button"));
    fireEvent.keyDown(screen.getByRole("listbox"), { key: "Escape" });
    expect(screen.queryByRole("listbox")).toBeNull();
    expect(onChange).not.toHaveBeenCalled();
  });

  it("does not open when disabled", () => {
    render(<Dropdown value="a" options={options} onChange={() => {}} ariaLabel="Metric" disabled />);
    fireEvent.click(screen.getByRole("button"));
    expect(screen.queryByRole("listbox")).toBeNull();
  });
});
