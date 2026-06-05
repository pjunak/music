import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it, vi } from "vitest";

import { FolderTree } from "./FolderTree";
import type { TreeFolder } from "./FolderTree";

function loader() {
  return vi.fn(async (p: string): Promise<TreeFolder[]> => {
    if (p === "") return [{ name: "Battle", path: "Battle", hasChildren: true }];
    if (p === "Battle")
      return [{ name: "Sieges", path: "Battle/Sieges", hasChildren: false }];
    return [];
  });
}

describe("FolderTree expansion persistence", () => {
  it("keeps expanded folders open when refreshKey bumps (the no-collapse fix)", async () => {
    const loadChildren = loader();
    const onSelect = vi.fn();
    const { rerender } = render(
      <FolderTree
        selectedPath=""
        onSelect={onSelect}
        loadChildren={loadChildren}
        refreshKey={0}
      />,
    );

    // Root child loads, then we expand it and its child loads.
    await userEvent.click(await screen.findByText("Battle"));
    expect(await screen.findByText("Sieges")).toBeInTheDocument();

    // A folder op (add/rename/delete/upload) bumps refreshKey. Before the fix
    // this wiped all expansion (setState({})) and the subtree collapsed.
    rerender(
      <FolderTree
        selectedPath=""
        onSelect={onSelect}
        loadChildren={loadChildren}
        refreshKey={1}
      />,
    );

    expect(await screen.findByText("Sieges")).toBeInTheDocument();
  });
});
