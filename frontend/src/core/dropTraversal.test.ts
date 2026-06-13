import { describe, expect, it } from "vitest";

import { groupByParent, isAudioFile } from "./dropTraversal";
import type { CollectedFile } from "./dropTraversal";

/** The folder-upload path's correctness hinges on two pure helpers: which
 *  files we keep (isAudioFile) and how we bucket them into destination
 *  subfolders (groupByParent). The async FileSystemEntry walk needs a real
 *  browser to exercise, but these two we can pin down here. */

function collected(relativePath: string): CollectedFile {
  // The File contents don't matter for grouping — only the relative path.
  return {
    relativePath,
    file: new File(["x"], relativePath.split("/").pop() ?? relativePath),
  };
}

describe("isAudioFile", () => {
  it("accepts known audio extensions case-insensitively", () => {
    for (const name of ["a.mp3", "b.FLAC", "c.Ogg", "d.opus", "e.m4a", "f.wav"]) {
      expect(isAudioFile(name)).toBe(true);
    }
  });

  it("rejects non-audio files and extensionless names", () => {
    for (const name of ["cover.jpg", "album.nfo", "readme.txt", "noext"]) {
      expect(isAudioFile(name)).toBe(false);
    }
  });
});

describe("groupByParent", () => {
  it("buckets loose files under the empty-string root", () => {
    const groups = groupByParent([collected("a.mp3"), collected("b.mp3")]);
    expect([...groups.keys()]).toEqual([""]);
    expect(groups.get("")).toHaveLength(2);
  });

  it("preserves a single dropped-folder structure", () => {
    const groups = groupByParent([
      collected("MyAlbum/01.mp3"),
      collected("MyAlbum/02.mp3"),
    ]);
    expect([...groups.keys()]).toEqual(["MyAlbum"]);
    expect(groups.get("MyAlbum")).toHaveLength(2);
  });

  it("splits nested folders (multi-disc album) into separate destinations", () => {
    const groups = groupByParent([
      collected("MyAlbum/Disc 1/01.mp3"),
      collected("MyAlbum/Disc 1/02.mp3"),
      collected("MyAlbum/Disc 2/01.mp3"),
    ]);
    expect(new Set(groups.keys())).toEqual(
      new Set(["MyAlbum/Disc 1", "MyAlbum/Disc 2"]),
    );
    expect(groups.get("MyAlbum/Disc 1")).toHaveLength(2);
    expect(groups.get("MyAlbum/Disc 2")).toHaveLength(1);
  });

  it("handles a mix of loose files and a folder in one drop", () => {
    const groups = groupByParent([
      collected("loose.mp3"),
      collected("MyAlbum/01.mp3"),
    ]);
    expect(new Set(groups.keys())).toEqual(new Set(["", "MyAlbum"]));
  });
});
