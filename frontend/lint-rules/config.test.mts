import { spawnSync } from "node:child_process";
import { mkdtempSync, rmdirSync, unlinkSync, writeFileSync } from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { describe, expect, it } from "vitest";

const frontendRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const repositoryRoot = path.resolve(frontendRoot, "..");
const binary = path.join(frontendRoot, "node_modules", "oxlint", "bin", "oxlint");

function lintProbe(parent: string, extension: string, source: string) {
  // Put the probe inside the real TS project so this tests type-aware discovery,
  // file overrides, and the same configuration invoked by CI.
  const directory = mkdtempSync(path.join(parent, ".lint-probe-"));
  const file = path.join(directory, `probe.${extension}`);
  try {
    writeFileSync(file, source);
    const result = spawnSync(process.execPath, [binary,
      "--config", path.join(repositoryRoot, ".oxlintrc.json"),
      "--deny-warnings", "--report-unused-disable-directives", file,
    ], { cwd: repositoryRoot, encoding: "utf8" });
    if (result.error) throw result.error;
    return { status: result.status, output: result.stdout + result.stderr };
  } finally {
    unlinkSync(file);
    rmdirSync(directory);
  }
}

describe("repository Oxlint configuration", () => {
  it.each(["src", "../tools", "../.github/scripts", "scripts", "lint-rules"])(
    "checks unhandled promises in %s",
    (directory) => {
      const result = lintProbe(path.resolve(frontendRoot, directory), "mts",
        'export async function operation() { return 1; }\noperation();\n');
      expect(result.status).toBe(1);
      expect(result.output).toContain("typescript(no-floating-promises)");
    },
  );

  it("accepts an explicitly awaited operation", () => {
    const result = lintProbe(path.join(repositoryRoot, "tools"), "mts",
      'export async function operation() { return 1; }\nawait operation();\n');
    expect(result.status, result.output).toBe(0);
  });

  it("enforces React render purity and hook dependencies", () => {
    const result = lintProbe(path.join(frontendRoot, "src"), "tsx",
      'import { useEffect } from "react";\n' +
      'export function Probe({value}: {value: string}) {\n' +
      'useEffect(() => { console.log(value); }, []);\n' +
      'return <span>{Date.now()}</span>;\n}\n');
    expect(result.status).toBe(1);
    expect(result.output).toContain("(purity)");
    expect(result.output).toContain("(exhaustive-deps)");
  });

  it("rejects unused suppression directives", () => {
    const result = lintProbe(path.join(repositoryRoot, "tools"), "mts",
      '// oxlint-disable-next-line no-debugger\nexport const answer = 42;\n');
    expect(result.status).toBe(1);
    expect(result.output).toMatch(/unused.*directive/i);
  });
});
