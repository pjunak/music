import { mkdtempSync, mkdirSync, rmdirSync, unlinkSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import path from "node:path";
import { describe, expect, it } from "vitest";

import { generateCompatAssets } from "./compat-build.mts";

function withSources(source: string, verify: (root: string) => void): void {
  const root = mkdtempSync(path.join(tmpdir(), "music-compat-build-"));
  const directory = path.join(root, "compat");
  mkdirSync(directory);
  const files = ["compat-mode.ts", "boot-watchdog.ts"].map((name) => path.join(directory, name));
  try {
    for (const file of files) writeFileSync(file, source);
    verify(root);
  } finally {
    for (const file of files) unlinkSync(file);
    rmdirSync(directory);
    rmdirSync(root);
  }
}

describe("ES5 compatibility asset generation", () => {
  it("erases types without changing classic-script execution", () => {
    withSources("(function () { var result: number = 42; return result; })();", (root) => {
      const assets = generateCompatAssets(root);
      expect(assets.compatMode).not.toContain(": number");
      expect(assets.compatMode).toContain("var result");
      expect(assets.bootWatchdog).toBe(assets.compatMode);
    });
  });

  it.each([
    "(() => 42)();",
    "const result = 42;",
    "try {} catch {}",
    "export var result = 42;",
  ])("rejects unsupported old-browser syntax: %s", (source) => {
    withSources(source, (root) => {
      expect(() => generateCompatAssets(root)).toThrow(SyntaxError);
    });
  });
});
