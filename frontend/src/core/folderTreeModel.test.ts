import { describe, expect, it } from "vitest";

import {
  ancestorsOf,
  buildFolderIndex,
  filterFolders,
  foldedMatch,
  foldName,
  parentPath,
} from "./folderTreeModel";
import type { FolderNode } from "./folderTreeModel";

const F = (path: string): FolderNode => ({
  name: path.split("/").pop() ?? path,
  path,
});

describe("path helpers", () => {
  it("parentPath returns '' at root", () => {
    expect(parentPath("Games")).toBe("");
    expect(parentPath("Games/Skyrim")).toBe("Games");
  });

  it("ancestorsOf lists outermost-first, excluding self", () => {
    expect(ancestorsOf("a/b/c")).toEqual(["a", "a/b"]);
    expect(ancestorsOf("a")).toEqual([]);
  });
});

describe("buildFolderIndex", () => {
  it("groups children under their parent, numerically aware", () => {
    const idx = buildFolderIndex([
      F("Album/Disc 10"),
      F("Album/Disc 2"),
      F("Album"),
      F("Zelda"),
    ]);
    expect(idx.childrenOf.get("")?.map((f) => f.name)).toEqual(["Album", "Zelda"]);
    // "Disc 2" before "Disc 10" — plain string sort would invert these.
    expect(idx.childrenOf.get("Album")?.map((f) => f.name)).toEqual([
      "Disc 2",
      "Disc 10",
    ]);
  });
});

describe("foldName / foldedMatch", () => {
  it("folds case and diacritics", () => {
    expect(foldName("Dvořák")).toBe("dvorak");
  });

  it("maps the folded match back to original indices", () => {
    // "řá" folds to "ra"; the highlight range must cover the original chars.
    const m = foldedMatch("Dvořák", "orak");
    expect(m).not.toBeNull();
    const [start, end] = m as [number, number];
    expect("Dvořák".slice(start, end)).toBe("ořák");
  });

  it("returns null when there is no match or the query is empty", () => {
    expect(foldedMatch("Skyrim", "witcher")).toBeNull();
    expect(foldedMatch("Skyrim", "")).toBeNull();
  });
});

describe("filterFolders", () => {
  it("keeps matches plus their ancestors only", () => {
    const folders = [
      F("Games"),
      F("Games/Skyrim"),
      F("Games/Witcher"),
      F("Movies"),
    ];
    const r = filterFolders(folders, "sky");
    expect(r.matches).toEqual(new Set(["Games/Skyrim"]));
    expect(r.visible).toEqual(new Set(["Games", "Games/Skyrim"]));
  });
});
