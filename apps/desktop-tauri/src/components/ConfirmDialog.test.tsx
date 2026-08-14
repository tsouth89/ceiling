import { fireEvent, render, screen } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import { ConfirmDialog } from "./ConfirmDialog";

describe("ConfirmDialog", () => {
  it("names the provider and credential type and can be cancelled", () => {
    const onConfirm = vi.fn();
    const onCancel = vi.fn();
    render(
      <ConfirmDialog
        open
        title="Remove cookie?"
        body="Remove the saved cookie for Claude? This signs that provider out of Ceiling."
        confirmLabel="Remove"
        cancelLabel="Cancel"
        onConfirm={onConfirm}
        onCancel={onCancel}
      />,
    );

    expect(screen.getByRole("alertdialog", { name: "Remove cookie?" })).toHaveTextContent(
      "Remove the saved cookie for Claude?",
    );
    fireEvent.click(screen.getByRole("button", { name: "Cancel" }));
    expect(onCancel).toHaveBeenCalledOnce();
    expect(onConfirm).not.toHaveBeenCalled();
  });

  it("confirms only after the destructive action is chosen", () => {
    const onConfirm = vi.fn();
    render(
      <ConfirmDialog
        open
        title="Remove cookie?"
        body="Remove the saved cookie for Claude?"
        confirmLabel="Remove"
        cancelLabel="Cancel"
        onConfirm={onConfirm}
        onCancel={vi.fn()}
      />,
    );

    fireEvent.click(screen.getByRole("button", { name: "Remove" }));
    expect(onConfirm).toHaveBeenCalledOnce();
  });

  it("does not render when closed", () => {
    render(
      <ConfirmDialog
        open={false}
        title="Remove cookie?"
        body="Remove the saved cookie for Claude?"
        confirmLabel="Remove"
        cancelLabel="Cancel"
        onConfirm={vi.fn()}
        onCancel={vi.fn()}
      />,
    );
    expect(screen.queryByRole("alertdialog")).toBeNull();
  });

  it("keeps Tab and Shift+Tab inside the dialog", () => {
    render(
      <>
        <button type="button">outside</button>
        <ConfirmDialog
          open
          title="Remove cookie?"
          body="Remove the saved cookie for Claude?"
          confirmLabel="Remove"
          cancelLabel="Cancel"
          onConfirm={vi.fn()}
          onCancel={vi.fn()}
        />
      </>,
    );

    const cancel = screen.getByRole("button", { name: "Cancel" });
    const confirm = screen.getByRole("button", { name: "Remove" });
    expect(cancel).toHaveFocus();

    fireEvent.keyDown(window, { key: "Tab" });
    expect(confirm).toHaveFocus();
    fireEvent.keyDown(window, { key: "Tab" });
    expect(cancel).toHaveFocus();
    fireEvent.keyDown(window, { key: "Tab", shiftKey: true });
    expect(confirm).toHaveFocus();
  });
});
