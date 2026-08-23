import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { LocaleProvider } from "../../../i18n/LocaleProvider";
import { buildBundle } from "../../../test/localeHarness";
import { TEST_PROVIDER_CATALOG } from "../../../test/providerCatalog";
import {
  ProvidersSidebar,
  type ProviderSidebarRow,
} from "./ProvidersSidebar";

const tauriMocks = vi.hoisted(() => ({
  getLocaleStrings: vi.fn(),
  setUiLanguage: vi.fn(),
}));

const eventMocks = vi.hoisted(() => ({
  listen: vi.fn(),
}));

vi.mock("../../../lib/tauri", async (importOriginal) => ({
  ...(await importOriginal<typeof import("../../../lib/tauri")>()),
  ...tauriMocks,
}));
vi.mock("@tauri-apps/api/event", () => eventMocks);

function rows(): ProviderSidebarRow[] {
  return TEST_PROVIDER_CATALOG.map(([id, displayName], index) => ({
    id,
    displayName,
    enabled: index < 2,
    status: index < 2 ? "ok" : "disabled",
    subtitlePrimary: index < 2 ? "auto" : "Disabled - auto",
    subtitleSecondary: index < 2 ? `${index + 1}%` : undefined,
  }));
}

describe("ProvidersSidebar", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    tauriMocks.getLocaleStrings.mockResolvedValue(buildBundle({
      ProviderSidebarSearch: "Search",
      ProviderSidebarNoMatches: "No matching providers",
      ProviderSidebarMoveUp: "Move up",
      ProviderSidebarMoveDown: "Move down",
    }));
    eventMocks.listen.mockResolvedValue(() => {});
  });

  it("renders the full provider catalog without dropping rows", async () => {
    const { container } = render(
      <LocaleProvider>
        <ProvidersSidebar
          providers={rows()}
          selectedId="codex"
          searchText=""
          onSearchTextChange={vi.fn()}
          onSelect={vi.fn()}
          onReorder={vi.fn()}
          onToggleEnabled={vi.fn()}
        />
      </LocaleProvider>,
    );

    expect(await screen.findByRole("list", { name: "Providers" })).toBeInTheDocument();
    expect(screen.getAllByRole("listitem")).toHaveLength(TEST_PROVIDER_CATALOG.length);
    expect(screen.queryByRole("option")).toBeNull();
    const names = Array.from(
      container.querySelectorAll(".providers-sidebar__name"),
      (node) => node.textContent,
    );
    expect(names).toEqual(TEST_PROVIDER_CATALOG.map(([, displayName]) => displayName));
  });

  it("renders provider search and empty matches state", async () => {
    render(
      <LocaleProvider>
        <ProvidersSidebar
          providers={[]}
          selectedId={null}
          searchText="zzzz"
          onSearchTextChange={vi.fn()}
          onSelect={vi.fn()}
          onReorder={vi.fn()}
          onToggleEnabled={vi.fn()}
        />
      </LocaleProvider>,
    );

    expect(await screen.findByRole("searchbox", { name: "Search" })).toBeInTheDocument();
    expect(screen.getByText("No matching providers")).toBeInTheDocument();
  });

  it("reorders providers through explicit move buttons", async () => {
    const onReorder = vi.fn();
    const { container } = render(
      <LocaleProvider>
        <ProvidersSidebar
          providers={rows()}
          selectedId="codex"
          searchText=""
          onSearchTextChange={vi.fn()}
          onSelect={vi.fn()}
          onReorder={onReorder}
          onToggleEnabled={vi.fn()}
        />
      </LocaleProvider>,
    );

    fireEvent.click(await screen.findByRole("button", { name: "Move down Codex" }));

    await waitFor(() => {
      const names = Array.from(
        container.querySelectorAll(".providers-sidebar__name"),
        (node) => node.textContent,
      );
      expect(names.slice(0, 3)).toEqual(["Claude", "Codex", "Cursor"]);
    });
    expect(onReorder).toHaveBeenCalledWith([
      "claude",
      "codex",
      ...TEST_PROVIDER_CATALOG.slice(2).map(([id]) => id),
    ]);
  });

  it("keeps an optimistic reorder when parent props are recreated", async () => {
    const onReorder = vi.fn();
    const { container, rerender } = render(
      <LocaleProvider>
        <ProvidersSidebar
          providers={rows()}
          selectedId="codex"
          searchText=""
          onSearchTextChange={vi.fn()}
          onSelect={vi.fn()}
          onReorder={onReorder}
          onToggleEnabled={vi.fn()}
        />
      </LocaleProvider>,
    );

    fireEvent.click(await screen.findByRole("button", { name: "Move down Codex" }));
    rerender(
      <LocaleProvider>
        <ProvidersSidebar
          providers={rows()}
          selectedId="codex"
          searchText=""
          onSearchTextChange={vi.fn()}
          onSelect={vi.fn()}
          onReorder={onReorder}
          onToggleEnabled={vi.fn()}
        />
      </LocaleProvider>,
    );

    await waitFor(() => {
      const names = Array.from(
        container.querySelectorAll(".providers-sidebar__name"),
        (node) => node.textContent,
      );
      expect(names.slice(0, 3)).toEqual(["Claude", "Codex", "Cursor"]);
    });
  });

  it("does not let the first provider move up", async () => {
    render(
      <LocaleProvider>
        <ProvidersSidebar
          providers={rows()}
          selectedId="codex"
          searchText=""
          onSearchTextChange={vi.fn()}
          onSelect={vi.fn()}
          onReorder={vi.fn()}
          onToggleEnabled={vi.fn()}
        />
      </LocaleProvider>,
    );

    expect(await screen.findByRole("button", { name: "Move up Codex" })).toBeDisabled();
  });

  it("keeps selection, enable, and reorder as sibling controls", async () => {
    const onSelect = vi.fn();
    const onToggleEnabled = vi.fn();
    const onReorder = vi.fn();
    render(
      <LocaleProvider>
        <ProvidersSidebar
          providers={rows()}
          selectedId="codex"
          searchText=""
          onSearchTextChange={vi.fn()}
          onSelect={onSelect}
          onReorder={onReorder}
          onToggleEnabled={onToggleEnabled}
        />
      </LocaleProvider>,
    );

    const select = (await screen.findByText("Codex")).closest("button");
    expect(select).not.toBeNull();
    const enabled = screen.getByRole("checkbox", { name: "Codex enabled" });
    const down = screen.getByRole("button", { name: "Move down Codex" });
    const row = select!.closest("li");
    expect(row).toContainElement(enabled);
    expect(row).toContainElement(down);
    expect(select).not.toContainElement(enabled);
    expect(select).not.toContainElement(down);

    fireEvent.click(select!);
    fireEvent.click(enabled);
    fireEvent.click(down);
    expect(onSelect).toHaveBeenCalledWith("codex");
    expect(onToggleEnabled).toHaveBeenCalledWith("codex", false);
    expect(onReorder).toHaveBeenCalled();
  });
});
