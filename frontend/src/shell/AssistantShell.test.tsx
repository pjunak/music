import { render, screen } from "@testing-library/react";
import { MemoryRouter, Route, Routes } from "react-router-dom";
import { describe, expect, it } from "vitest";

import { AssistantShell, MoodLibraryShell } from "./AssistantShell";

describe("AssistantShell", () => {
  it("shows only working Assistant sections", () => {
    render(
      <MemoryRouter initialEntries={["/assistant/playlists"]}>
        <Routes>
          <Route path="/assistant" element={<AssistantShell />}>
            <Route path="playlists" element={<div>Playlist workspace</div>} />
          </Route>
        </Routes>
      </MemoryRouter>,
    );

    expect(screen.getAllByRole("link")).toHaveLength(4);
    expect(screen.getByRole("link", { name: "Playlist Builder" })).toBeInTheDocument();
    expect(screen.getByRole("link", { name: "EQ Assistant" })).toBeInTheDocument();
    expect(screen.getByRole("link", { name: "Mood Library" })).toBeInTheDocument();
    expect(screen.getByRole("link", { name: "Settings" })).toBeInTheDocument();
    expect(screen.queryByRole("link", { name: "Cleanup" })).not.toBeInTheDocument();
  });

  it("keeps analysis and evidence inside one Mood Library workspace", () => {
    render(
      <MemoryRouter initialEntries={["/assistant/moods/workflow"]}>
        <Routes>
          <Route path="/assistant/moods" element={<MoodLibraryShell />}>
            <Route path="workflow" element={<div>Mood workflow</div>} />
            <Route path="context" element={<div>Context browser</div>} />
          </Route>
        </Routes>
      </MemoryRouter>,
    );

    expect(
      screen.getByRole("link", {
        name: /Analyze and tag/,
      }),
    ).toHaveClass("is-active");
    expect(
      screen.getByRole("link", {
        name: /Track context/,
      }),
    ).toBeInTheDocument();
    expect(screen.getByText("Mood workflow")).toBeVisible();
  });
});
