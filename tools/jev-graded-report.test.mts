import assert from "node:assert/strict";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { test } from "node:test";
import { main, renderGradedReport } from "./jev-graded-report.mts";

const ARMS = ["labels3", "labels8", "labels8_physical", "labels8_temporal"];
const GROUPS = [
  ["mood", 39], ["scene", 42], ["setting", 49], ["period", 8],
] as const;
const levels = ["none", "weak", "moderate", "strong", "defining"];

const question = (id: string) => ({ type: "score", instructions: { description: `Rate ${id}` }, criteria: levels });
const request = (ids: string[], source: string) => ({
  state: { evidence: source }, questions: Object.fromEntries(ids.map(id => [id, question(id)])),
});

interface Fixture {
  plan: ReturnType<typeof makePlan>;
  rows: Record<string, unknown>[];
  records: Record<string, unknown>[];
}

function makePlan() {
  const groups = GROUPS.map(([group, count]) => ({
    key: group,
    tags: Array.from({ length: count }, (_, index) => ({
      id: `${group}.${String(index).padStart(2, "0")}`,
      name: `${group} ${index}`,
    })),
  }));
  const vocabulary = { groups };
  const recordings = [
    { display_name: "Private *Song* One", source_path: "\\\\?\\L:\\private music\\one #mix (take 1).m4a", input: { track_id: 7 } },
    { display_name: "Private Song Two", source_path: "L:\\private music\\two.m4a", input: { track_id: 9 } },
  ];
  const cases: Record<string, unknown>[] = [
    { group: "probe", expected_extreme: 0, request: request(["probe.absent"], "synthetic absent probe") },
    { group: "probe", expected_extreme: 4, request: request(["probe.present"], "synthetic present probe") },
  ];
  const addRecording = (track: number, arm: string, repeat: boolean): void => {
    for (const group of groups) {
      for (let offset = 0; offset < group.tags.length; offset += 25) {
        const ids = group.tags.slice(offset, offset + 25).map(tag => tag.id);
        cases.push({ group: group.key, track_id: track, arm, repeat_control: repeat,
          request: request(ids, `audio evidence ${track} ${arm} ${group.key} ${offset}`) });
      }
    }
  };
  for (const recording of recordings) for (const arm of ARMS) addRecording(recording.input.track_id, arm, false);
  for (const recording of recordings) addRecording(recording.input.track_id, "labels3", true);
  return {
    schema_version: "jev-graded-pilot/v1", rubric: "musical-fit-ordinal/v1", certifies_model: false,
    model: "jev-1.13", display_cutoff: 0.25,
    display_cutoff_status: "provisional, not fitted to listening labels",
    period_policy: "retain all period scores", arms: ARMS, vocabulary, recordings, cases,
    max_requests: cases.length, max_input_units: 1_000_000,
  };
}

function scoreFor(id: string): number {
  if (id === "probe.present") return 1;
  if (id === "probe.absent") return 0;
  const index = Number(id.split(".")[1]);
  return index < 10 ? 0.5 : 0;
}

function makeFixture(): Fixture {
  const plan = makePlan(), rows: Record<string, unknown>[] = [], records: Record<string, unknown>[] = [];
  plan.cases.forEach((item, index) => {
    const questions = (item.request as { questions: Record<string, unknown> }).questions;
    const answers = Object.fromEntries(Object.keys(questions).map(id => {
      const relevance = scoreFor(id);
      return [id, { type: "score", score: relevance * 4,
        probabilities: { "0": 0.2, "1": 0.2, "2": 0.2, "3": 0.2, "4": 0.2 }, confidence: 0.5 }];
    }));
    const scores = Object.fromEntries(Object.entries(answers).map(([id, answer]) => {
      const typed = answer as { score: number; probabilities: Record<string, number>; confidence: number };
      return [id, { relevance: typed.score / 4, raw_score: typed.score,
        level_probabilities: typed.probabilities, score_confidence: typed.confidence,
        status: "needs_listening_review" }];
    }));
    records.push({ event: "attempt_started", index, request: structuredClone(item.request) });
    records.push({ event: "response", index, succeeded: true, model: plan.model,
      input_tokens: 10, output_tokens: 2, answers });
    const probe = item.group === "probe";
    rows.push({ case: index, track_id: probe ? null : item.track_id, arm: probe ? null : item.arm, group: item.group,
      repeat_control: probe ? null : item.repeat_control, scores, probe_passed: probe ? true : null });
  });
  return { plan, rows, records };
}

const jsonl = (records: unknown[]): string => records.map(value => JSON.stringify(value)).join("\n") + "\n";
const render = (fixture: Fixture, status = "succeeded", cutoff?: number): string => renderGradedReport(
  fixture.plan,
  { status, result: { rows: fixture.rows, usage: { attempted_requests: fixture.records.length / 2 } } },
  jsonl(fixture.records), cutoff,
);

function setScore(fixture: Fixture, track: number, arm: string, repeat: boolean, id: string, relevance: number): void {
  const index = fixture.plan.cases.findIndex(item => item.track_id === track && item.arm === arm
    && item.repeat_control === repeat
    && Object.hasOwn((item.request as { questions: Record<string, unknown> }).questions, id));
  assert(index >= 0);
  const response = fixture.records.find(record => record.event === "response" && record.index === index)!;
  const answer = (response.answers as Record<string, Record<string, unknown>>)[id];
  answer.score = relevance * 4;
  const row = fixture.rows.find(value => value.case === index)!;
  const score = (row.scores as Record<string, Record<string, unknown>>)[id];
  score.relevance = relevance;
  score.raw_score = relevance * 4;
}

void test("renders every selected recording, every arm, all cutoff tags and review-safe language", () => {
  const fixture = makeFixture(), report = render(fixture);
  assert.match(report, /normalized positions on five described ordinal musical-fit levels/);
  assert.match(report, /They are not probabilities/);
  assert.match(report, /Display cutoff: \*\*25%\*\* \(provisional, not fitted to listening labels\)/);
  assert(report.includes("[Private \\*Song\\* One](<L:/private music/one %23mix (take 1).m4a>)"));
  assert.doesNotMatch(report, /\/\/\?\//);
  assert.equal(report.match(/^### \[/gm)?.length, fixture.plan.recordings.length);
  assert.equal(report.match(/^\| mood \|/gm)?.length, fixture.plan.recordings.length);
  for (let index = 0; index < 10; index++) assert.match(report, new RegExp(`mood ${index} 50%`));
  assert.match(report, /leaving a tag blank does not make it a negative label/);
  assert.doesNotMatch(report, /audio evidence 7/);
});

void test("period recommendation requires both the floor and lead while retaining the full audit", () => {
  const fixture = makeFixture();
  setScore(fixture, 7, "labels3", false, "period.00", 0.75);
  setScore(fixture, 7, "labels3", false, "period.01", 0.5);
  setScore(fixture, 7, "labels8", false, "period.00", 0.6);
  setScore(fixture, 7, "labels8", false, "period.01", 0.5);
  const report = render(fixture);
  assert.match(report, /labels3: \*\*period 0 75%\*\* \(lead 25%\)/);
  assert.match(report, /labels8: ambiguous \/ none \(period 0 60%, period 1 50%\)/);
  assert.equal(report.match(/^\| period \d+ \|/gm)?.length, 16, "all periods remain visible for both recordings");
});

void test("failed partial runs show incomplete cells and uncertain started-request cost", () => {
  const fixture = makeFixture();
  const missingIndex = fixture.plan.cases.findLastIndex(item => item.track_id === 9 && item.arm === "labels3"
    && item.repeat_control === false && item.group === "mood");
  fixture.rows = fixture.rows.filter(row => Number(row.case) < missingIndex);
  fixture.records = fixture.records.filter(record => Number(record.index) < missingIndex
    || (record.event === "attempt_started" && record.index === missingIndex));
  const report = renderGradedReport(fixture.plan,
    { status: "failed", result: { rows: fixture.rows, usage: { attempted_requests: missingIndex + 1 } } },
    jsonl(fixture.records));
  assert.match(report, /Run status: \*\*failed\*\*/);
  assert.match(report, /\*\*Incomplete \(25\/39 scores\)\*\*/);
  assert.match(report, /\| \*\*Whole run\*\* \| 31 \| 310 \+ 1 unknown \| 62 \+ 1 unknown \|/);
  assert.doesNotMatch(report, /Incomplete \(25\/39 scores\).*mood 38 0%/);
});

void test("failed runs recover completed paid answers from the durable journal and label them", () => {
  const fixture = makeFixture(), lastCompleted = 2;
  fixture.records = fixture.records.filter(record => Number(record.index) <= lastCompleted);
  const report = renderGradedReport(fixture.plan, { status: "failed", result: null }, jsonl(fixture.records));
  assert.match(report, /Recovery note: \*\*1 completed song partition recovered from the durable request journal\*\*/);
  assert.match(report, /\*Journal-recovered: 1\/1 completed partition\*<br>\*\*Incomplete \(25\/39 scores\)\*\*/);
  assert.match(report, /\| \*\*Whole run\*\* \| 3 \| 30 \|/);
});

void test("journal recovery rejects an all-zero Score distribution just like the runtime", () => {
  const fixture = makeFixture();
  fixture.records = fixture.records.filter(record => Number(record.index) <= 2);
  const response = fixture.records.find(record => record.event === "response" && record.index === 2)!;
  (response.answers as Record<string, Record<string, unknown>>)["mood.00"].probabilities =
    Object.fromEntries(levels.map((_, index) => [String(index), 0]));
  assert.throws(() => renderGradedReport(fixture.plan, { status: "failed", result: null }, jsonl(fixture.records)),
    /score distribution has no positive probability/);
});

void test("a failed semantic probe cannot be followed by another paid request or a successful run", () => {
  const continued = makeFixture();
  const response = continued.records.find(record => record.event === "response" && record.index === 1)!;
  (response.answers as Record<string, Record<string, unknown>>)["probe.present"].score = 2;
  assert.throws(() => render(continued, "failed"), /continued after a failed semantic probe/);

  const claimedSuccess = makeFixture();
  setScore(claimedSuccess, 7, "labels3", false, "mood.00", 0.5);
  const probeResponse = claimedSuccess.records.find(record => record.event === "response" && record.index === 1)!;
  (probeResponse.answers as Record<string, Record<string, unknown>>)["probe.present"].score = 2;
  const probeRow = claimedSuccess.rows.find(row => row.case === 1)!;
  const probeScore = (probeRow.scores as Record<string, Record<string, unknown>>)["probe.present"];
  probeScore.raw_score = 2;
  probeScore.relevance = 0.5;
  probeRow.probe_passed = false;
  assert.throws(() => render(claimedSuccess), /continued after a failed semantic probe|failed semantic probe/);
});

void test("primary averages exclude deliberate repeats and total usage counts each response once", () => {
  const report = render(makeFixture());
  assert.match(report, /\| labels3 primary \| 14 \| 140 \| 28 \| 70\.0 \| 14\.0 \|/);
  assert.match(report, /\| Deliberate repeats \| 14 \| 140 \| 28 \| — \| — \|/);
  assert.match(report, /\| Semantic probes \| 2 \| 20 \| 4 \| — \| — \|/);
  assert.match(report, /\| \*\*Whole run\*\* \| 72 \| 720 \| 144 \| — \| — \|/);
  assert.equal(report.match(/\| labels3 primary \| 7 \/ 7 \| 70 \| 14 \|/g)?.length, 2);
  assert.equal(report.match(/\| labels3 repeat \| 7 \/ 7 \| 70 \| 14 \|/g)?.length, 2);
});

void test("per-song input and output accounting preserve independent unknowns and reject invalid usage", () => {
  for (const kind of ["input", "output"] as const) {
    const fixture = makeFixture();
    const response = fixture.records.find(record => record.event === "response" && record.index === 2)!;
    response[`${kind}_tokens`] = null;
    const report = render(fixture);
    assert.match(report, kind === "input"
      ? /\| labels3 primary \| 7 \/ 7 \| 60 \+ 1 unknown \| 14 \|/
      : /\| labels3 primary \| 7 \/ 7 \| 70 \| 12 \+ 1 unknown \|/);
    assert.match(report, kind === "input"
      ? /\| labels3 primary \| 14 \| 130 \+ 1 unknown \| 28 \| incomplete \/ unknown \| 14\.0 \|/
      : /\| labels3 primary \| 14 \| 140 \| 26 \+ 1 unknown \| 70\.0 \| incomplete \/ unknown \|/);
    for (const invalid of [-1, 0.5, "missing"]) {
      response[`${kind}_tokens`] = invalid;
      assert.throws(() => render(fixture), new RegExp(`Invalid ${kind} usage`));
    }
  }
  const partial = makeFixture();
  partial.records = partial.records.filter(record => Number(record.index) <= 2);
  const report = renderGradedReport(partial.plan, { status: "failed", result: null }, jsonl(partial.records));
  assert.match(report, /\| labels3 primary \| 1 \/ 7 \| 10 \(partial\) \| 2 \(partial\) \|/);
  assert.match(report, /\| labels8 primary \| 0 \/ 7 \| not run \| not run \|/);
});

void test("cutoff overrides are display-only and provisional cutoff remains explicit", () => {
  const fixture = makeFixture(), report = render(fixture, "succeeded", 0.75);
  assert.match(report, /Display cutoff: \*\*75%\*\* \(review override; completed dense scores are unchanged\)/);
  assert.doesNotMatch(report, /mood 0 50%/);
  assert.match(render(fixture), /provisional, not fitted to listening labels/);
});

void test("duplicate starts, wrong case membership and wrong request references are rejected", () => {
  const duplicate = makeFixture();
  duplicate.records.splice(1, 0, { ...duplicate.records[0] });
  assert.throws(() => render(duplicate, "failed"), /hidden replay/);

  const wrongCase = makeFixture();
  wrongCase.rows[2].arm = "labels8";
  assert.throws(() => render(wrongCase, "failed"), /does not match planned case/);

  const wrongRequest = makeFixture();
  (wrongRequest.records[0].request as Record<string, unknown>).state = { changed: true };
  assert.throws(() => render(wrongRequest, "failed"), /request differs from plan/);
});

void test("full vocabulary coverage, exact IDs, finite values and raw normalization are enforced", () => {
  const missing = makeFixture();
  const primaryCase = missing.plan.cases.find(item => item.group === "mood" && item.repeat_control === false)!;
  delete (primaryCase.request as { questions: Record<string, unknown> }).questions["mood.00"];
  assert.throws(() => render(missing, "failed"), /full vocabulary group/);

  const mismatch = makeFixture();
  const score = (mismatch.rows[2].scores as Record<string, Record<string, unknown>>)["mood.00"];
  score.relevance = 0.75;
  assert.throws(() => render(mismatch), /relevance differs/);

  const nonFinite = makeFixture();
  const response = nonFinite.records.find(record => record.event === "response" && record.index === 2)!;
  (response.answers as Record<string, Record<string, unknown>>)["mood.00"].score = Number.NaN;
  assert.throws(() => render(nonFinite), /invalid raw score/);

  const wrongCase = makeFixture();
  wrongCase.rows[2].group = "Mood";
  assert.throws(() => render(wrongCase), /does not match planned case/);
});

void test("CLI writes a new bounded offline report and refuses to overwrite it", () => {
  const fixture = makeFixture(), directory = fs.mkdtempSync(path.join(os.tmpdir(), "jev-graded-report-"));
  const planPath = path.join(directory, "plan.json"), resultPath = path.join(directory, "result.json");
  const journalPath = path.join(directory, "requests.jsonl"), outputPath = path.join(directory, "report.md");
  try {
    fs.writeFileSync(planPath, JSON.stringify(fixture.plan));
    fs.writeFileSync(resultPath, JSON.stringify({ status: "succeeded", result: {
      rows: fixture.rows, usage: { attempted_requests: fixture.plan.cases.length },
    } }));
    fs.writeFileSync(journalPath, jsonl(fixture.records));
    const args = ["--plan", planPath, "--result", resultPath, "--journal", journalPath,
      "--output", outputPath, "--cutoff", "0.5"];
    main(args);
    assert.match(fs.readFileSync(outputPath, "utf8"), /Display cutoff: \*\*50%\*\*/);
    assert.throws(() => main(args), /EEXIST/);
  } finally {
    for (const file of [planPath, resultPath, journalPath, outputPath]) {
      if (fs.existsSync(file)) fs.unlinkSync(file);
    }
    fs.rmdirSync(directory);
  }
});
