import { execFileSync } from "node:child_process";
import { mkdtempSync, mkdirSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import path from "node:path";
import { describe, expect, it } from "vitest";

import { checkSource, javascriptSources } from "./check-source.mts";

describe("TypeScript source boundary", () => {
  it("rejects tracked and new JavaScript while ignoring generated output", () => {
    const directory = mkdtempSync(path.join(tmpdir(), "music-source-gate-"));
    if (path.dirname(path.resolve(directory)) !== path.resolve(tmpdir()) ||
        !path.basename(directory).startsWith("music-source-gate-")) {
      throw new Error("Unexpected temporary directory");
    }
    try {
      execFileSync("git", ["init", "--quiet", directory]);
      writeFileSync(path.join(directory, ".gitignore"), "dist/\n");
      mkdirSync(path.join(directory, "dist"));
      writeFileSync(path.join(directory, "dist", "generated.js"), "debugger;");
      writeFileSync(path.join(directory, "valid.mts"), "export const answer: number = 42;");
      expect(javascriptSources(directory)).toEqual([]);
      expect(() => checkSource(directory)).not.toThrow();

      writeFileSync(path.join(directory, "tracked.js"), "debugger;");
      execFileSync("git", ["add", "tracked.js"], { cwd: directory });
      writeFileSync(path.join(directory, "new.mjs"), "debugger;");
      writeFileSync(path.join(directory, "new.cjs"), "debugger;");
      writeFileSync(path.join(directory, "new.jsx"), "debugger;");
      expect(javascriptSources(directory)).toEqual(["new.cjs", "new.jsx", "new.mjs", "tracked.js"]);
      expect(() => checkSource(directory)).toThrow("JavaScript source is not allowed");

      rmSync(path.join(directory, "tracked.js"));
      expect(javascriptSources(directory)).not.toContain("tracked.js");
      execFileSync("git", ["add", "--force", "dist/generated.js"], { cwd: directory });
      expect(javascriptSources(directory)).toContain("dist/generated.js");
    } finally {
      rmSync(directory, { recursive: true, force: true });
    }
  });
});
