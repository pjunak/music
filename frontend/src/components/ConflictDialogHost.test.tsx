import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, describe, expect, it } from "vitest";

import { ConflictDialogHost } from "./ConflictDialogHost";
import { uploadConflictDialog, useConflictStore } from "./conflictDialog";

afterEach(() => {
  // Resolve any dialog a test left open so its promise doesn't dangle.
  useConflictStore.getState().resolve(null);
});

describe("ConflictDialogHost", () => {
  it("renders nothing until a chooser is opened", () => {
    const { container } = render(<ConflictDialogHost />);
    expect(container).toBeEmptyDOMElement();
  });

  it("summarises the collisions and resolves the chosen policy", async () => {
    const choice = uploadConflictDialog({
      count: 2,
      total: 5,
      sampleNames: ["a.mp3", "b.mp3"],
    });
    render(<ConflictDialogHost />);

    expect(screen.getByText(/2 of 5 files already exist/i)).toBeInTheDocument();
    expect(screen.getByText("a.mp3")).toBeInTheDocument();

    await userEvent.click(screen.getByRole("button", { name: "Overwrite" }));
    await expect(choice).resolves.toBe("overwrite");
  });

  it("resolves skip from its button", async () => {
    const skip = uploadConflictDialog({ count: 1, total: 1, sampleNames: ["x.mp3"] });
    render(<ConflictDialogHost />);
    await userEvent.click(screen.getByRole("button", { name: "Skip existing" }));
    await expect(skip).resolves.toBe("skip");
  });

  it("resolves rename from the keep-both button", async () => {
    const keep = uploadConflictDialog({ count: 1, total: 1, sampleNames: ["x.mp3"] });
    render(<ConflictDialogHost />);
    await userEvent.click(screen.getByRole("button", { name: "Keep both" }));
    await expect(keep).resolves.toBe("rename");
  });

  it("resolves null when cancelled", async () => {
    const choice = uploadConflictDialog({ count: 1, total: 3, sampleNames: ["x.mp3"] });
    render(<ConflictDialogHost />);
    await userEvent.click(screen.getByRole("button", { name: "Cancel" }));
    await expect(choice).resolves.toBeNull();
  });

  it("says 'All' when every file collides", () => {
    void uploadConflictDialog({ count: 3, total: 3, sampleNames: [] });
    render(<ConflictDialogHost />);
    expect(screen.getByText(/All 3 files already exist/i)).toBeInTheDocument();
  });
});
