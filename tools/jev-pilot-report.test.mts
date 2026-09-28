import assert from "node:assert/strict";
import { test } from "node:test";
import { renderPilotReport } from "./jev-pilot-report.mts";

const assessment = (name: string) => ({ questions: { [name]: { type: "noul" } }, state: { source: name } });
function plan() {
  return {
    schema_version: "jev-private-pilot/v2", arms: ["physical", "learned"], selected_tracks: [7],
    max_requests: 10, max_input_units: 1000, conformance: assessment("conformance"),
    corpus: { recordings: [{ display_name: "Private Song *One*", input: { track_id: 7 }, source_path: "never-render-me.m4a" }] },
    vocabulary: { groups: [{ tags: [
      { id: "mood.calm", name: "calm" }, { id: "mood.dreamy", name: "dreamy" },
    ] }] },
    cases: [
      { track_id: 7, arm: "physical", repeat_control: false, max_requests: 2, max_input_units: 100, assessment: [assessment("p")] },
      { track_id: 7, arm: "learned", repeat_control: false, max_requests: 2, max_input_units: 100, assessment: [assessment("l")] },
      { track_id: 7, arm: "learned", repeat_control: true, max_requests: 2, max_input_units: 100, assessment: [assessment("r")] },
    ],
  };
}
const emptyProfile = { "7": { track_id: 7, tags: [], decisions: [] } };
const suggestionProfile = { "7": { track_id: 7, tags: ["calm"], decisions: [
  { tag: "calm", support: "tentative", evidence: ["local"], evidence_ids: ["audio"], contradiction_ids: [] },
] } };
const row = (caseIndex: number, arm: string, repeat: boolean, profiles: unknown, error: unknown = null) =>
  ({ case: caseIndex, track_id: 7, arm, repeat_control: repeat, profiles, error, diagnostics: {} });
const started = (caseIndex: number, index: number, request: unknown, phase = "assessment") =>
  ({ event: "attempt_started", case: caseIndex, index, phase, request });
const response = (caseIndex: number, index: number, input_tokens: unknown, output_tokens: unknown,
  succeeded = true, phase = "assessment") => ({ event: "response", case: caseIndex, index, phase,
  succeeded, input_tokens, output_tokens, answers: succeeded ? {} : null });
const jsonl = (records: unknown[]) => records.map(value => JSON.stringify(value)).join("\n") + "\n";
const conformance = (p: ReturnType<typeof plan>) => [
  started(Number.MAX_SAFE_INTEGER, 0, p.conformance, "conformance"),
  response(Number.MAX_SAFE_INTEGER, 0, 3, 1, true, "conformance"),
];

void test("empty, failed and missing outcomes stay distinct and repeats are marked", () => {
  const p = plan();
  const result = { status: "failed", error: "stopped", result: { rows: [
    row(0, "physical", false, emptyProfile), row(1, "learned", false, null, "provider_failed"),
  ] } };
  const journal = jsonl([...conformance(p),
    started(0, 0, p.cases[0].assessment[0]), response(0, 0, null, null),
    started(1, 0, p.cases[1].assessment[0]), response(1, 0, 5, 2, false),
  ]);
  const report = renderPilotReport(p, result, journal);
  assert.match(report, /No suggestions returned/);
  assert.match(report, /\*\*Failed:\*\* provider\\_failed/);
  assert.match(report, /Missing result/);
  assert.match(report, /deliberate repeat control/);
  assert.match(report, /unknown \(1 attempt\)/);
  assert.match(report, /\| learned \| 1 \| 1 \| 0 \| 1 \| 0 \| 1 \| 5 \| 2 \|/);
  assert.doesNotMatch(report, /never-render-me/);
  assert.equal(report.match(/Useful tags/g)?.length, 1, "Owner rates tags once per song");
});

void test("identical tag sets are grouped across primary and repeat arms in the owner sheet", () => {
  const p = plan();
  const rows = [row(0, "physical", false, suggestionProfile), row(1, "learned", false, suggestionProfile),
    row(2, "learned", true, suggestionProfile)];
  const records = [...conformance(p)];
  p.cases.forEach((item, index) => records.push(started(index, 0, item.assessment[0]), response(index, 0, 10, 2)));
  const report = renderPilotReport(p, { status: "succeeded", error: null, result: { rows } }, jsonl(records));
  assert.match(report, /Set 1: \*\*calm\*\*.*physical, learned, learned \(repeat control\)/);
  assert.equal(report.match(/Useful tags/g)?.length, 1);
  assert.match(report, /\| \*\*Total\*\* \| 4 \| 4 \| 33 \| 7 \|/);
});

void test("result rows and profiles must match the exact planned case and selected track", () => {
  const p = plan(), journal = jsonl(conformance(p));
  const wrongRow = row(0, "learned", false, emptyProfile);
  assert.throws(() => renderPilotReport(p, { status: "failed", result: { rows: [wrongRow] } }, journal), /planned case/);
  const wrongProfile = row(0, "physical", false, { "8": { track_id: 8, tags: [], decisions: [] } });
  assert.throws(() => renderPilotReport(p, { status: "failed", result: { rows: [wrongProfile] } }, journal), /selected track/);
});

void test("a successful result requires complete paired journal responses and every assessment", () => {
  const p = plan(), rows = [row(0, "physical", false, emptyProfile), row(1, "learned", false, emptyProfile),
    row(2, "learned", true, emptyProfile)];
  const records = [...conformance(p), started(0, 0, p.cases[0].assessment[0])];
  assert.throws(() => renderPilotReport(p, { status: "succeeded", result: { rows } }, jsonl(records)), /missing rows or responses/);
});

void test("duplicate starts and responses without starts expose hidden replay or malformed journals", () => {
  const p = plan(), start = started(0, 0, p.cases[0].assessment[0]);
  assert.throws(() => renderPilotReport(p, { status: "failed", result: { rows: [] } }, jsonl([...conformance(p), start, start])), /hidden replay/);
  assert.throws(() => renderPilotReport(p, { status: "failed", result: { rows: [] } },
    jsonl([...conformance(p), response(0, 0, 1, 1)])), /without one matching start/);
  assert.throws(() => renderPilotReport(p, { status: "failed", result: { rows: [] } },
    jsonl([...conformance(p), start, started(0, 0, { grounding: true }, "grounding")])), /hidden replay/);
});

void test("unanswered starts make arm usage unknown because they may have incurred cost", () => {
  const p = plan();
  const report = renderPilotReport(p, { status: "failed", result: { rows: [] } },
    jsonl([...conformance(p), started(0, 0, p.cases[0].assessment[0])]));
  assert.match(report, /\| physical \| 1 \| 0 \| 0 \| 0 \| 1 \| 0 \| unknown \(1 attempt\) \| unknown \(1 attempt\) \|/);
});

void test("successful status rejects unsuccessful grounding responses", () => {
  const p = plan(), rows = [row(0, "physical", false, emptyProfile), row(1, "learned", false, emptyProfile),
    row(2, "learned", true, emptyProfile)], records = [...conformance(p)];
  p.cases.forEach((item, index) => records.push(started(index, 0, item.assessment[0]), response(index, 0, 1, 1)));
  records.push(started(0, 1, { questions: {}, state: {} }, "grounding"), response(0, 1, 1, 1, false, "grounding"));
  assert.throws(() => renderPilotReport(p, { status: "succeeded", result: { rows } }, jsonl(records)), /unsuccessful response/);
});

void test("phase and total usage include repeats and conformance once, retaining uncertain cost", () => {
  const p = plan(), records = [...conformance(p),
    started(0, 0, p.cases[0].assessment[0]), response(0, 0, 10, 2),
    started(0, 1, {}, "grounding"), response(0, 1, 20, 3, true, "grounding"),
    started(1, 0, p.cases[1].assessment[0]), response(1, 0, 30, 4),
    started(1, 1, {}, "grounding"), response(1, 1, 40, 5, true, "grounding"),
    started(2, 0, p.cases[2].assessment[0]), response(2, 0, 50, 6),
    started(2, 1, {}, "grounding"),
  ];
  const report = renderPilotReport(p, { status: "failed", result: { rows: [] } }, jsonl(records));
  assert.match(report, /\| physical \| assessment \| 1 \| 10 \| 2 \|/);
  assert.match(report, /\| physical \| grounding \| 1 \| 20 \| 3 \|/);
  assert.match(report, /\| learned \| assessment \| 1 \| 30 \| 4 \|/);
  assert.match(report, /\| learned \| grounding \| 1 \| 40 \| 5 \|/);
  assert.match(report, /\| assessment \| 3 \| 3 \| 90 \| 12 \|/);
  assert.match(report, /\| grounding \| 3 \| 2 \| 60 \+ 1 unknown \| 8 \+ 1 unknown \|/);
  assert.match(report, /\| conformance \| 1 \| 1 \| 3 \| 1 \|/);
  assert.match(report, /\| \*\*Total\*\* \| 7 \| 6 \| 153 \+ 1 unknown \| 21 \+ 1 unknown \|/);
});

void test("response phase must match the started request before accounting", () => {
  const p = plan(), records = [...conformance(p),
    started(0, 0, p.cases[0].assessment[0]), response(0, 0, 10, 2, true, "grounding")];
  assert.throws(() => renderPilotReport(p, { status: "failed", result: { rows: [] } }, jsonl(records)), /phase differs/);
});

void test("saved names resolve to vocabulary IDs while provider-shaped and mismatched profiles fail", () => {
  const p = plan(), records = [...conformance(p), started(0, 0, p.cases[0].assessment[0]), response(0, 0, 10, 2)];
  const render = (profile: unknown) => renderPilotReport(p,
    { status: "failed", result: { rows: [row(0, "physical", false, profile)] } }, jsonl(records));
  assert.match(render(suggestionProfile), /\*\*calm\*\* \(`mood\.calm`, tentative\)/);
  assert.throws(() => render({ "7": { track_id: 7, tags: ["mood.calm"], decisions: [
    { tag_id: "mood.calm", support: "tentative" },
  ] } }), /Invalid saved decision tag/);
  assert.throws(() => render({ "7": { ...suggestionProfile["7"], tags: ["dreamy"] } }), /tags\/decisions differ/);
  assert.throws(() => render({ "7": { track_id: 7, tags: ["invented"], decisions: [
    { tag: "invented", support: "tentative" },
  ] } }), /outside vocabulary/);
});

void test("owner grouping treats reordered normalized decisions as the same tag set", () => {
  const p = plan(), records = [...conformance(p)];
  const rows = p.cases.map((item, index) => {
    records.push(started(index, 0, item.assessment[0]), response(index, 0, 10, 2));
    const names = index === 1 ? ["dreamy", "calm"] : ["calm", "dreamy"];
    return row(index, item.arm, item.repeat_control, { "7": {
      track_id: 7, tags: names, decisions: names.map(tag => ({ tag, support: "tentative" })),
    } });
  });
  const report = renderPilotReport(p, { status: "succeeded", result: { rows } }, jsonl(records));
  assert.equal(report.match(/- Set \d+:/g)?.length, 1);
});
