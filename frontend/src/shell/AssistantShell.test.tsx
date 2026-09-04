import { render, screen } from "@testing-library/react";
import { MemoryRouter, Route, Routes } from "react-router-dom";
import { describe, expect, it } from "vitest";

import {
  AssistantShell,
  LibraryCleanupShell,
  MoodLibraryShell,
} from "./AssistantShell";

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
    expect(screen.getByRole("link", { name: "Playlist builder" })).toHaveClass(
      "is-active",
    );
    expect(screen.getByRole("link", { name: "EQ drafts" })).toBeInTheDocument();
    expect(screen.getByRole("link", { name: "Mood library" })).toBeInTheDocument();
    expect(screen.getByRole("link", { name: "Library cleanup" })).toBeInTheDocument();
    expect(screen.getByRole("link", { name: "AI setup" })).toBeInTheDocument();
    expect(screen.queryByRole("link", { name: "Settings" })).not.toBeInTheDocument();
  });

  it("keeps cleanup execution, sources, and rollback together", () => {
    render(
      <MemoryRouter initialEntries={["/assistant/cleanup/history"]}>
        <Routes>
          <Route path="/assistant/cleanup" element={<LibraryCleanupShell />}>
            <Route path="history" element={<div>Run journals</div>} />
          </Route>
        </Routes>
      </MemoryRouter>,
    );

    expect(screen.getByRole("link", { name: "Clean up" })).toBeInTheDocument();
    expect(screen.getByRole("link", { name: "Sources" })).toBeInTheDocument();
    expect(screen.queryByRole("link", { name: "AI assistance" })).not.toBeInTheDocument();
    expect(screen.getByRole("link", { name: "History & rollback" })).toHaveClass(
      "section-nav-tab-active",
    );
    expect(screen.getByText("Run journals")).toBeVisible();
  });

  it("keeps AI setup distinct from the app-wide Settings destination", () => {
    render(
      <MemoryRouter initialEntries={["/assistant/ai"]}>
        <Routes>
          <Route path="/assistant" element={<AssistantShell />}>
            <Route path="ai" element={<div>Model setup</div>} />
          </Route>
        </Routes>
      </MemoryRouter>,
    );

    expect(screen.getByRole("link", { name: "AI setup" })).toHaveClass(
      "is-active",
    );
    expect(screen.getByText("Model setup")).toBeVisible();
  });

  it("keeps analysis, evidence, and human-owned tags in distinct Mood Library tabs", () => {
    render(
      <MemoryRouter initialEntries={["/assistant/moods/workflow"]}>
        <Routes>
          <Route path="/assistant/moods" element={<MoodLibraryShell />}>
            <Route path="workflow" element={<div>Mood workflow</div>} />
            <Route path="context" element={<div>Context browser</div>} />
            <Route path="tags" element={<div>Mood tag editor</div>} />
            <Route path="vocabulary" element={<div>Vocabulary editor</div>} />
          </Route>
        </Routes>
      </MemoryRouter>,
    );

    expect(
      screen.getByRole("link", {
        name: /Analysis/,
      }),
    ).toHaveClass("section-nav-tab-active");
    expect(
      screen.getByRole("link", {
        name: /Track context/,
      }),
    ).toBeInTheDocument();
    expect(
      screen.getByRole("link", {
        name: /Mood tags/,
      }),
    ).toBeInTheDocument();
    expect(
      screen.getByRole("link", {
        name: /Mood vocabulary/,
      }),
    ).toBeInTheDocument();
    expect(screen.getByText("Mood workflow")).toBeVisible();
  });
});
