// Offline report for one complete simple-Noul run paired with a validated graded baseline.
import assert from "node:assert/strict";
import { createHash } from "node:crypto";
import fs from "node:fs";
import path from "node:path";
import { parseArgs } from "node:util";
import { pathToFileURL } from "node:url";
import { renderGradedReport } from "./jev-graded-report.mts";

type ObjectValue = Record<string, unknown>;
type Group = "mood" | "scene" | "setting" | "period";

interface Tag {
  id: string;
  name: string;
  group: Group;
}
interface Usage {
  requests: number;
  inputTokens: number | null;
  outputTokens: number | null;
  inputReports: number;
  outputReports: number;
  estimatedInputCostUsd: number | null;
}
interface TagScore {
  id: string;
  tag: string;
  oldScore: number;
  oldNormalized: number;
  newProbability: number;
}
interface SongComparison {
  trackId: number;
  displayName: string;
  sourcePath: string;
  usage: { previousScore: Usage; simpleNoul: Usage };
  groups: Record<Group, TagScore[]>;
}
interface ParsedJournal {
  answers: Map<number, Map<string, number>>;
  usage: Map<number, [number | null, number | null]>;
}
interface Distribution {
  count: number;
  minimum: number;
  median: number;
  mean: number;
  maximum: number;
  at_least_0_25: number;
  at_least_0_50: number;
  at_least_0_70: number;
}
interface ComparisonArtifacts {
  previousScoreTags: string;
  simpleNoulTags: string;
  comparison: string;
  comparisonJson: ObjectValue;
}

const GROUPS: Group[] = ["mood", "scene", "setting", "period"];
const GROUP_COUNTS: Record<Group, number> = { mood: 39, scene: 42, setting: 49, period: 8 };
const SELECTED_TRACKS = [7, 8, 9, 10, 11, 12, 13];
const MODEL = "jev-1.13.0";
const INPUT_USD_PER_MILLION = 0.042;

const object = (value: unknown, message: string): ObjectValue => {
  assert(value !== null && typeof value === "object" && !Array.isArray(value), message);
  return value as ObjectValue;
};
const array = (value: unknown, message: string): unknown[] => {
  assert(Array.isArray(value), message);
  return value;
};
const text = (value: unknown, message: string): string => {
  assert(typeof value === "string" && value.length > 0, message);
  return value;
};
const integer = (value: unknown, message: string): number => {
  assert(typeof value === "number" && Number.isSafeInteger(value) && value >= 0, message);
  return value;
};
const finite = (value: unknown, message: string): number => {
  assert(typeof value === "number" && Number.isFinite(value), message);
  return value;
};
const unit = (value: unknown, message: string): number => {
  const parsed = finite(value, message);
  assert(parsed >= 0 && parsed <= 1, message);
  return parsed;
};
const exactKeys = (value: ObjectValue, expected: string[], message: string): void => {
  assert.deepEqual(Object.keys(value).sort(), [...expected].sort(), message);
};
const canonical = (value: unknown): string => Array.isArray(value)
  ? `[${value.map(canonical).join(",")}]`
  : value !== null && typeof value === "object"
    ? `{${Object.keys(value).sort().map(key => `${JSON.stringify(key)}:${canonical((value as ObjectValue)[key])}`).join(",")}}`
    : JSON.stringify(value);
const bounded = (file: string, limit: number): string => {
  const stat = fs.statSync(file);
  assert(stat.isFile() && stat.size > 0 && stat.size <= limit, "Invalid bounded input");
  return fs.readFileSync(file, "utf8");
};
const nativeJsonFingerprint = (serialized: string): string => {
  let compact = "", inString = false, escaped = false;
  for (const character of serialized) {
    if (inString) {
      compact += character;
      if (escaped) escaped = false;
      else if (character === "\\") escaped = true;
      else if (character === "\"") inString = false;
    } else if (character === "\"") {
      inString = true;
      compact += character;
    } else if (!/\s/.test(character)) compact += character;
  }
  assert(!inString && !escaped, "Invalid serialized baseline JSON");
  return createHash("sha256").update(compact).digest("hex");
};
const markdown = (value: string): string => value.replace(/[\\`*_[\]<>|]/g, "\\$&").replace(/[\r\n]+/g, " ");
const linkTarget = (value: string): string => {
  const localPath = value.replace(/^\\\\\?\\UNC\\/i, "\\\\").replace(/^\\\\\?\\/, "");
  return localPath.replace(/\\/g, "/").replace(/[%<>#?\r\n]/g, character =>
    `%${character.charCodeAt(0).toString(16).toUpperCase().padStart(2, "0")}`);
};
const percent = (value: number): string => `${(value * 100).toFixed(1)}%`;
const numberText = (value: number): string => value.toFixed(3);
const inputCost = (tokens: number): number => Number((tokens * INPUT_USD_PER_MILLION / 1_000_000).toFixed(12));
const questionText = (group: Group, name: string): string => {
  if (group === "mood") return `Is ${name} a fitting description of this music?`;
  if (group === "scene") return `Is this music fitting accompaniment for ${name}?`;
  if (group === "setting") return `Is ${name} a fitting setting evoked by this music?`;
  return `Is ${name} a fitting musical style or period for this music?`;
};
const containsExactString = (value: unknown, target: string): boolean => {
  if (typeof value === "string") {
    return value.replace(/\\/g, "/").toLocaleLowerCase() === target.replace(/\\/g, "/").toLocaleLowerCase();
  }
  if (Array.isArray(value)) return value.some(item => containsExactString(item, target));
  if (value !== null && typeof value === "object") {
    return Object.values(value as ObjectValue).some(item => containsExactString(item, target));
  }
  return false;
};

function readVocabulary(plan: ObjectValue): { tags: Map<string, Tag>; byGroup: Map<Group, Tag[]> } {
  const tags = new Map<string, Tag>(), byGroup = new Map<Group, Tag[]>();
  for (const groupValue of array(object(plan.vocabulary, "Missing vocabulary").groups, "Missing vocabulary groups")) {
    const group = object(groupValue, "Invalid vocabulary group"), key = text(group.key, "Missing group key") as Group;
    assert(GROUPS.includes(key) && !byGroup.has(key), "Unexpected or duplicate vocabulary group");
    const entries = array(group.tags, "Missing vocabulary tags").map(value => {
      const item = object(value, "Invalid vocabulary tag");
      const tag = { id: text(item.id, "Missing tag id"), name: text(item.name, "Missing tag name"), group: key };
      assert(!tags.has(tag.id), "Duplicate vocabulary tag");
      tags.set(tag.id, tag);
      return tag;
    });
    assert.equal(entries.length, GROUP_COUNTS[key], `Unexpected ${key} vocabulary size`);
    byGroup.set(key, entries);
  }
  assert.equal(tags.size, 138, "Simple report requires all 138 shipped tags");
  assert.deepEqual([...byGroup.keys()].sort(), [...GROUPS].sort(), "Vocabulary groups differ");
  return { tags, byGroup };
}

/** Parse a complete generic-runner journal. This validates transport ordering and answers, not the full pilot-plan contract. */
export function parseCompleteSimpleNoulJournal(planValue: unknown, journalText: string): ParsedJournal {
  const plan = object(planValue, "Invalid simple plan"), comparisons = array(plan.comparisons, "Missing comparisons")
    .map(value => object(value, "Invalid comparison"));
  const records = journalText.split(/\r?\n/);
  if (records.at(-1) === "") records.pop();
  assert(records.length > 0 && records.every(line => line.length > 0 && line.length <= 16 * 1024 * 1024),
    "Invalid or oversized journal record");
  const header = object(JSON.parse(records[0]), "Invalid journal plan record");
  exactKeys(header, ["event", "plan"], "Journal plan record fields differ");
  assert(header.event === "plan" && canonical(header.plan) === canonical(plan), "Journal plan differs from simple plan");
  const answers = new Map<number, Map<string, number>>(), usage = new Map<number, [number | null, number | null]>();
  let cursor = 1;
  for (let index = 0; index < comparisons.length; index++) {
    assert(cursor < records.length, "Simple journal is partial");
    const attempt = object(JSON.parse(records[cursor++]), "Invalid attempt record"), comparison = comparisons[index];
    exactKeys(attempt, ["event", "index", "case_id", "variant"], "Attempt record fields differ");
    assert(attempt.event === "attempt_started" && attempt.index === index
      && attempt.case_id === comparison.case_id && attempt.variant === comparison.variant,
    "Invalid attempt checkpoint order or identity");
    assert(cursor < records.length, "Simple journal has an unresolved attempt");
    const response = object(JSON.parse(records[cursor++]), "Invalid response record");
    assert(response.event !== "stopped", "Simple journal stopped before completion");
    exactKeys(response, ["event", "index", "result"], "Response record fields differ");
    assert(response.event === "response" && response.index === index, "Response does not match its attempt");
    const result = object(response.result, "Missing response result");
    exactKeys(result, ["answers", "model", "input_tokens", "output_tokens"], "Response result fields differ");
    assert(result.model === plan.model, "Response model differs from simple plan");
    const plannedQuestions = object(object(comparison.request, "Missing comparison request").questions,
      "Missing comparison questions");
    const responseAnswers = object(result.answers, "Missing response answers");
    assert.deepEqual(Object.keys(responseAnswers).sort(), Object.keys(plannedQuestions).sort(),
      "Response answer membership differs");
    const parsed = new Map<string, number>();
    for (const id of Object.keys(plannedQuestions)) {
      const answer = object(responseAnswers[id], `Missing answer ${id}`);
      exactKeys(answer, ["type", "noul"], `Answer ${id} fields differ`);
      assert(answer.type === "noul", `Answer ${id} has the wrong type`);
      parsed.set(id, unit(answer.noul, `Answer ${id} has an invalid Noul probability`));
    }
    const token = (key: "input_tokens" | "output_tokens"): number | null => {
      const value = result[key];
      if (value === null) return null;
      return integer(value, `Invalid ${key}`);
    };
    answers.set(index, parsed);
    usage.set(index, [token("input_tokens"), token("output_tokens")]);
  }
  assert(cursor < records.length, "Simple journal lacks a completion checkpoint");
  const complete = object(JSON.parse(records[cursor++]), "Invalid completion record");
  exactKeys(complete, ["event", "requests", "certifies_model"], "Completion record fields differ");
  assert(complete.event === "complete" && complete.requests === comparisons.length && complete.certifies_model === false,
    "Completion checkpoint does not cover the simple plan");
  assert.equal(cursor, records.length, "Simple journal continued after its terminal checkpoint");
  return { answers, usage };
}

function validateSimplePlan(baselineValue: unknown, simpleValue: unknown, baselineFingerprint?: string): {
  baseline: ObjectValue; simple: ObjectValue; comparisons: ObjectValue[]; recordings: Map<number, ObjectValue>;
  vocabulary: ReturnType<typeof readVocabulary>;
} {
  const baseline = object(baselineValue, "Invalid baseline plan"), simple = object(simpleValue, "Invalid simple plan");
  assert(simple.schema_version === "jev-simple-noul-pilot/v1", "Wrong simple plan schema");
  assert(simple.model === MODEL && simple.baseline_arm === "labels8" && simple.arm === "simple_noul",
    "Simple plan model or arm differs");
  assert(simple.certifies_model === false, "Simple plan must not certify the model");
  assert.deepEqual(array(simple.selected_tracks, "Missing selected tracks"), SELECTED_TRACKS,
    "Simple plan must contain sorted tracks 7 through 13");
  assert(typeof simple.baseline_plan_sha256 === "string" && /^[a-f0-9]{64}$/.test(simple.baseline_plan_sha256),
    "Simple plan lacks its native baseline fingerprint");
  if (baselineFingerprint !== undefined) {
    assert(simple.baseline_plan_sha256 === baselineFingerprint, "Simple plan references a different serialized baseline plan");
  }
  assert(canonical(simple.vocabulary) === canonical(baseline.vocabulary), "Simple vocabulary differs from baseline");
  const vocabulary = readVocabulary(baseline);

  const baselineRecordings = new Map<number, ObjectValue>();
  for (const value of array(baseline.recordings, "Missing baseline recordings")) {
    const recording = object(value, "Invalid baseline recording");
    const track = integer(object(recording.input, "Missing baseline input").track_id, "Invalid baseline track");
    assert(track > 0 && !baselineRecordings.has(track), "Duplicate baseline recording");
    baselineRecordings.set(track, recording);
  }
  const recordings = new Map<number, ObjectValue>();
  const simpleRecordings = array(simple.recordings, "Missing simple recordings").map(value => object(value, "Invalid recording"));
  assert.equal(simpleRecordings.length, SELECTED_TRACKS.length, "Simple recording count differs");
  simpleRecordings.forEach((recording, index) => {
    exactKeys(recording, ["track_id", "display_name", "source_path", "file_sha256"], "Simple recording fields differ");
    const track = integer(recording.track_id, "Invalid simple recording track"), old = baselineRecordings.get(track);
    assert.equal(track, SELECTED_TRACKS[index], "Simple recording order differs");
    assert(old && recording.display_name === old.display_name && recording.source_path === old.source_path
      && recording.file_sha256 === old.file_sha256, "Simple recording differs from baseline");
    recordings.set(track, recording);
  });

  const cases = array(baseline.cases, "Missing baseline cases").map(value => object(value, "Invalid baseline case"));
  const expected = cases.map((item, index) => ({ item, index })).filter(({ item }) =>
    SELECTED_TRACKS.includes(Number(item.track_id)) && item.arm === "labels8" && item.repeat_control === false);
  assert.equal(expected.length, 49, "Baseline does not contain the 49 selected labels8 partitions");
  const comparisons = array(simple.comparisons, "Missing comparisons").map(value => object(value, "Invalid comparison"));
  const pairs = array(simple.pairs, "Missing pairs").map(value => object(value, "Invalid pair"));
  assert.equal(integer(simple.request_count, "Invalid request count"), 49, "Simple request count differs");
  assert.equal(comparisons.length, 49, "Simple comparison count differs");
  assert.equal(pairs.length, comparisons.length, "Simple pair count differs");
  assert(integer(simple.max_input_units, "Invalid input-unit bound") > 0, "Simple input-unit bound is empty");
  const coverage = new Map<number, Set<string>>(SELECTED_TRACKS.map(track => [track, new Set()]));
  comparisons.forEach((comparison, index) => {
    exactKeys(comparison, ["case_id", "partition", "variant", "fit_threshold", "tags", "request"],
      "Comparison fields differ");
    const { item: old, index: baselineCase } = expected[index];
    const track = integer(old.track_id, "Invalid baseline case track"), group = text(old.group, "Invalid group") as Group;
    assert(GROUPS.includes(group), "Unknown comparison group");
    assert(comparison.case_id === `track-${track}` && comparison.partition === baselineCase
      && comparison.variant === "simple_noul" && comparison.fit_threshold === 0.5,
    "Simple comparison pairing or transport threshold differs");
    const pair = pairs[index];
    exactKeys(pair, ["baseline_case", "track_id", "group"], "Pair fields differ");
    assert(pair.baseline_case === baselineCase && pair.track_id === track && pair.group === group,
      "Simple pair does not match its baseline partition");
    const oldRequest = object(old.request, "Missing baseline request"), request = object(comparison.request, "Missing request");
    exactKeys(request, ["state", "questions"], "Simple request fields differ");
    assert(canonical(request.state) === canonical(oldRequest.state), "Simple request state differs from baseline");
    const oldQuestions = object(oldRequest.questions, "Missing baseline questions");
    const questions = object(request.questions, "Missing simple questions");
    const tags = object(comparison.tags, "Missing comparison tags");
    assert.deepEqual(Object.keys(questions), Object.keys(oldQuestions), "Simple question membership or order differs");
    assert.deepEqual(Object.keys(tags), Object.keys(oldQuestions), "Simple tag membership or order differs");
    for (const id of Object.keys(oldQuestions)) {
      const oldQuestion = object(oldQuestions[id], `Invalid baseline question ${id}`);
      assert(oldQuestion.type === "score", "Baseline comparison question is not Score");
      const oldInstructions = object(oldQuestion.instructions, `Missing baseline instructions ${id}`);
      const oldTag = object(oldInstructions.tag, `Missing baseline tag ${id}`);
      const known = vocabulary.tags.get(id);
      assert(known?.group === group && known.name === oldTag.name, "Baseline question does not match vocabulary");
      const target = object(tags[id], `Missing comparison tag ${id}`);
      exactKeys(target, ["tag", "group", "required", "forbidden"], `Comparison tag ${id} fields differ`);
      assert(target.tag === known.name && target.group === group && target.required === false && target.forbidden === false,
        `Comparison tag ${id} differs from baseline`);
      const question = object(questions[id], `Missing simple question ${id}`);
      exactKeys(question, ["type", "instructions"], `Simple question ${id} fields differ`);
      assert(question.type === "noul", `Simple question ${id} is not Noul`);
      const instructions = object(question.instructions, `Missing simple instructions ${id}`);
      exactKeys(instructions, ["question", "definition"], `Simple question ${id} instructions differ`);
      assert(instructions.question === questionText(group, known.name)
        && canonical(instructions.definition) === canonical(oldTag.definition), `Simple question ${id} changed text or definition`);
      assert(!coverage.get(track)!.has(id), "Tag appears more than once for a selected song");
      coverage.get(track)!.add(id);
    }
    const recording = recordings.get(track)!;
    for (const privateValue of [recording.display_name, recording.source_path]) {
      assert(!containsExactString(request, text(privateValue, "Invalid private recording value")),
        "Local recording identity leaked into simple request");
    }
  });
  for (const [track, ids] of coverage) {
    assert.equal(ids.size, 138, `Track ${track} does not cover the full vocabulary exactly once`);
    assert.deepEqual([...ids].sort(), [...vocabulary.tags.keys()].sort(), `Track ${track} vocabulary differs`);
  }
  return { baseline, simple, comparisons, recordings, vocabulary };
}

function usageFor(indices: number[], values: Map<number, [number | null, number | null]>): Usage {
  let input = 0, output = 0, inputReports = 0, outputReports = 0;
  for (const index of indices) {
    const value = values.get(index);
    assert(value, "Missing actual request usage");
    if (value[0] !== null) { input += value[0]; inputReports++; }
    if (value[1] !== null) { output += value[1]; outputReports++; }
  }
  return {
    requests: indices.length,
    inputTokens: inputReports === indices.length ? input : null,
    outputTokens: outputReports === indices.length ? output : null,
    inputReports,
    outputReports,
    estimatedInputCostUsd: inputReports === indices.length ? inputCost(input) : null,
  };
}

function baselineData(plan: ObjectValue, resultValue: unknown, journalText: string,
  comparisons: ObjectValue[]): { scores: Map<number, Map<string, [number, number]>>; usage: Map<number, [number | null, number | null]> } {
  const resultRoot = object(resultValue, "Invalid baseline result");
  assert(resultRoot.status === "succeeded", "Historical baseline must be complete and successful");
  const rows = array(object(resultRoot.result, "Missing baseline result body").rows, "Missing baseline rows")
    .map(value => object(value, "Invalid baseline row"));
  const rowByCase = new Map<number, ObjectValue>();
  for (const row of rows) rowByCase.set(integer(row.case, "Invalid baseline row case"), row);
  const scores = new Map<number, Map<string, [number, number]>>();
  for (const comparison of comparisons) {
    const index = integer(comparison.partition, "Invalid baseline partition"), row = rowByCase.get(index);
    assert(row, "Missing selected baseline result row");
    const saved = object(row.scores, "Missing selected baseline scores"), track = integer(row.track_id, "Invalid baseline row track");
    const current = scores.get(track) ?? new Map<string, [number, number]>();
    for (const [id, value] of Object.entries(saved)) {
      const score = object(value, `Invalid baseline score ${id}`);
      assert(!current.has(id), "Duplicate selected baseline score");
      const raw = finite(score.raw_score, `Invalid baseline raw score ${id}`);
      const normalized = unit(score.relevance, `Invalid baseline normalized score ${id}`);
      assert.equal(normalized, raw / 4, "Baseline normalization differs from Score / 4");
      current.set(id, [raw, normalized]);
    }
    scores.set(track, current);
  }
  const selected = new Set(comparisons.map(item => integer(item.partition, "Invalid partition")));
  const usage = new Map<number, [number | null, number | null]>();
  for (const line of journalText.split(/\r?\n/).filter(Boolean)) {
    assert(line.length <= 16 * 1024 * 1024, "Oversized baseline journal record");
    const record = object(JSON.parse(line), "Invalid baseline journal record");
    if (record.event !== "response") continue;
    const index = integer(record.index, "Invalid baseline journal index");
    if (!selected.has(index)) continue;
    assert(record.succeeded === true && record.model === plan.model, "Selected baseline response is not successful or pinned");
    assert(!usage.has(index), "Duplicate selected baseline response");
    const token = (key: "input_tokens" | "output_tokens"): number | null => {
      const value = record[key];
      return value === null || value === undefined ? null : integer(value, `Invalid baseline ${key}`);
    };
    usage.set(index, [token("input_tokens"), token("output_tokens")]);
  }
  assert.equal(usage.size, comparisons.length, "Selected baseline usage is incomplete");
  return { scores, usage };
}

function aggregateUsage(values: Usage[]): Usage {
  const requests = values.reduce((sum, value) => sum + value.requests, 0);
  const inputReports = values.reduce((sum, value) => sum + value.inputReports, 0);
  const outputReports = values.reduce((sum, value) => sum + value.outputReports, 0);
  const input = values.reduce((sum, value) => sum + (value.inputTokens ?? 0), 0);
  const output = values.reduce((sum, value) => sum + (value.outputTokens ?? 0), 0);
  return { requests, inputReports, outputReports, inputTokens: inputReports === requests ? input : null,
    outputTokens: outputReports === requests ? output : null,
    estimatedInputCostUsd: inputReports === requests ? inputCost(input) : null };
}

function usageJson(value: Usage): ObjectValue {
  return { requests: value.requests, input_tokens: value.inputTokens, output_tokens: value.outputTokens,
    responses_reporting_input: value.inputReports, responses_reporting_output: value.outputReports,
    input_cost_usd_at_0_042_per_million: value.estimatedInputCostUsd };
}

function usageText(value: number | null): string {
  return value === null ? "unknown (incomplete token reporting)" : String(value);
}

function ranked(values: TagScore[], kind: "old" | "new"): TagScore[] {
  return [...values].sort((left, right) => (kind === "old"
    ? right.oldNormalized - left.oldNormalized : right.newProbability - left.newProbability) || left.id.localeCompare(right.id));
}

function topFive(values: TagScore[], kind: "old" | "new"): TagScore[] {
  return ranked(values, kind).slice(0, 5);
}

function distribution(values: TagScore[], kind: "old" | "new"): Distribution {
  const scores = values.map(value => kind === "old" ? value.oldNormalized : value.newProbability).sort((a, b) => a - b);
  assert(scores.length > 0, "Cannot describe an empty score group");
  const median = scores.length % 2 === 0 ? (scores[scores.length / 2 - 1] + scores[scores.length / 2]) / 2
    : scores[Math.floor(scores.length / 2)];
  return { count: scores.length, minimum: scores[0], median,
    mean: scores.reduce((sum, value) => sum + value, 0) / scores.length, maximum: scores.at(-1)!,
    at_least_0_25: scores.filter(value => value >= 0.25).length,
    at_least_0_50: scores.filter(value => value >= 0.5).length,
    at_least_0_70: scores.filter(value => value >= 0.7).length };
}

/** Build display artifacts from already paired complete song data. No score subtraction or quality judgement is performed. */
export function buildValidatedSimpleNoulArtifacts(songs: SongComparison[], baselinePlanSha256: string): ComparisonArtifacts {
  assert(songs.length > 0, "No validated songs to report");
  const oldLines = ["# Previous Score tag rankings", "",
    "These historical values are five-level ordinal musical-fit results. `Score / 4` is only the level's normalized position; it is not a probability, calibrated strength, or percentage of the song.", ""];
  const newLines = ["# Simple Noul tag rankings", "",
    "Each value is Jev's probability of answering yes to that tag's short fit question. It is not mood strength, a calibrated real-world probability, or an automatic tag decision. All values are retained.", ""];
  const comparisonLines = ["# Historical Score and simple Noul comparison", "",
    "This is a descriptive comparison against a completed historical baseline, not a contemporaneous repeat. The old arm used a five-level Score question; the new arm used a short Noul yes/no question. Evidence, definitions, partitions, model, and songs were held fixed, while both wording and primitive changed.", "",
    "Top-five overlap and threshold counts describe the two outputs on their own scales. They do not establish calibrated strength, accuracy, or which method is better. No cutoff below is used to omit or apply tags.", ""];
  const jsonSongs: ObjectValue[] = [];
  for (const song of songs) {
    const heading = `## [${markdown(song.displayName)}](<${linkTarget(song.sourcePath)}>) (track ${song.trackId})`;
    oldLines.push(heading, "", `Actual historical usage: ${song.usage.previousScore.requests} requests; ${usageText(song.usage.previousScore.inputTokens)} input tokens; ${usageText(song.usage.previousScore.outputTokens)} output tokens.`, "");
    newLines.push(heading, "", `Actual simple-Noul usage: ${song.usage.simpleNoul.requests} requests; ${usageText(song.usage.simpleNoul.inputTokens)} input tokens; ${usageText(song.usage.simpleNoul.outputTokens)} output tokens.`, "");
    comparisonLines.push(heading, "", "| Run | Requests | Input tokens | Output tokens | Estimated input charge |",
      "|---|---:|---:|---:|---:|",
      `| Historical Score / labels8 | ${song.usage.previousScore.requests} | ${usageText(song.usage.previousScore.inputTokens)} | ${usageText(song.usage.previousScore.outputTokens)} | ${song.usage.previousScore.estimatedInputCostUsd === null ? "unknown" : `$${song.usage.previousScore.estimatedInputCostUsd.toFixed(6)}`} |`,
      `| Simple Noul | ${song.usage.simpleNoul.requests} | ${usageText(song.usage.simpleNoul.inputTokens)} | ${usageText(song.usage.simpleNoul.outputTokens)} | ${song.usage.simpleNoul.estimatedInputCostUsd === null ? "unknown" : `$${song.usage.simpleNoul.estimatedInputCostUsd.toFixed(6)}`} |`, "");
    const jsonGroups: ObjectValue = {};
    for (const group of GROUPS) {
      const values = song.groups[group], oldRanked = ranked(values, "old"), newRanked = ranked(values, "new");
      const oldTop = topFive(values, "old"), newTop = topFive(values, "new");
      const newIds = new Set(newTop.map(value => value.id));
      const overlap = oldTop.filter(value => newIds.has(value.id)).map(value => value.id);
      oldLines.push(`### ${group}`, "", `Top five: ${oldTop.map(value => `${markdown(value.tag)} ${value.oldScore.toFixed(2)}/4 (${numberText(value.oldNormalized)}, ${percent(value.oldNormalized)})`).join(", ")}.`, "",
        "| Rank | Tag | Score | Score / 4 | Percent display |", "|---:|---|---:|---:|---:|",
        ...oldRanked.map((value, index) => `| ${index + 1} | ${markdown(value.tag)} | ${value.oldScore.toFixed(2)} | ${numberText(value.oldNormalized)} | ${percent(value.oldNormalized)} |`), "");
      newLines.push(`### ${group}`, "", `Top five: ${newTop.map(value => `${markdown(value.tag)} ${numberText(value.newProbability)} (${percent(value.newProbability)})`).join(", ")}.`, "",
        "| Rank | Tag | Noul yes probability | Percent display |", "|---:|---|---:|---:|",
        ...newRanked.map((value, index) => `| ${index + 1} | ${markdown(value.tag)} | ${numberText(value.newProbability)} | ${percent(value.newProbability)} |`), "");
      const oldDistribution = distribution(values, "old"), newDistribution = distribution(values, "new");
      const oldRanks = new Map(oldRanked.map((value, index) => [value.id, index + 1]));
      const newRanks = new Map(newRanked.map((value, index) => [value.id, index + 1]));
      comparisonLines.push(`### ${group}`, "",
        `Top-five overlap: **${overlap.length}/5**${overlap.length ? ` (${overlap.map(id => markdown(values.find(value => value.id === id)!.tag)).join(", ")})` : ""}.`, "",
        "| Scale | Count | Min | Median | Mean | Max | ≥0.25 | ≥0.50 | ≥0.70 |", "|---|---:|---:|---:|---:|---:|---:|---:|---:|",
        `| Historical Score / 4 ordinal position | ${oldDistribution.count} | ${numberText(oldDistribution.minimum)} | ${numberText(oldDistribution.median)} | ${numberText(oldDistribution.mean)} | ${numberText(oldDistribution.maximum)} | ${oldDistribution.at_least_0_25} | ${oldDistribution.at_least_0_50} | ${oldDistribution.at_least_0_70} |`,
        `| Simple Noul yes probability | ${newDistribution.count} | ${numberText(newDistribution.minimum)} | ${numberText(newDistribution.median)} | ${numberText(newDistribution.mean)} | ${numberText(newDistribution.maximum)} | ${newDistribution.at_least_0_25} | ${newDistribution.at_least_0_50} | ${newDistribution.at_least_0_70} |`, "");
      jsonGroups[group] = { top_five: { previous_score: oldTop.map(value => value.id), simple_noul: newTop.map(value => value.id),
        overlap_count: overlap.length, overlap }, distributions: { previous_score_over_4: oldDistribution,
        simple_noul_yes_probability: newDistribution }, tags: values.map(value => ({ id: value.id, tag: value.tag,
        previous_rank: oldRanks.get(value.id), simple_noul_rank: newRanks.get(value.id),
        previous_score: value.oldScore, previous_score_over_4: value.oldNormalized,
        simple_noul_yes_probability: value.newProbability })) };
    }
    jsonSongs.push({ track_id: song.trackId, display_name: song.displayName, source_path: song.sourcePath,
      usage: { previous_score: usageJson(song.usage.previousScore), simple_noul: usageJson(song.usage.simpleNoul) }, groups: jsonGroups });
  }
  const baselineUsage = aggregateUsage(songs.map(song => song.usage.previousScore));
  const simpleUsage = aggregateUsage(songs.map(song => song.usage.simpleNoul));
  comparisonLines.splice(6, 0, "## Actual usage for the seven songs", "",
    "Costs below are reported only when every response supplied input-token usage. The estimate uses the currently verified $0.042 per million input tokens.", "",
    "| Run | Requests | Input tokens | Output tokens | Estimated input charge |", "|---|---:|---:|---:|---:|",
    `| Historical Score / labels8 | ${baselineUsage.requests} | ${usageText(baselineUsage.inputTokens)} | ${usageText(baselineUsage.outputTokens)} | ${baselineUsage.estimatedInputCostUsd === null ? "unknown" : `$${baselineUsage.estimatedInputCostUsd.toFixed(6)}`} |`,
    `| Simple Noul | ${simpleUsage.requests} | ${usageText(simpleUsage.inputTokens)} | ${usageText(simpleUsage.outputTokens)} | ${simpleUsage.estimatedInputCostUsd === null ? "unknown" : `$${simpleUsage.estimatedInputCostUsd.toFixed(6)}`} |`, "");
  return {
    previousScoreTags: oldLines.join("\n") + "\n",
    simpleNoulTags: newLines.join("\n") + "\n",
    comparison: comparisonLines.join("\n") + "\n",
    comparisonJson: { schema_version: "jev-simple-noul-comparison/v1", complete: true, certifies_model: false,
      baseline_plan_sha256: baselinePlanSha256, model: MODEL, historical_baseline: true,
      semantics: { previous_score: "Five-level ordinal musical fit; normalized only as Score / 4.",
        simple_noul: "Model probability of answering yes to the short fit question.",
        comparison: "Descriptive top-five overlap and within-scale distributions only; no calibrated difference or improvement claim." },
      usage: { input_usd_per_million: INPUT_USD_PER_MILLION, previous_score: usageJson(baselineUsage),
        simple_noul: usageJson(simpleUsage) }, songs: jsonSongs },
  };
}

export function renderSimpleNoulComparison(baselinePlanValue: unknown, baselineResultValue: unknown,
  baselineJournalText: string, simplePlanValue: unknown, simpleJournalText: string,
  baselineFingerprint?: string): ComparisonArtifacts {
  const validated = validateSimplePlan(baselinePlanValue, simplePlanValue, baselineFingerprint);
  const simpleJournal = parseCompleteSimpleNoulJournal(validated.simple, simpleJournalText);
  const old = baselineData(validated.baseline, baselineResultValue, baselineJournalText, validated.comparisons);
  const songs: SongComparison[] = SELECTED_TRACKS.map(track => {
    const recording = validated.recordings.get(track)!;
    const groups: Record<Group, TagScore[]> = { mood: [], scene: [], setting: [], period: [] };
    const baselineIndices: number[] = [], simpleIndices: number[] = [];
    validated.comparisons.forEach((comparison, index) => {
      if (comparison.case_id !== `track-${track}`) return;
      const baselineIndex = integer(comparison.partition, "Invalid baseline partition");
      baselineIndices.push(baselineIndex); simpleIndices.push(index);
      const baselineScores = old.scores.get(track)!;
      const simpleAnswers = simpleJournal.answers.get(index)!;
      for (const [id, probability] of simpleAnswers) {
        const tag = validated.vocabulary.tags.get(id)!, score = baselineScores.get(id);
        assert(score, `Missing historical score ${id}`);
        groups[tag.group].push({ id, tag: tag.name, oldScore: score[0], oldNormalized: score[1], newProbability: probability });
      }
    });
    for (const group of GROUPS) assert.equal(groups[group].length, GROUP_COUNTS[group], `Incomplete ${group} report data`);
    return { trackId: track, displayName: text(recording.display_name, "Missing display name"),
      sourcePath: text(recording.source_path, "Missing source path"), groups,
      usage: { previousScore: usageFor(baselineIndices, old.usage), simpleNoul: usageFor(simpleIndices, simpleJournal.usage) } };
  });
  return buildValidatedSimpleNoulArtifacts(songs, text(validated.simple.baseline_plan_sha256, "Missing baseline fingerprint"));
}

export function main(args: string[]): void {
  const { values } = parseArgs({ args, options: {
    "baseline-plan": { type: "string" }, "baseline-result": { type: "string" },
    "baseline-journal": { type: "string" }, journal: { type: "string" }, "output-directory": { type: "string" },
  } });
  const requiredKeys = ["baseline-plan", "baseline-result", "baseline-journal", "journal", "output-directory"] as const;
  assert(requiredKeys.every(key => values[key]),
    "Usage: --baseline-plan PLAN --baseline-result RESULT --baseline-journal REQUESTS_JSONL --journal SIMPLE_JSONL --output-directory NEW_DIRECTORY");
  const required = (key: typeof requiredKeys[number]): string => { const value = values[key]; assert(value); return value; };
  const baselinePlanText = bounded(required("baseline-plan"), 64 * 1024 * 1024);
  const baselinePlan = JSON.parse(baselinePlanText);
  const baselineResult = JSON.parse(bounded(required("baseline-result"), 32 * 1024 * 1024));
  const baselineJournal = bounded(required("baseline-journal"), 256 * 1024 * 1024);
  // This is the repository's authoritative full graded-plan/result/journal validator.
  renderGradedReport(baselinePlan, baselineResult, baselineJournal);
  const simplePlan = JSON.parse(bounded(required("journal"), 64 * 1024 * 1024).split(/\r?\n/, 1)[0]).plan;
  const simpleJournal = bounded(required("journal"), 64 * 1024 * 1024);
  const artifacts = renderSimpleNoulComparison(baselinePlan, baselineResult, baselineJournal, simplePlan, simpleJournal,
    nativeJsonFingerprint(baselinePlanText));
  const output = required("output-directory");
  assert(!fs.existsSync(output), "Output directory already exists");
  fs.mkdirSync(output);
  fs.writeFileSync(path.join(output, "previous-score-tags.md"), artifacts.previousScoreTags, { flag: "wx" });
  fs.writeFileSync(path.join(output, "simple-noul-tags.md"), artifacts.simpleNoulTags, { flag: "wx" });
  fs.writeFileSync(path.join(output, "comparison.md"), artifacts.comparison, { flag: "wx" });
  fs.writeFileSync(path.join(output, "comparison.json"), JSON.stringify(artifacts.comparisonJson, null, 2) + "\n", { flag: "wx" });
}

if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) {
  try { main(process.argv.slice(2)); } catch (error) {
    console.error("Jev simple Noul report failed. Check the exact plans, completed journals, baseline result, and output path.");
    if (error instanceof Error) console.error(error.message);
    process.exitCode = 1;
  }
}
