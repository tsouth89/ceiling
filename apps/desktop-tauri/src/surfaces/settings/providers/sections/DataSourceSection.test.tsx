import { render, screen } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import { DataSourceSection } from "./DataSourceSection";
import type { ProviderDetail } from "../../../../types/bridge";

vi.mock("../../../../lib/tauri", () => ({
  openExternalUrl: vi.fn(() => Promise.resolve()),
}));

const t = (k: string) => k;

function provider(id: string): ProviderDetail {
  return { id, displayName: id } as unknown as ProviderDetail;
}

describe("DataSourceSection", () => {
  it("shows provider-specific provenance for a first-class provider", () => {
    render(<DataSourceSection provider={provider("claude")} t={t} />);
    expect(screen.getByText("DataSourceClaude")).toBeInTheDocument();
    expect(screen.getByText("DataSourcePrivacyNote")).toBeInTheDocument();
    expect(screen.getByText("DataSourceLearnMore")).toBeInTheDocument();
  });

  it("falls back to the generic line for providers without dedicated copy", () => {
    render(<DataSourceSection provider={provider("openrouter")} t={t} />);
    expect(screen.getByText("DataSourceGeneric")).toBeInTheDocument();
  });
});
