import { render, screen } from "@testing-library/react";
import { MemoryRouter, Route, Routes } from "react-router-dom";
import { describe, expect, it } from "vitest";

import { AssistantShell } from "./AssistantShell";

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

    expect(screen.getAllByRole("link")).toHaveLength(5);
    expect(screen.getByRole("link", { name: "Playlist Builder" })).toBeInTheDocument();
    expect(screen.getByRole("link", { name: "EQ Assistant" })).toBeInTheDocument();
    expect(screen.getByRole("link", { name: "Library Analysis" })).toBeInTheDocument();
    expect(screen.getByRole("link", { name: "Tag Vocabulary" })).toBeInTheDocument();
    expect(screen.getByRole("link", { name: "AI Setup" })).toBeInTheDocument();
    expect(screen.queryByRole("link", { name: "Cleanup" })).not.toBeInTheDocument();
  });
});
