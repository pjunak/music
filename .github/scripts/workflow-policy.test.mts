import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";
import { runInNewContext } from "node:vm";

const workflow = readFileSync(new URL("../workflows/verify.yml", import.meta.url), "utf8").replace(/\r\n/g, "\n");
const container = workflow.split("\n  container:\n")[1]?.split("\n  fuzz-smoke:\n")[0];
const expression = container?.match(/\n {4}if: \$\{\{ (.+) \}\}/)?.[1];

const cases: ReadonlyArray<readonly [string, string, string, boolean | undefined, boolean]> = [
  ["release push skips duplicate container verification", "push", "Build and dispatch", false, false],
  ["release dispatch skips duplicate container verification", "workflow_dispatch", "Build and dispatch", false, false],
  ["caller can request container verification", "push", "Build and dispatch", true, true],
  ["pull requests always verify the container", "pull_request", "Verify", undefined, true],
  ["direct manual verification retains the default container gate", "workflow_dispatch", "Verify", true, true],
  ["manual verification respects an explicit opt-out", "workflow_dispatch", "Verify", false, false],
];

for (const [description, event, name, input, expected] of cases) {
  await test(description, () => {
    assert.ok(expression, "Container job must declare an explicit verification policy");
    assert.equal(Boolean(runInNewContext(expression, {
      github: { event_name: event, workflow: name },
      inputs: { run_container: input ?? false },
    })), expected);
  });
}

await test("both reusable and manual inputs enable the container by default", () => {
  const inputs = [...workflow.matchAll(/ {6}run_container:\n(?: {8}.+\n)+/g)];
  assert.equal(inputs.length, 2);
  for (const [input] of inputs) assert.match(input, / {8}default: true/);
});


await test("output publication waits for verification and image smoke tests on main", () => {
  const release = readFileSync(new URL("../workflows/build-and-dispatch.yml", import.meta.url), "utf8").replace(/\r\n/g, "\n");
  const output = release.split("\n  output-release:\n")[1]?.split("\n  deploy:\n")[0];
  assert.ok(output, "Output release job must exist");
  assert.match(output, /needs: \[verify, release\]/);
  assert.match(output, /if: github.ref == 'refs\/heads\/main'/);
  assert.match(output, /name: music-output-linux-x86_64/);
  assert.match(output, /contents: write/);
  assert.match(workflow, /runs-on: ubuntu-24.04/);
});

await test("release context includes the frontend compatibility build inputs", () => {
  const rules = readFileSync(new URL("../../.dockerignore", import.meta.url), "utf8")
    .split(/\r?\n/).map((line) => line.trim());
  // The release context starts denied. Keep the ES5 generator's source and
  // referenced TypeScript project in its explicit build-input allowlist.
  for (const input of ["frontend/tsconfig.compat.json", "frontend/compat/", "frontend/compat/**"]) {
    assert.ok(rules.includes(`!${input}`), `Docker context must include ${input}`);
  }
});
