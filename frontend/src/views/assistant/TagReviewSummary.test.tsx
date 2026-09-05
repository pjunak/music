import { render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it } from "vitest";

import type { TagReviewSummary as Summary } from "@/core/api";

import { TagReviewSummary } from "./TagReviewSummary";

describe("TagReviewSummary", () => {
  it("shows source-specific decisions and explains the denominator", async () => {
    render(<TagReviewSummary summary={{ matching_tracks: 200, sources: [
      { analyzer_id: "local-metadata/v1", pending: 5, accepted: 2, rejected: 1 },
      { analyzer_id: "model-context-tagger/v6", pending: 8, accepted: 4, rejected: 0 },
    ] }} />);
    await userEvent.click(screen.getByText("Review summary · 7 of 20 suggestions reviewed"));
    const table = screen.getByRole("table", { name: "Current suggestion review counts" });
    expect(within(table).getByRole("row", { name: "local-metadata/v1 5 2 1" })).toBeInTheDocument();
    expect(within(table).getByRole("row", { name: "model-context-tagger/v6 8 4 0" })).toBeInTheDocument();
    expect(screen.getByText(/200 matching tracks, including all pages and review states/)).toBeInTheDocument();
    expect(screen.getByText(/not model accuracy or lifetime history/)).toBeInTheDocument();
  });

  it("shows an empty scope without inventing a review rate", async () => {
    render(<TagReviewSummary summary={{ matching_tracks: 0, sources: [] }} />);
    await userEvent.click(screen.getByText("Review summary · 0 of 0 suggestions reviewed"));
    expect(screen.getByText("No current suggestions in this scope.")).toBeInTheDocument();
    expect(screen.queryByRole("table")).not.toBeInTheDocument();
  });

  it.each([
    undefined, null, {}, { matching_tracks: -1, sources: [] },
    { matching_tracks: 1, sources: [null] },
    { matching_tracks: 1, sources: [{ analyzer_id: "source", pending: -1, accepted: 0, rejected: 0 }] },
    { matching_tracks: 1, sources: [{ analyzer_id: "source", pending: 0.5, accepted: 0, rejected: 0 }] },
    { matching_tracks: 1, sources: [{ analyzer_id: "source", pending: Number.MAX_SAFE_INTEGER, accepted: 1, rejected: 0 }] },
    { matching_tracks: 1, sources: Array(2).fill({ analyzer_id: "source", pending: 1, accepted: 0, rejected: 0 }) },
  ])("omits unavailable or malformed counts (%j)", (summary) => {
    const { container } = render(<TagReviewSummary summary={summary as Summary | undefined} />);
    expect(container).toBeEmptyDOMElement();
  });
});
