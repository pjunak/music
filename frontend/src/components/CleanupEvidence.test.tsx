import { useState } from "react";
import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it, vi } from "vitest";
import type { CleanupImportedEvidence } from "@/core/api";
import { CleanupEditionTarget, CleanupEvidenceImport } from "./CleanupEvidence";
import { releaseIdFromInput } from "./cleanupReview";

const release = "1e18da96-4713-41d2-8c3b-17a5743736d1";

describe("source and edition review", () => {
  it("accepts release IDs and official release URLs without accepting other resources", () => {
    expect(releaseIdFromInput(release.toUpperCase())).toBe(release);
    expect(releaseIdFromInput(`https://musicbrainz.org/release/${release}/`)).toBe(release);
    for (const invalid of ["00000000-0000-0000-0000-000000000000", "../release", `https://example.com/release/${release}`, `https://musicbrainz.org/recording/${release}`, `https://user@musicbrainz.org/release/${release}`, `https://musicbrainz.org/release/${release}?x=1`, `http://musicbrainz.org/release/${release}`]) {
      expect(releaseIdFromInput(invalid)).toBeNull();
    }
  });

  it("requires valid explicit edition input before preparing the next review", async () => {
    const onTarget = vi.fn();
    const user = userEvent.setup();
    render(<CleanupEditionTarget folder="Album" disabled={false} onTarget={onTarget} />);
    await user.click(screen.getByText("Find a known album edition"));
    expect(screen.getByRole("button")).toBeDisabled();
    const input = screen.getByRole("textbox");
    await user.type(input, "invalid");
    expect(screen.getByRole("alert")).toBeInTheDocument();
    await user.clear(input);
    await user.type(input, `https://musicbrainz.org/release/${release}`);
    expect(onTarget).not.toHaveBeenCalled();
    await user.click(screen.getByRole("button"));
    expect(onTarget).toHaveBeenCalledWith(release);
  });

  it("keeps imported evidence passive until the operator supplies a source and opts into proposals", async () => {
    function Import() {
      const [items, setItems] = useState<CleanupImportedEvidence[]>([{ track_id: 7, fields: { title: "Source title" } }]);
      return <><CleanupEvidenceImport imports={items} onChange={setItems} /><output>{JSON.stringify(items)}</output></>;
    }
    const user = userEvent.setup();
    render(<Import />);
    const propose = screen.getByRole("checkbox", { name: "Propose imported metadata changes for review" });
    expect(propose).not.toBeChecked();
    expect(propose).toBeDisabled();
    const source = screen.getByRole("textbox", { name: "Source reference for imported metadata" });
    await user.type(source, "Creator tracklist");
    expect(propose).toBeEnabled();
    expect(propose).not.toBeChecked();
    await user.click(propose);
    expect(JSON.parse(screen.getByRole("status").textContent!)[0]).toMatchObject({ source: "Creator tracklist", propose: true });
    await user.clear(source);
    expect(propose).not.toBeChecked();
    expect(propose).toBeDisabled();
  });
});
