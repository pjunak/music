import { execFileSync } from "node:child_process";
import { existsSync } from "node:fs";
import { fileURLToPath } from "node:url";
import path from "node:path";

export function javascriptSources(repositoryRoot: string): string[] {
  // Include untracked additions, but let Git exclude dependencies/build output.
  // A tracked generated file still fails: generated JavaScript is not source.
  const files = execFileSync(
    "git", ["ls-files", "--cached", "--others", "--exclude-standard", "-z"],
    { cwd: repositoryRoot, encoding: "utf8" },
  ).split("\0");
  return [...new Set(files)].filter((file) =>
    /\.(?:[cm]?js|jsx)$/i.test(file) && existsSync(path.join(repositoryRoot, file)),
  ).sort();
}

export function checkSource(repositoryRoot: string): void {
  const files = javascriptSources(repositoryRoot);
  if (files.length > 0) {
    throw new Error(`JavaScript source is not allowed; use TypeScript:\n${files.join("\n")}`);
  }
}

if (process.argv[1] && path.resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  checkSource(fileURLToPath(new URL("../../", import.meta.url)));
  console.log("Source boundary: no tracked or untracked JavaScript source");
}
