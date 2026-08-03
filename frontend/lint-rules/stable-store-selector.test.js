import { spawnSync } from "node:child_process";
import {
  mkdtempSync,
  readFileSync,
  rmdirSync,
  unlinkSync,
  writeFileSync,
} from "node:fs";
import { tmpdir } from "node:os";
import path from "node:path";
import { fileURLToPath } from "node:url";

import { describe, expect, it } from "vitest";

const ruleDirectory = path.dirname(fileURLToPath(import.meta.url));
const frontendRoot = path.resolve(ruleDirectory, "..");
const oxlintBin = path.join(frontendRoot, "node_modules", "oxlint", "bin", "oxlint");
const config = path.join(ruleDirectory, "test.oxlintrc.json");

function lintFixture(name) {
  const source = readFileSync(
    path.join(ruleDirectory, "fixtures", `${name}.txt`),
    "utf8",
  );
  const temporaryDirectory = mkdtempSync(path.join(tmpdir(), "music-oxlint-"));
  const fixture = path.join(temporaryDirectory, `${name}.ts`);
  writeFileSync(fixture, source);

  const result = spawnSync(process.execPath, [oxlintBin, "--config", config, fixture], {
    cwd: frontendRoot,
    encoding: "utf8",
  });

  unlinkSync(fixture);
  rmdirSync(temporaryDirectory);

  if (result.error) throw result.error;
  return {
    status: result.status,
    output: `${result.stdout}${result.stderr}`,
  };
}

describe("local/stable-store-selector", () => {
  it("accepts selectors that return stable references or primitives", () => {
    const result = lintFixture("stable-store-selector.valid");

    expect(result.output).toBe("");
    expect(result.status).toBe(0);
  });

  it("rejects fresh arrays and objects returned by store selectors", () => {
    const result = lintFixture("stable-store-selector.invalid");

    expect(result.status).toBe(1);
    expect(result.output.match(/local\(stable-store-selector\)/g)).toHaveLength(5);
  });
});
