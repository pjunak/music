import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";
import { runInNewContext } from "node:vm";

const workflow = readFileSync(new URL("../workflows/verify.yml", import.meta.url), "utf8").replace(/\r\n/g, "\n");
const container = workflow.split("\n  container:\n")[1]?.split("\n  fuzz-smoke:\n")[0];
const expression = container?.match(/\n    if: \$\{\{ (.+) \}\}/)?.[1];

for (const [description, event, name, input, expected] of [
  ["release push skips duplicate container verification", "push", "Build and dispatch", false, false],
  ["release dispatch skips duplicate container verification", "workflow_dispatch", "Build and dispatch", false, false],
  ["caller can request container verification", "push", "Build and dispatch", true, true],
  ["pull requests always verify the container", "pull_request", "Verify", undefined, true],
  ["direct manual verification retains the default container gate", "workflow_dispatch", "Verify", true, true],
  ["manual verification respects an explicit opt-out", "workflow_dispatch", "Verify", false, false],
]) {
  test(description, () => {
    assert.ok(expression, "Container job must declare an explicit verification policy");
    assert.equal(Boolean(runInNewContext(expression, {
      github: { event_name: event, workflow: name },
      inputs: { run_container: input ?? false },
    })), expected);
  });
}

test("both reusable and manual inputs enable the container by default", () => {
  const inputs = [...workflow.matchAll(/      run_container:\n(?:        .+\n)+/g)];
  assert.equal(inputs.length, 2);
  for (const [input] of inputs) assert.match(input, /        default: true/);
});
