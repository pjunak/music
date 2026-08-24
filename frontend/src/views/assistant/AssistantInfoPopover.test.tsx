import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it } from "vitest";

import { ProviderBoundaryPopover } from "./AssistantInfoPopover";

describe("ProviderBoundaryPopover", () => {
  it("keeps disclosure details behind a keyboard-operable compact summary", async () => {
    const user = userEvent.setup();
    render(
      <ProviderBoundaryPopover
        shared={["Track titles"]}
        neverShared={["Audio files"]}
        footer="Confirmation is required."
      />,
    );

    const trigger = screen.getByText("Provider boundary").closest("summary");
    const popover = trigger?.closest("details");
    expect(popover).not.toHaveAttribute("open");

    await user.click(trigger!);

    expect(popover).toHaveAttribute("open");
    expect(screen.getByText("Track titles")).toBeInTheDocument();
    expect(screen.getByText("Audio files")).toBeInTheDocument();
    expect(screen.getByText("Confirmation is required.")).toBeInTheDocument();
  });
});
