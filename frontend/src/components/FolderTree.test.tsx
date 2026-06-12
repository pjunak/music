import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it, vi } from "vitest";

import { FolderTree } from "./FolderTree";
import type { TreeFolder } from "./FolderTree";

const FOLDERS: TreeFolder[] = [
  { name: "Battle", path: "Battle" },
  { name: "Sieges", path: "Battle/Sieges" },
  { name: "Dvořák", path: "Dvořák" },
  { name: "Games", path: "Games" },
  { name: "Skyrim", path: "Games/Skyrim" },
  { name: "Taverns", path: "Taverns" },
];

const loader = () => vi.fn(async (): Promise<TreeFolder[]> => FOLDERS);

describe("FolderTree", () => {
  it("renders roots collapsed; clicking a row selects and expands it", async () => {
    const onSelect = vi.fn();
    render(<FolderTree selectedPath="" onSelect={onSelect} loadAll={loader()} />);
    expect(await screen.findByText("Battle")).toBeInTheDocument();
    expect(screen.queryByText("Sieges")).not.toBeInTheDocument();

    await userEvent.click(screen.getByText("Battle"));
    expect(onSelect).toHaveBeenCalledWith("Battle");
    expect(await screen.findByText("Sieges")).toBeInTheDocument();
  });

  it("keeps expanded folders open when refreshKey bumps (the no-collapse fix)", async () => {
    const loadAll = loader();
    const onSelect = vi.fn();
    const { rerender } = render(
      <FolderTree selectedPath="" onSelect={onSelect} loadAll={loadAll} refreshKey={0} />,
    );
    await userEvent.click(await screen.findByText("Battle"));
    expect(await screen.findByText("Sieges")).toBeInTheDocument();

    // A folder op (add/rename/delete/upload) bumps refreshKey: the list
    // re-fetches but the open subtrees must stay open.
    rerender(
      <FolderTree selectedPath="" onSelect={onSelect} loadAll={loadAll} refreshKey={1} />,
    );
    expect(await screen.findByText("Sieges")).toBeInTheDocument();
    expect(loadAll).toHaveBeenCalledTimes(2);
  });

  it("filter shows matches plus their ancestors and hides the rest", async () => {
    render(<FolderTree selectedPath="" onSelect={vi.fn()} loadAll={loader()} />);
    await screen.findByText("Battle");

    await userEvent.type(screen.getByLabelText("Filter folders"), "sky");
    // Match + its ancestor for context; everything else gone.
    expect(screen.getByTitle("Games/Skyrim")).toBeInTheDocument();
    expect(screen.getByTitle("Games")).toBeInTheDocument();
    expect(screen.queryByTitle("Battle")).not.toBeInTheDocument();
    expect(screen.queryByTitle("Taverns")).not.toBeInTheDocument();
    expect(screen.getByText(/1 of 6 folders match/)).toBeInTheDocument();
  });

  it("filter matching is diacritic-insensitive", async () => {
    render(<FolderTree selectedPath="" onSelect={vi.fn()} loadAll={loader()} />);
    await screen.findByText("Battle");

    await userEvent.type(screen.getByLabelText("Filter folders"), "dvor");
    expect(screen.getByTitle("Dvořák")).toBeInTheDocument();
    expect(screen.queryByTitle("Games")).not.toBeInTheDocument();
  });

  it("auto-reveals the selected path by expanding its ancestors", async () => {
    render(
      <FolderTree selectedPath="Games/Skyrim" onSelect={vi.fn()} loadAll={loader()} />,
    );
    // No clicks: the selection alone must make the nested row visible.
    expect(await screen.findByTitle("Games/Skyrim")).toBeInTheDocument();
  });

  it("supports arrow keys, Enter to select, and type-ahead", async () => {
    const onSelect = vi.fn();
    render(<FolderTree selectedPath="" onSelect={onSelect} loadAll={loader()} />);
    const first = await screen.findByTitle("Battle");

    first.focus();
    await userEvent.keyboard("{ArrowDown}{Enter}");
    expect(onSelect).toHaveBeenLastCalledWith("Dvořák");

    // Type-ahead jumps to the next folder starting with the typed letter.
    await userEvent.keyboard("t{Enter}");
    expect(onSelect).toHaveBeenLastCalledWith("Taverns");
  });
});
