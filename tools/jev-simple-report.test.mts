import assert from "node:assert/strict";
import { test } from "node:test";
import { buildValidatedSimpleNoulArtifacts, parseCompleteSimpleNoulJournal } from "./jev-simple-report.mts";

const comparison = (ids = ["mood.a", "mood.b"]) => ({
  case_id: "track-7", partition: 2, variant: "simple_noul", fit_threshold: 0.5,
  tags: Object.fromEntries(ids.map(id => [id, { tag: id, group: "mood", required: false, forbidden: false }])),
  request: { state: { evidence: "fixture" }, questions: Object.fromEntries(ids.map(id => [id,
    { type: "noul", instructions: { question: `Is ${id} fitting?`, definition: id } }])) },
});
const plan = () => ({ schema_version: "jev-simple-noul-pilot/v1", model: "jev-1.13.0",
  comparisons: [comparison()], certifies_model: false });
const jsonl = (values: unknown[]): string => values.map(value => JSON.stringify(value)).join("\n") + "\n";
const completeJournal = (p = plan()) => jsonl([
  { event: "plan", plan: p },
  { event: "attempt_started", index: 0, case_id: "track-7", variant: "simple_noul" },
  { event: "response", index: 0, result: { model: "jev-1.13.0", input_tokens: 10, output_tokens: 2,
    answers: { "mood.a": { type: "noul", noul: 0.8 }, "mood.b": { type: "noul", noul: 0.2 } } } },
  { event: "complete", requests: 1, certifies_model: false },
]);

void test("complete generic journal retains exact Noul values and independent usage", () => {
  const p = plan(), parsed = parseCompleteSimpleNoulJournal(p, completeJournal(p));
  assert.deepEqual([...parsed.answers.get(0)!], [["mood.a", 0.8], ["mood.b", 0.2]]);
  assert.deepEqual(parsed.usage.get(0), [10, 2]);

  const unknownUsage = completeJournal(p).replace('"input_tokens":10', '"input_tokens":null');
  assert.deepEqual(parseCompleteSimpleNoulJournal(p, unknownUsage).usage.get(0), [null, 2]);
});

void test("partial, replayed, stopped, wrong-model, and out-of-range journals never parse as complete", () => {
  const p = plan(), records = completeJournal(p).trim().split("\n").map(line => JSON.parse(line));
  assert.throws(() => parseCompleteSimpleNoulJournal(p, jsonl(records.slice(0, -1))), /completion checkpoint/);
  assert.throws(() => parseCompleteSimpleNoulJournal(p, jsonl([records[0], records[1], records[1], ...records.slice(2)])),
    /Response record fields differ|Response does not match/);
  assert.throws(() => parseCompleteSimpleNoulJournal(p, jsonl([records[0], records[1], { event: "stopped", index: 0 }])),
    /stopped before completion/);
  records[2].result.model = "other-model";
  assert.throws(() => parseCompleteSimpleNoulJournal(p, jsonl(records)), /model differs/);
  records[2].result.model = "jev-1.13.0";
  records[2].result.answers["mood.a"].noul = 1.01;
  assert.throws(() => parseCompleteSimpleNoulJournal(p, jsonl(records)), /invalid Noul probability/);
});

void test("answer membership is exact while object insertion order is irrelevant", () => {
  const p = plan(), records = completeJournal(p).trim().split("\n").map(line => JSON.parse(line));
  const reversed = structuredClone(records);
  reversed[2].result.answers = { "mood.b": reversed[2].result.answers["mood.b"],
    "mood.a": reversed[2].result.answers["mood.a"] };
  assert.deepEqual([...parseCompleteSimpleNoulJournal(p, jsonl(reversed)).answers.get(0)!],
    [["mood.a", 0.8], ["mood.b", 0.2]]);
  const missing = structuredClone(records);
  delete missing[2].result.answers["mood.b"];
  assert.throws(() => parseCompleteSimpleNoulJournal(p, jsonl(missing)), /answer membership/);
});

void test("attempt identity and terminal fields are exact", () => {
  const p = plan(), records = completeJournal(p).trim().split("\n").map(line => JSON.parse(line));
  const wrongPair = structuredClone(records);
  wrongPair[1].case_id = "track-8";
  assert.throws(() => parseCompleteSimpleNoulJournal(p, jsonl(wrongPair)), /order or identity/);
  const extraTerminal = structuredClone(records);
  extraTerminal[3].complete = true;
  assert.throws(() => parseCompleteSimpleNoulJournal(p, jsonl(extraTerminal)), /Completion record fields differ/);
});

void test("small validated comparison keeps scales distinct, ranks every tag, and reports overlap without deltas", () => {
  const usage = (inputTokens: number | null) => ({ requests: 1, inputTokens, outputTokens: 2,
    inputReports: inputTokens === null ? 0 : 1, outputReports: 1,
    estimatedInputCostUsd: inputTokens === null ? null : inputTokens * 0.042 / 1_000_000 });
  const values = [
    { id: "mood.a", tag: "calm", oldScore: 4, oldNormalized: 1, newProbability: 0.2 },
    { id: "mood.b", tag: "bright", oldScore: 2, oldNormalized: 0.5, newProbability: 0.9 },
    { id: "mood.c", tag: "dark", oldScore: 1, oldNormalized: 0.25, newProbability: 0.7 },
    { id: "mood.d", tag: "tense", oldScore: 0, oldNormalized: 0, newProbability: 0.5 },
    { id: "mood.e", tag: "warm", oldScore: 0, oldNormalized: 0, newProbability: 0.25 },
    { id: "mood.f", tag: "cold", oldScore: 0, oldNormalized: 0, newProbability: 0.24 },
  ];
  const artifacts = buildValidatedSimpleNoulArtifacts([{
    trackId: 7, displayName: "Fixture Song", sourcePath: "L:\\music\\fixture.m4a",
    usage: { previousScore: usage(100), simpleNoul: usage(null) },
    groups: { mood: values, scene: values, setting: values, period: values },
  }], "baseline-hash");
  assert.match(artifacts.previousScoreTags, /five-level ordinal musical-fit/);
  assert.match(artifacts.simpleNoulTags, /probability of answering yes/);
  assert.match(artifacts.comparison, /historical baseline, not a contemporaneous repeat/);
  assert.match(artifacts.comparison, /Top-five overlap: \*\*4\/5\*\*/);
  assert.match(artifacts.comparison, /\| Historical Score \/ 4 ordinal position \| 6 .*\| 3 \| 2 \| 1 \|/);
  assert.match(artifacts.comparison, /\| Simple Noul yes probability \| 6 .*\| 4 \| 3 \| 2 \|/);
  assert.match(artifacts.comparison, /Simple Noul \| 1 \| unknown \(incomplete token reporting\).*\| unknown \|/);
  assert.doesNotMatch(artifacts.comparison, /improved|improvement|score difference|delta| Δ /i);
  const json = artifacts.comparisonJson as { complete: boolean; songs: Array<{ groups: Record<string, { tags: unknown[] }> }> };
  assert.equal(json.complete, true);
  assert.equal(json.songs[0].groups.mood.tags.length, 6);
});
