import { render, screen } from "@testing-library/react";
import { MemoryRouter } from "react-router-dom";
import { describe, expect, it } from "vitest";

import { AssistantCleanupView } from "./AssistantCleanupView";

describe("AssistantCleanupView", () => {
  it("keeps the working cleanup tool discoverable without claiming missing foundations", () => {
    render(
      <MemoryRouter>
        <AssistantCleanupView />
      </MemoryRouter>,
    );

    expect(
      screen.getByRole("heading", { name: "Cleanup remains in Library" }),
    ).toBeInTheDocument();
    expect(screen.getByText(/Durable jobs and review tools are now available/))
      .toBeInTheDocument();
    expect(screen.getByRole("link", { name: "Open Library cleanup" }))
      .toHaveAttribute("href", "/library");
  });
});
