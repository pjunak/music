import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it, vi } from "vitest";
import type { CleanupEnrichmentPlan, CleanupModelResult, CleanupReleaseChoice } from "@/core/api";
import { CleanupEditionComparison } from "./CleanupEditionComparison";

vi.mock("./CleanupModelReview", () => ({ CleanupModelReview: ({ onResult }: { onResult: (result: CleanupModelResult) => void }) =>
  <button onClick={() => onResult({ schema_version: "assistant-library-cleanup-result/v1", mode: "edition", track_id: 1,
    recommended_release_id: "mp3", decision: { decision: "select", reason: "Streets and Faces distinguishes this release.", candidate_id: "candidate-1", evidence_ids: [] }, ops: [] })}>Finish AI advice</button> }));

const choices: CleanupReleaseChoice[] = ["wav", "mp3"].map((id) => ({
  id, title: "Soundtrack", artist: "Composer", date: "2023-08-03", country: "XW", barcode: null, catalog_numbers: [], ops: [],
  assignment: { classification: "album", matched: 1, considered: 43, unmatched_tracks: [], unmatched_slots: [] },
  edition_review: { release_id: id, compared_release_ids: ["wav", "mp3"], title: "Soundtrack", description: id === "wav" ? "Steam, GOG.com WAV" : "GOG.com MP3",
    formats: ["Digital Media"], labels: [], folder_tracks: 43, compared_tracks: 43, release_tracks: 43, title_matches: id === "wav" ? 42 : 43,
    duration_matches: id === "wav" ? 42 : 43, duration_conflicts: 0, complete: true, recommended: id === "mp3",
    distinguishing_tracks: [{ title: id === "wav" ? "Lead Your Way" : "Streets and Faces", disc_no: 1, track_no: 30, present: id === "mp3", duration_agrees: id === "mp3" }],
    distinguishing_tracks_total: 1, missing_titles: [], missing_titles_total: 0 },
}));
const plan: CleanupEnrichmentPlan = { schema: "library-cleanup-enrichment/v1", track_id: 1, path: "Album/one.mp3", status: "identified", identity: null, ops: [], notes: [], tag_suggestions: [], release_choices: choices };

describe("edition comparison", () => {
  it("shows descriptions and distinguishing songs once per folder; AI advice requires explicit use", async () => {
    const onEdition = vi.fn(); const user = userEvent.setup();
    render(<CleanupEditionComparison plans={[plan, { ...plan, track_id: 2 }]} edition="" catalogJobId="job" disabled={false} onEdition={onEdition} />);
    expect(screen.getAllByRole("combobox")).toHaveLength(1);
    expect(screen.getByRole("option", { name: /GOG.com MP3.*Worldwide/ })).toBeInTheDocument();
    await user.click(screen.getByText("Compare editions and tracklist differences"));
    expect(screen.getByText("Streets and Faces")).toBeInTheDocument();
    expect(screen.getByText("Lead Your Way")).toBeInTheDocument();
    expect(screen.getByText("Best supported by the retrieved tracklists.")).toBeInTheDocument();
    expect(screen.getAllByText(/Strict assignment.*1\/43/)).toHaveLength(2);
    await user.click(screen.getByRole("button", { name: "Finish AI advice" }));
    expect(onEdition).not.toHaveBeenCalled();
    await user.click(screen.getByRole("button", { name: "Use suggested edition: GOG.com MP3" }));
    expect(onEdition).toHaveBeenCalledExactlyOnceWith("mp3");
  });
  it("explains old results and does not offer AI without comparison evidence", async () => {
    const old = { ...plan, release_choices: choices.map(({ edition_review: _review, ...choice }) => choice) };
    render(<CleanupEditionComparison plans={[old]} edition="" catalogJobId="job" disabled={false} onEdition={vi.fn()} />);
    await userEvent.click(screen.getByText("Compare editions and tracklist differences"));
    expect(screen.getAllByText(/predates edition comparisons/)).toHaveLength(2);
    expect(screen.queryByRole("button", { name: "Finish AI advice" })).not.toBeInTheDocument();
  });
});
