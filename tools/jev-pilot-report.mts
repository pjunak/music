// Offline listening report for an exact Jev private-pilot plan/result/journal tuple.
import assert from "node:assert/strict";
import fs from "node:fs";
import { parseArgs } from "node:util";
import { pathToFileURL } from "node:url";

type ObjectValue = Record<string, unknown>;
interface Usage { responses: number; input: number; inputUnknown: number; output: number; outputUnknown: number }
interface JournalPair { start: ObjectValue; response?: ObjectValue }

const object = (value: unknown, message: string): ObjectValue => {
  assert(value !== null && typeof value === "object" && !Array.isArray(value), message);
  return value as ObjectValue;
};
const array = (value: unknown, message: string): unknown[] => { assert(Array.isArray(value), message); return value; };
const integer = (value: unknown, message: string): number => {
  assert(typeof value === "number" && Number.isSafeInteger(value) && value >= 0, message); return value;
};
const text = (value: unknown, message: string): string => { assert(typeof value === "string" && value.length > 0, message); return value; };
const canonical = (value: unknown): string => Array.isArray(value) ? `[${value.map(canonical).join(",")}]` :
  value !== null && typeof value === "object" ? `{${Object.keys(value).sort().map(key =>
    `${JSON.stringify(key)}:${canonical((value as ObjectValue)[key])}`).join(",")}}` : JSON.stringify(value);
const bounded = (file: string, limit: number): string => {
  const stat = fs.statSync(file); assert(stat.isFile() && stat.size > 0 && stat.size <= limit, "Invalid bounded input");
  return fs.readFileSync(file, "utf8");
};
const markdown = (value: string): string => value.replace(/[\\`*_[\]<>]/g, "\\$&").replace(/[\r\n]+/g, " ");
const usage = (): Usage => ({ responses: 0, input: 0, inputUnknown: 0, output: 0, outputUnknown: 0 });
const addUsage = (target: Usage, response: ObjectValue): void => {
  target.responses++;
  if (typeof response.input_tokens === "number" && Number.isSafeInteger(response.input_tokens) && response.input_tokens >= 0) target.input += response.input_tokens;
  else { assert(response.input_tokens === null || response.input_tokens === undefined, "Invalid input usage"); target.inputUnknown++; }
  if (typeof response.output_tokens === "number" && Number.isSafeInteger(response.output_tokens) && response.output_tokens >= 0) target.output += response.output_tokens;
  else { assert(response.output_tokens === null || response.output_tokens === undefined, "Invalid output usage"); target.outputUnknown++; }
};
const markUnknown = (target: Usage): void => { target.inputUnknown++; target.outputUnknown++; };
const usageText = (known: number, unknown: number): string => unknown === 0 ? String(known) :
  known === 0 ? `unknown (${unknown} attempt${unknown === 1 ? "" : "s"})` : `${known} + ${unknown} unknown`;

export function renderPilotReport(planValue: unknown, resultValue: unknown, journalText: string): string {
  const plan = object(planValue, "Invalid plan"), resultRoot = object(resultValue, "Invalid result");
  assert(plan.schema_version === "jev-private-pilot/v2", "Wrong plan schema");
  const cases = array(plan.cases, "Missing cases").map(value => object(value, "Invalid case"));
  const recordings = array(object(plan.corpus, "Missing corpus").recordings, "Missing recordings").map(value => object(value, "Invalid recording"));
  const selected = array(plan.selected_tracks, "Missing selected tracks").map(value => integer(value, "Invalid selected track"));
  const names = new Map<number, string>();
  for (const recording of recordings) {
    const id = integer(object(recording.input, "Missing recording input").track_id, "Invalid recording track");
    assert(!names.has(id), "Duplicate recording track");
    names.set(id, text(recording.display_name, "Missing local display name"));
  }
  assert.deepEqual([...names.keys()].sort((a, b) => a - b), [...new Set(selected)].sort((a, b) => a - b), "Selected/corpus tracks differ");
  const arms = array(plan.arms, "Missing arms").map(value => text(value, "Invalid arm"));
  assert.equal(new Set(arms).size, arms.length, "Duplicate arm");
  cases.forEach((item, index) => {
    integer(item.track_id, "Invalid case track");
    assert(names.has(item.track_id as number), "Case outside selected tracks");
    assert(arms.includes(text(item.arm, "Invalid case arm")), "Case outside planned arms");
    assert(typeof item.repeat_control === "boolean", "Invalid repeat flag");
    integer(item.max_requests, "Invalid case request bound"); integer(item.max_input_units, "Invalid case input bound");
    array(item.assessment, "Missing planned assessment");
    assert(index <= Number.MAX_SAFE_INTEGER);
  });

  const result = resultRoot.result === null || resultRoot.result === undefined ? {} : object(resultRoot.result, "Invalid nested result");
  const status = text(resultRoot.status, "Missing result status");
  assert(status === "succeeded" || status === "failed", "Unexpected result status");
  const rowsValue = result.rows ?? object(result.feature_progress ?? {}, "Invalid progress").rows ?? [];
  const rows = array(rowsValue, "Invalid result rows").map(value => object(value, "Invalid result row"));
  const rowByCase = new Map<number, ObjectValue>();
  for (const row of rows) {
    const caseIndex = integer(row.case, "Invalid result case");
    assert(caseIndex < cases.length && !rowByCase.has(caseIndex), "Bad or duplicate result membership");
    const planned = cases[caseIndex];
    assert(row.track_id === planned.track_id && row.arm === planned.arm && row.repeat_control === planned.repeat_control,
      "Result row does not match planned case");
    rowByCase.set(caseIndex, row);
  }

  const pairs = new Map<string, JournalPair>(), casePairs = new Map<number, JournalPair[]>(), conformance: JournalPair[] = [];
  for (const line of journalText.split(/\r?\n/).filter(Boolean)) {
    assert(line.length <= 16 * 1024 * 1024, "Oversized journal record");
    const record = object(JSON.parse(line), "Invalid journal record"), event = text(record.event, "Invalid journal event");
    const phase = text(record.phase, "Invalid journal phase"), index = integer(record.index, "Invalid journal index");
    const isConformance = phase === "conformance";
    let caseIndex = -1;
    if (!isConformance) { caseIndex = integer(record.case, "Invalid journal case"); assert(caseIndex < cases.length, "Journal case outside plan"); }
    const key = isConformance ? `conformance:${index}` : `${caseIndex}:${index}`;
    if (event === "attempt_started") {
      assert(!pairs.has(key), "Duplicate or hidden replay attempt");
      const pair = { start: record }; pairs.set(key, pair);
      (isConformance ? conformance : (casePairs.get(caseIndex) ?? (casePairs.set(caseIndex, []), casePairs.get(caseIndex)!))).push(pair);
      if (isConformance) assert(canonical(record.request) === canonical(plan.conformance), "Conformance request differs from plan");
      else if (phase === "assessment") {
        const assessment = array(cases[caseIndex].assessment, "Missing assessment");
        assert(index < assessment.length && canonical(record.request) === canonical(assessment[index]), "Assessment request differs from plan");
      } else assert(phase === "grounding", "Unknown journal phase");
    } else if (event === "response") {
      assert(typeof record.succeeded === "boolean", "Response lacks outcome");
      const pair = pairs.get(key); assert(pair && !pair.response, "Response without one matching start");
      assert.equal(phase, pair.start.phase, "Response phase differs from its attempt"); pair.response = record;
    } else assert.fail("Unknown journal event");
  }
  const started = [...pairs.values()], dangling = started.filter(pair => !pair.response);
  assert(started.length <= integer(plan.max_requests, "Invalid plan request bound"), "Journal exceeds request budget");
  for (const [caseIndex, attempts] of casePairs) {
    assert(attempts.length <= integer(cases[caseIndex].max_requests, "Invalid case bound"), "Case exceeds request budget");
    attempts.forEach((pair, index) => assert(pair.start.index === index, "Case attempt indices are not sequential"));
  }
  conformance.forEach((pair, index) => assert(pair.start.index === index, "Conformance attempt indices are not sequential"));
  if (status === "succeeded") {
    assert(rowByCase.size === cases.length && dangling.length === 0, "Successful result has missing rows or responses");
    assert([...rowByCase.values()].every(row => row.error === null), "Successful result contains failed row");
    assert(conformance.length === 1 && conformance[0].response?.succeeded === true, "Successful result lacks conformance");
    assert(started.every(pair => pair.response?.succeeded === true), "Successful result contains unsuccessful response");
    for (const [caseIndex, item] of cases.entries()) {
      const assessment = array(item.assessment, "Missing assessment");
      const attempts = casePairs.get(caseIndex) ?? [];
      for (let index = 0; index < assessment.length; index++) {
        assert(attempts.some(pair => pair.start.phase === "assessment" && pair.start.index === index && pair.response?.succeeded === true),
          "Successful result lacks a completed planned assessment");
      }
    }
  }
  const recordedUsage = isObject(result.usage) ? result.usage : undefined;
  if (recordedUsage?.attempted_requests !== undefined) assert.equal(recordedUsage.attempted_requests, started.length, "Usage/journal attempts differ");

  const tags = new Map<string, string>();
  for (const groupValue of array(object(plan.vocabulary, "Missing vocabulary").groups, "Missing vocabulary groups")) {
    const group = object(groupValue, "Invalid vocabulary group");
    for (const tagValue of array(group.tags, "Invalid vocabulary tags")) {
      const tag = object(tagValue, "Invalid vocabulary tag"), id = text(tag.id, "Invalid tag id");
      assert(!tags.has(id)); tags.set(id, text(tag.name, "Invalid tag name"));
    }
  }
  const primaryUsage = new Map(arms.map(arm => [arm, usage()])), repeatUsage = usage(), conformanceUsage = usage();
  const totalUsage = usage(), phases = ["assessment", "grounding", "conformance"];
  const phaseUsage = new Map(phases.map(phase => [phase, usage()]));
  const armPhaseUsage = new Map(arms.map(arm => [arm, new Map(phases.slice(0, 2).map(phase => [phase, usage()]))]));
  const observe = (target: Usage, pair: JournalPair): void => {
    if (pair.response) addUsage(target, pair.response); else markUnknown(target);
  };
  for (const pair of started) {
    observe(totalUsage, pair);
    observe(phaseUsage.get(pair.start.phase as string)!, pair);
  }
  for (const pair of conformance) observe(conformanceUsage, pair);
  for (const [caseIndex, attempts] of casePairs) for (const pair of attempts) {
    const target = cases[caseIndex].repeat_control ? repeatUsage : primaryUsage.get(cases[caseIndex].arm as string)!;
    observe(target, pair);
    if (!cases[caseIndex].repeat_control) observe(armPhaseUsage.get(cases[caseIndex].arm as string)!.get(pair.start.phase as string)!, pair);
  }

  const lines = ["# Jev private-pilot listening report", "",
    `Run status: **${status}**. This report does not select a winning arm or change tags.`, "",
    "## Primary arm summary", "", "| Arm | Planned songs | Results | No suggestions | Failed | Missing | Responses | Input tokens | Output tokens |",
    "|---|---:|---:|---:|---:|---:|---:|---:|---:|"];
  for (const arm of arms) {
    const indexes = cases.flatMap((item, index) => item.arm === arm && item.repeat_control === false ? [index] : []);
    let empty = 0, failed = 0, present = 0;
    for (const index of indexes) { const state = classify(cases[index], rowByCase.get(index), tags); if (state.kind === "empty") empty++; if (state.kind === "failed") failed++; if (state.kind !== "missing") present++; }
    const value = primaryUsage.get(arm)!;
    lines.push(`| ${markdown(arm)} | ${indexes.length} | ${present} | ${empty} | ${failed} | ${indexes.length - present} | ${value.responses} | ${usageText(value.input, value.inputUnknown)} | ${usageText(value.output, value.outputUnknown)} |`);
  }
  lines.push("", "## Primary usage by phase", "", "| Arm | Phase | Responses | Input tokens | Output tokens |",
    "|---|---|---:|---:|---:|");
  for (const [arm, armPhases] of armPhaseUsage) for (const [phase, value] of armPhases) {
    lines.push(`| ${markdown(arm)} | ${phase} | ${value.responses} | ${usageText(value.input, value.inputUnknown)} | ${usageText(value.output, value.outputUnknown)} |`);
  }
  lines.push("", "## Complete run usage", "",
    "Includes primary cases, deliberate repeats, and conformance exactly once. This is another view of the usage above, not additional spend. Unanswered attempts retain unknown usage.", "",
    "| Phase | Started | Responses | Input tokens | Output tokens |", "|---|---:|---:|---:|---:|");
  for (const [phase, value] of phaseUsage) {
    lines.push(`| ${phase} | ${started.filter(pair => pair.start.phase === phase).length} | ${value.responses} | ${usageText(value.input, value.inputUnknown)} | ${usageText(value.output, value.outputUnknown)} |`);
  }
  lines.push(`| **Total** | ${started.length} | ${totalUsage.responses} | ${usageText(totalUsage.input, totalUsage.inputUnknown)} | ${usageText(totalUsage.output, totalUsage.outputUnknown)} |`);
  lines.push("", "## Controls", "", "| Control | Planned cases | Started | Returned | Missing responses | Input tokens | Output tokens |",
    "|---|---:|---:|---:|---:|---:|---:|",
    `| Conformance | 1 | ${conformance.length} | ${conformanceUsage.responses} | ${conformance.length - conformanceUsage.responses} | ${usageText(conformanceUsage.input, conformanceUsage.inputUnknown)} | ${usageText(conformanceUsage.output, conformanceUsage.outputUnknown)} |`,
    `| Deliberate repeats | ${cases.filter(item => item.repeat_control === true).length} | ${[...casePairs].filter(([index]) => cases[index].repeat_control === true).reduce((sum, [, value]) => sum + value.length, 0)} | ${repeatUsage.responses} | ${[...casePairs].filter(([index]) => cases[index].repeat_control === true).flatMap(([, value]) => value).filter(pair => !pair.response).length} | ${usageText(repeatUsage.input, repeatUsage.inputUnknown)} | ${usageText(repeatUsage.output, repeatUsage.outputUnknown)} |`);
  for (const arm of arms) {
    lines.push("", `## ${markdown(arm)}`, "");
    for (const [caseIndex, item] of cases.entries()) if (item.arm === arm) {
      const trackId = integer(item.track_id, "Invalid case track"), name = markdown(names.get(trackId)!);
      const repeat = item.repeat_control === true ? " — **deliberate repeat control**" : "";
      const state = classify(item, rowByCase.get(caseIndex), tags);
      lines.push(`- **${name}** (track ${trackId}, case ${caseIndex})${repeat}: ${state.summary}`);
      for (const suggestion of state.suggestions) lines.push(`  - ${suggestion}`);
    }
  }
  lines.push("", "## Owner rating sheets", "");
  for (const trackId of selected) {
    const groups = new Map<string, { label: string; arms: string[] }>();
    for (const [caseIndex, item] of cases.entries()) if (item.track_id === trackId) {
      const state = classify(item, rowByCase.get(caseIndex), tags);
      const key = state.kind === "suggestions" || state.kind === "empty" ? `tags:${state.tagIds.join(",")}` : `${state.kind}:${state.summary}`;
      const label = state.kind === "suggestions" ? state.suggestions.join(", ") : state.summary;
      const arm = `${markdown(item.arm as string)}${item.repeat_control === true ? " (repeat control)" : ""}`;
      const group = groups.get(key) ?? { label, arms: [] }; group.arms.push(arm); groups.set(key, group);
    }
    lines.push(`### ${markdown(names.get(trackId)!)} (track ${trackId})`, "");
    let set = 0;
    for (const group of groups.values()) {
      const prefix = group.label.startsWith("**Missing") || group.label.startsWith("**Failed") ? "Run state" : `Set ${++set}`;
      lines.push(`- ${prefix}: ${group.label} — ${group.arms.join(", ")}`);
    }
    lines.push("- Useful tags: ____________________", "- Wrong tags: ____________________",
      "- Uncertain tags: ____________________", "- Missing tags: ____________________", "");
  }
  return lines.join("\n") + "\n";
}

function isObject(value: unknown): value is ObjectValue { return value !== null && typeof value === "object" && !Array.isArray(value); }
interface Classification { kind: string; summary: string; suggestions: string[]; tagIds: string[] }
function classify(planned: ObjectValue, row: ObjectValue | undefined, tags: Map<string, string>): Classification {
  if (!row) return { kind: "missing", summary: "**Missing result**", suggestions: [], tagIds: [] };
  if (row.error !== null && row.error !== undefined) {
    return { kind: "failed", summary: `**Failed:** ${markdown(text(row.error, "Invalid row error"))}`, suggestions: [], tagIds: [] };
  }
  const profiles = object(row.profiles, "Missing successful profiles"), keys = Object.keys(profiles), expected = String(planned.track_id);
  assert.deepEqual(keys, [expected], "Profile does not match selected track");
  const profile = object(profiles[expected], "Invalid profile"); assert.equal(profile.track_id, planned.track_id, "Proposal track mismatch");
  const decisions = array(profile.decisions, "Missing decisions").map(value => object(value, "Invalid decision"));
  // Saved ModelTagTrackOutput contains normalized names (TagDecision.tag), after
  // the application has resolved the provider's tag_id against the vocabulary.
  const names = decisions.map(decision => text(decision.tag, "Invalid saved decision tag"));
  assert.deepEqual(array(profile.tags, "Missing profile tags"), names, "Profile tags/decisions differ");
  const ids = names.map(name => {
    const matches = [...tags].filter(([, tagName]) => tagName === name);
    assert(matches.length === 1, "Proposal outside vocabulary or ambiguous saved tag");
    return matches[0][0];
  });
  assert.equal(new Set(ids).size, ids.length, "Duplicate proposal");
  const suggestions = decisions.map((decision, index) => {
    const id = ids[index], name = names[index];
    const support = text(decision.support, "Missing support"); assert(support === "supported" || support === "tentative", "Invalid support");
    return `**${markdown(name)}** (\`${markdown(id)}\`, ${support})`;
  });
  return suggestions.length ? { kind: "suggestions", summary: `${suggestions.length} suggestion${suggestions.length === 1 ? "" : "s"}`, suggestions, tagIds: ids.sort() } :
    { kind: "empty", summary: "**No suggestions returned**", suggestions: [], tagIds: [] };
}

export function main(args: string[]): void {
  const { values } = parseArgs({ args, options: Object.fromEntries(["plan", "result", "journal", "output"].map(key => [key, { type: "string" }])) });
  assert(["plan", "result", "journal", "output"].every(key => values[key]), "Usage: --plan PLAN --result RESULT --journal REQUESTS_JSONL --output NEW_MARKDOWN");
  const required = (key: string): string => { const value = values[key]; assert(value); return value; };
  const report = renderPilotReport(JSON.parse(bounded(required("plan"), 64 * 1024 * 1024)),
    JSON.parse(bounded(required("result"), 32 * 1024 * 1024)), bounded(required("journal"), 256 * 1024 * 1024));
  fs.writeFileSync(required("output"), report, { flag: "wx" });
}

if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) {
  try { main(process.argv.slice(2)); } catch { console.error("Jev pilot report failed. Check exact plan/result/journal membership and completion."); process.exitCode = 1; }
}
