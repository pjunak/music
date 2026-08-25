import { render, screen } from "@testing-library/react";
import { MemoryRouter, Route, Routes } from "react-router-dom";
import { describe, expect, it } from "vitest";

import {
  AssistantSettingsShell,
  AssistantShell,
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

    expect(screen.getAllByRole("link")).toHaveLength(4);
    expect(screen.getByRole("link", { name: "Playlist builder" })).toHaveClass(
      "is-active",
    );
    expect(screen.getByRole("link", { name: "EQ drafts" })).toBeInTheDocument();
    expect(screen.getByRole("link", { name: "Mood library" })).toBeInTheDocument();
    expect(screen.getByRole("link", { name: "Assistant setup" })).toBeInTheDocument();
    expect(screen.queryByRole("link", { name: "Settings" })).not.toBeInTheDocument();
    expect(screen.queryByRole("link", { name: "Cleanup" })).not.toBeInTheDocument();
  });

  it("keeps Assistant setup distinct from the app-wide Settings destination", () => {
    render(
      <MemoryRouter initialEntries={["/assistant/settings/models"]}>
        <Routes>
          <Route path="/assistant" element={<AssistantShell />}>
            <Route path="settings" element={<AssistantSettingsShell />}>
              <Route path="models" element={<div>Model setup</div>} />
            </Route>
          </Route>
        </Routes>
      </MemoryRouter>,
    );

    expect(screen.getByRole("link", { name: "Assistant setup" })).toHaveClass(
      "is-active",
    );
    expect(screen.getByRole("link", { name: "Models and providers" })).toHaveClass(
      "section-nav-tab-active",
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
    expect(screen.getByText("Mood workflow")).toBeVisible();
  });
});
