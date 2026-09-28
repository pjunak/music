// Offline listening report for an exact graded Jev plan/result/journal tuple.
import assert from "node:assert/strict";
import fs from "node:fs";
import { parseArgs } from "node:util";
import { pathToFileURL } from "node:url";

type ObjectValue = Record<string, unknown>;
type Group = "mood" | "scene" | "setting" | "period";

interface TokenUsage {
  tokens: number;
  unknown: number;
}
interface Usage {
  attempts: number;
  responses: number;
  input: TokenUsage;
  output: TokenUsage;
}
interface JournalPair {
  start: ObjectValue;
  response?: ObjectValue;
}
interface Tag {
  id: string;
  name: string;
  group: Group;
}
interface ScoreValue {
  relevance: number;
  confidence: number;
  rawScore: number;
  probabilities: ObjectValue;
}
interface Cell {
  scores: Map<string, ScoreValue>;
  completedCases: number;
  plannedCases: number;
  recoveredCases: number;
}

const GROUPS: Group[] = ["mood", "scene", "setting", "period"];
const EXPECTED_ARMS = ["labels3", "labels8", "labels8_physical", "labels8_temporal"];
const EXPECTED_GROUP_COUNTS: Record<Group, number> = { mood: 39, scene: 42, setting: 49, period: 8 };

const object = (value: unknown, message: string): ObjectValue => {
  assert(value !== null && typeof value === "object" && !Array.isArray(value), message);
  return value as ObjectValue;
};
const array = (value: unknown, message: string): unknown[] => {
  assert(Array.isArray(value), message);
  return value;
};
const integer = (value: unknown, message: string): number => {
  assert(typeof value === "number" && Number.isSafeInteger(value) && value >= 0, message);
  return value;
};
const number = (value: unknown, message: string): number => {
  assert(typeof value === "number" && Number.isFinite(value), message);
  return value;
};
const unit = (value: unknown, message: string): number => {
  const result = number(value, message);
  assert(result >= 0 && result <= 1, message);
  return result;
};
const text = (value: unknown, message: string): string => {
  assert(typeof value === "string" && value.length > 0, message);
  return value;
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
const markdown = (value: string): string => value.replace(/[\\`*_[\]<>|]/g, "\\$&").replace(/[\r\n]+/g, " ");
const linkTarget = (value: string): string => {
  const localPath = value.replace(/^\\\\\?\\UNC\\/i, "\\\\").replace(/^\\\\\?\\/, "");
  return localPath.replace(/\\/g, "/").replace(/[%<>#?\r\n]/g, character =>
    `%${character.charCodeAt(0).toString(16).toUpperCase().padStart(2, "0")}`);
};
const percent = (value: number): string => `${Math.round(value * 100)}%`;
const signedPoints = (value: number): string => `${value >= 0 ? "+" : ""}${(value * 100).toFixed(1)} pp`;
const cellKey = (track: number, arm: string, repeat: boolean): string => `${track}:${arm}:${repeat ? "repeat" : "primary"}`;
const usage = (): Usage => ({ attempts: 0, responses: 0,
  input: { tokens: 0, unknown: 0 }, output: { tokens: 0, unknown: 0 } });
const addUsage = (target: Usage, pair: JournalPair): void => {
  target.attempts++;
  if (pair.response) target.responses++;
  for (const kind of ["input", "output"] as const) {
    const value = pair.response?.[`${kind}_tokens`];
    if (typeof value === "number" && Number.isSafeInteger(value) && value >= 0) {
      target[kind].tokens += value;
      assert(Number.isSafeInteger(target[kind].tokens), `Invalid ${kind} usage total`);
    } else {
      assert(value === null || value === undefined, `Invalid ${kind} usage`);
      target[kind].unknown++;
    }
  }
};
const usageText = (value: TokenUsage): string => value.unknown === 0 ? String(value.tokens)
  : value.tokens === 0 ? `unknown (${value.unknown} attempt${value.unknown === 1 ? "" : "s"})`
    : `${value.tokens} + ${value.unknown} unknown`;

function exactString(value: unknown, target: string): boolean {
  if (typeof value === "string" && value.replace(/\\/g, "/").toLocaleLowerCase()
    === target.replace(/\\/g, "/").toLocaleLowerCase()) return true;
  if (Array.isArray(value)) return value.some(item => exactString(item, target));
  if (value !== null && typeof value === "object") {
    return Object.values(value as ObjectValue).some(item => exactString(item, target));
  }
  return false;
}

function scoreAnswer(value: unknown, levels: number, message: string): { raw: number; confidence: number; probabilities: ObjectValue } {
  const answer = object(value, message);
  assert(answer.type === "score", `${message}: wrong answer type`);
  const raw = number(answer.score, `${message}: invalid raw score`);
  assert(raw >= 0 && raw <= levels - 1, `${message}: raw score outside rubric`);
  const confidence = unit(answer.confidence, `${message}: invalid confidence`);
  const probabilities = object(answer.probabilities, `${message}: missing probabilities`);
  assert.deepEqual(Object.keys(probabilities).sort(), Array.from({ length: levels }, (_, index) => String(index)),
    `${message}: probability levels differ`);
  for (const probability of Object.values(probabilities)) unit(probability, `${message}: invalid level probability`);
  assert(Object.values(probabilities).some(probability => Number(probability) > 0),
    `${message}: score distribution has no positive probability`);
  return { raw, confidence, probabilities };
}

export function renderGradedReport(
  planValue: unknown,
  resultValue: unknown,
  journalText: string,
  cutoffOverride?: number,
): string {
  const plan = object(planValue, "Invalid plan"), resultRoot = object(resultValue, "Invalid result");
  assert(plan.schema_version === "jev-graded-pilot/v1" && plan.rubric === "musical-fit-ordinal/v1", "Wrong graded plan schema");
  assert(plan.certifies_model === false, "Graded plan must not certify a model");
  const planCutoff = unit(plan.display_cutoff, "Invalid plan cutoff");
  const cutoff = cutoffOverride === undefined ? planCutoff : unit(cutoffOverride, "Invalid cutoff override");
  const arms = array(plan.arms, "Missing arms").map(value => text(value, "Invalid arm"));
  assert.deepEqual(arms, EXPECTED_ARMS, "Graded plan must contain the four frozen arms");

  const tags = new Map<string, Tag>(), tagsByGroup = new Map<Group, Tag[]>();
  for (const groupValue of array(object(plan.vocabulary, "Missing vocabulary").groups, "Missing vocabulary groups")) {
    const group = object(groupValue, "Invalid vocabulary group"), key = text(group.key, "Missing group key") as Group;
    assert(GROUPS.includes(key) && !tagsByGroup.has(key), "Unexpected or duplicate vocabulary group");
    const entries = array(group.tags, "Missing vocabulary tags").map(tagValue => {
      const tag = object(tagValue, "Invalid vocabulary tag");
      const entry = { id: text(tag.id, "Missing tag id"), name: text(tag.name, "Missing tag name"), group: key };
      assert(!tags.has(entry.id), "Duplicate vocabulary tag");
      tags.set(entry.id, entry);
      return entry;
    });
    assert.equal(entries.length, EXPECTED_GROUP_COUNTS[key], `Unexpected ${key} vocabulary size`);
    tagsByGroup.set(key, entries);
  }
  assert.equal(tags.size, 138, "Graded report requires all 138 shipped tags");
  assert.deepEqual([...tagsByGroup.keys()].sort(), [...GROUPS].sort(), "Vocabulary groups differ");

  const recordings = array(plan.recordings, "Missing recordings").map(value => object(value, "Invalid recording"));
  assert(recordings.length > 0 && recordings.length <= 16, "Invalid recording count");
  const recordingById = new Map<number, ObjectValue>();
  for (const recording of recordings) {
    const track = integer(object(recording.input, "Missing recording input").track_id, "Invalid recording track");
    assert(track > 0 && !recordingById.has(track), "Duplicate recording track");
    text(recording.display_name, "Missing local display name");
    text(recording.source_path, "Missing local source path");
    recordingById.set(track, recording);
  }

  const cases = array(plan.cases, "Missing cases").map(value => object(value, "Invalid case"));
  assert.equal(integer(plan.max_requests, "Invalid request bound"), cases.length, "Plan request count differs");
  const plannedCoverage = new Map<string, Map<Group, Set<string>>>(), plannedCases = new Map<string, Map<Group, number>>();
  const probeExtremes: number[] = [];
  cases.forEach((item, index) => {
    const group = text(item.group, "Invalid case group");
    const request = object(item.request, "Missing planned request");
    const questions = object(request.questions, "Missing planned questions");
    assert(Object.keys(questions).length > 0, "Empty graded request");
    for (const [id, questionValue] of Object.entries(questions)) {
      const question = object(questionValue, "Invalid graded question");
      assert(question.type === "score", "Graded request must use Score questions");
      assert.equal(array(question.criteria, "Missing Score levels").length, 5, "Graded request must use five levels");
      if (group !== "probe") assert(tags.get(id)?.group === group, "Question tag does not match case group");
    }
    if (group === "probe") {
      assert.equal(Object.keys(questions).length, 1, "Probe must contain one Score question");
      probeExtremes.push(integer(item.expected_extreme, "Invalid probe extreme"));
      assert(item.track_id === null || item.track_id === undefined, "Probe must not reference a recording");
      return;
    }
    assert(GROUPS.includes(group as Group), "Unknown musical group");
    const track = integer(item.track_id, "Invalid case track"), arm = text(item.arm, "Invalid case arm");
    assert(recordingById.has(track) && arms.includes(arm), "Case outside recording or arm membership");
    assert(typeof item.repeat_control === "boolean", "Invalid repeat flag");
    const recording = recordingById.get(track)!;
    for (const forbidden of [recording.display_name, recording.source_path]) {
      assert(!exactString(request, text(forbidden, "Invalid local identity")), "Local name or path leaked into provider request");
    }
    const key = cellKey(track, arm, item.repeat_control);
    const coverage = plannedCoverage.get(key) ?? new Map<Group, Set<string>>();
    const counts = plannedCases.get(key) ?? new Map<Group, number>();
    const ids = coverage.get(group as Group) ?? new Set<string>();
    for (const id of Object.keys(questions)) {
      assert(!ids.has(id), "Duplicate tag question in planned cell");
      ids.add(id);
    }
    coverage.set(group as Group, ids);
    counts.set(group as Group, (counts.get(group as Group) ?? 0) + 1);
    plannedCoverage.set(key, coverage);
    plannedCases.set(key, counts);
    assert(index <= Number.MAX_SAFE_INTEGER);
  });
  assert.deepEqual(probeExtremes, [0, 4], "Expected absent and defining semantic Score probes");
  for (const track of recordingById.keys()) for (const arm of arms) {
    const key = cellKey(track, arm, false), coverage = plannedCoverage.get(key);
    assert(coverage, "Missing primary track/arm plan");
    for (const group of GROUPS) {
      assert.deepEqual([...coverage.get(group) ?? []].sort(), tagsByGroup.get(group)!.map(tag => tag.id).sort(),
        "Primary plan does not cover the full vocabulary group");
      assert.equal(plannedCases.get(key)?.get(group), Math.ceil(EXPECTED_GROUP_COUNTS[group] / 25),
        "Primary plan partition count differs");
    }
  }
  for (const [key, coverage] of plannedCoverage) {
    if (!key.endsWith(":repeat")) continue;
    for (const group of GROUPS) {
      assert.deepEqual([...coverage.get(group) ?? []].sort(), tagsByGroup.get(group)!.map(tag => tag.id).sort(),
        "Repeat plan does not cover the full vocabulary group");
      assert.equal(plannedCases.get(key)?.get(group), Math.ceil(EXPECTED_GROUP_COUNTS[group] / 25),
        "Repeat plan partition count differs");
    }
  }
  const recordingIds = [...recordingById.keys()];
  const expectedRepeats = new Set([recordingIds[0], recordingIds.at(-1)!].map(track => cellKey(track, "labels3", true)));
  const observedRepeats = new Set([...plannedCoverage.keys()].filter(key => key.endsWith(":repeat")));
  assert.deepEqual([...observedRepeats].sort(), [...expectedRepeats].sort(), "Repeat membership differs from frozen endpoint controls");

  const pairs = new Map<number, JournalPair>();
  let openAttempt: number | undefined, stopped = false;
  for (const line of journalText.split(/\r?\n/).filter(Boolean)) {
    assert(!stopped, "Journal continued after a failed response");
    assert(line.length <= 16 * 1024 * 1024, "Oversized journal record");
    const record = object(JSON.parse(line), "Invalid journal record");
    const event = text(record.event, "Invalid journal event"), index = integer(record.index, "Invalid journal index");
    assert(index < cases.length, "Journal index outside plan");
    if (event === "attempt_started") {
      assert(openAttempt === undefined, "Duplicate or hidden replay attempt before the prior response");
      assert.equal(index, pairs.size, "Journal attempt indices are not sequential");
      assert(!pairs.has(index), "Duplicate or hidden replay attempt");
      assert.equal(canonical(record.request), canonical(cases[index].request), "Journal request differs from plan");
      pairs.set(index, { start: record });
      openAttempt = index;
    } else if (event === "response") {
      assert(typeof record.succeeded === "boolean", "Response lacks outcome");
      const pair = pairs.get(index);
      assert(pair && !pair.response && openAttempt === index, "Response without one matching start");
      pair.response = record;
      openAttempt = undefined;
      stopped = record.succeeded === false;
    } else {
      assert.fail("Unknown journal event");
    }
  }
  assert(pairs.size <= cases.length, "Journal exceeds request budget");

  const result = resultRoot.result === null || resultRoot.result === undefined ? {} : object(resultRoot.result, "Invalid nested result");
  const status = text(resultRoot.status, "Missing result status");
  assert(status === "succeeded" || status === "failed", "Unexpected result status");
  const rowsValue = result.rows ?? object(result.feature_progress ?? {}, "Invalid feature progress").rows ?? [];
  const rows = array(rowsValue, "Invalid result rows").map(value => object(value, "Invalid result row"));
  const rowByCase = new Map<number, ObjectValue>(), cells = new Map<string, Map<Group, Cell>>();
  const responseScores = new Map<number, Map<string, ScoreValue>>(), probeOutcomes = new Map<number, boolean>();
  for (const [index, pair] of pairs) {
    if (pair.response?.succeeded !== true) continue;
    assert.equal(pair.response.model, plan.model, "Response model differs");
    const planned = cases[index], group = text(planned.group, "Invalid planned group");
    const answers = object(pair.response.answers, "Successful response lacks answers");
    const questions = object(object(planned.request, "Missing planned request").questions, "Missing planned questions");
    assert.deepEqual(Object.keys(answers).sort(), Object.keys(questions).sort(), "Response answer membership differs");
    const parsed = new Map<string, ScoreValue>();
    for (const id of Object.keys(questions)) {
      const levelCount = array(object(questions[id], "Invalid question").criteria, "Missing levels").length;
      const raw = scoreAnswer(answers[id], levelCount, `Answer ${id}`);
      parsed.set(id, { relevance: raw.raw / (levelCount - 1), confidence: raw.confidence,
        rawScore: raw.raw, probabilities: raw.probabilities });
    }
    responseScores.set(index, parsed);
    if (group === "probe") {
      const only = parsed.values().next().value as ScoreValue;
      const passed = planned.expected_extreme === 4 ? only.relevance >= 0.75 : only.relevance <= 0.25;
      probeOutcomes.set(index, passed);
      assert(passed || index === pairs.size - 1, "Journal continued after a failed semantic probe");
    }
  }
  const addCaseScores = (index: number, parsed: Map<string, ScoreValue>, recovered: boolean): void => {
    const planned = cases[index], group = text(planned.group, "Invalid planned group");
    if (group === "probe") return;
    const track = integer(planned.track_id, "Invalid planned track"), arm = text(planned.arm, "Invalid planned arm");
    assert(typeof planned.repeat_control === "boolean", "Invalid planned repeat flag");
    const key = cellKey(track, arm, planned.repeat_control), grouped = cells.get(key) ?? new Map<Group, Cell>();
    const groupKey = group as Group;
    const cell = grouped.get(groupKey) ?? { scores: new Map(), completedCases: 0,
      plannedCases: plannedCases.get(key)?.get(groupKey) ?? 0, recoveredCases: 0 };
    for (const [id, score] of parsed) {
      assert(!cell.scores.has(id), "Duplicate saved or recovered score across partitions");
      cell.scores.set(id, score);
    }
    cell.completedCases++;
    if (recovered) cell.recoveredCases++;
    grouped.set(groupKey, cell);
    cells.set(key, grouped);
  };
  for (const row of rows) {
    const index = integer(row.case, "Invalid result case");
    assert(index < cases.length && !rowByCase.has(index), "Bad or duplicate result membership");
    const planned = cases[index], group = text(planned.group, "Invalid planned group");
    if (group === "probe") {
      assert(row.track_id === null && row.arm === null && row.repeat_control === null && row.group === planned.group,
        "Probe result row does not match Rust null metadata shape");
    } else {
      assert(row.track_id === planned.track_id && row.arm === planned.arm && row.group === planned.group
        && row.repeat_control === planned.repeat_control, "Result row does not match planned case");
    }
    const pair = pairs.get(index);
    assert(pair?.response?.succeeded === true, "Result row lacks one successful journal response");
    const questions = object(object(planned.request, "Missing planned request").questions, "Missing planned questions");
    const saved = object(row.scores, "Missing saved scores");
    assert.deepEqual(Object.keys(saved).sort(), Object.keys(questions).sort(), "Saved score membership differs");
    const parsed = responseScores.get(index)!;
    for (const id of Object.keys(questions)) {
      const raw = parsed.get(id)!, score = object(saved[id], `Missing saved score ${id}`);
      const relevance = unit(score.relevance, `Invalid relevance ${id}`), savedRaw = number(score.raw_score, `Invalid raw score ${id}`);
      const confidence = unit(score.score_confidence, `Invalid score confidence ${id}`);
      assert.equal(savedRaw, raw.rawScore, "Saved raw score differs from response");
      assert.equal(relevance, raw.relevance, "Saved relevance differs from raw Score normalization");
      assert.equal(confidence, raw.confidence, "Saved confidence differs from response");
      assert.equal(canonical(score.level_probabilities), canonical(raw.probabilities), "Saved probabilities differ from response");
      assert.equal(score.status, "needs_listening_review", "Saved score lost review status");
    }
    if (group === "probe") {
      assert.equal(row.probe_passed, probeOutcomes.get(index), "Saved probe outcome differs from its Score");
    } else {
      assert(row.probe_passed === null || row.probe_passed === undefined, "Song result must not contain a probe outcome");
    }
    rowByCase.set(index, row);
    addCaseScores(index, parsed, false);
  }
  const successfulResponses = [...pairs].flatMap(([index, pair]) => pair.response?.succeeded === true ? [index] : []);
  if (status === "failed") {
    for (const index of successfulResponses) {
      if (!rowByCase.has(index)) addCaseScores(index, responseScores.get(index)!, true);
    }
  }
  if (isObject(result.usage) && result.usage.attempted_requests !== undefined) {
    assert.equal(result.usage.attempted_requests, pairs.size, "Usage/journal attempts differ");
  }
  if (status === "succeeded") {
    assert.deepEqual([...rowByCase.keys()].sort((left, right) => left - right), successfulResponses,
      "Successful journal responses and result rows differ");
    assert.equal(rowByCase.size, cases.length, "Successful result has missing rows");
    assert.equal(pairs.size, cases.length, "Successful result has missing attempts");
    assert([...pairs.values()].every(pair => pair.response?.succeeded === true), "Successful result contains missing or failed responses");
    assert([...probeOutcomes.values()].every(Boolean), "Successful result contains a failed semantic probe");
  }

  const primaryUsage = new Map(arms.map(arm => [arm, usage()]));
  const perRecordingUsage = new Map<string, Usage>();
  const repeatUsage = usage(), probeUsage = usage(), totalUsage = usage();
  for (const [index, pair] of pairs) {
    addUsage(totalUsage, pair);
    const item = cases[index];
    if (item.group === "probe") addUsage(probeUsage, pair);
    else {
      if (item.repeat_control === true) addUsage(repeatUsage, pair);
      else addUsage(primaryUsage.get(item.arm as string)!, pair);
      const key = cellKey(item.track_id as number, item.arm as string, item.repeat_control === true);
      const value = perRecordingUsage.get(key) ?? usage();
      addUsage(value, pair);
      perRecordingUsage.set(key, value);
    }
  }
  const recoveredPartitions = [...cells.values()].reduce((total, grouped) => total
    + [...grouped.values()].reduce((subtotal, cell) => subtotal + cell.recoveredCases, 0), 0);

  const lines = [
    "# Jev graded listening report", "",
    `Run status: **${status}**. This is a listening-review artifact; it does not certify calibrated accuracy or change library tags.`,
  ];
  if (recoveredPartitions > 0) lines.push("",
    `Recovery note: **${recoveredPartitions} completed song partition${recoveredPartitions === 1 ? "" : "s"} recovered from the durable request journal** because the failed result lacked those checkpoints. Recovered cells are marked below.`);
  lines.push("",
    "Percentages below are normalized positions on five described ordinal musical-fit levels. They are not probabilities, confidence, calibrated intensity, or percentage of the song. Score confidence and level distributions remain separate in the result or durable journal for every completed response; unrun tags stay incomplete.", "",
    `Display cutoff: **${percent(cutoff)}** (${cutoffOverride === undefined ? markdown(text(plan.display_cutoff_status, "Missing cutoff status")) : "review override; completed dense scores are unchanged"}). Every completed raw tag score remains in the result or durable journal even when omitted from this shorter display.`, "",
    "## Provider usage", "",
    "Primary averages include selected songs only. Deliberate repeats and semantic probes are separate and the whole-run total counts each started request exactly once. Missing input and output usage stay independently unknown. Per-recording tables below show attempted requests and reported tokens for each recipe; unrun requests are not counted as zero-cost completed work.", "",
    "| Scope | Responses | Input tokens | Output tokens | Average input per primary song | Average output per primary song |", "|---|---:|---:|---:|---:|---:|");
  for (const arm of arms) {
    const value = primaryUsage.get(arm)!;
    const planned = cases.filter(item => item.group !== "probe" && item.repeat_control === false && item.arm === arm).length;
    const average = (kind: "input" | "output"): string => value.responses === planned && value[kind].unknown === 0
      ? (value[kind].tokens / recordings.length).toFixed(1) : "incomplete / unknown";
    lines.push(`| ${markdown(arm)} primary | ${value.responses} | ${usageText(value.input)} | ${usageText(value.output)} | ${average("input")} | ${average("output")} |`);
  }
  lines.push(`| Deliberate repeats | ${repeatUsage.responses} | ${usageText(repeatUsage.input)} | ${usageText(repeatUsage.output)} | — | — |`,
    `| Semantic probes | ${probeUsage.responses} | ${usageText(probeUsage.input)} | ${usageText(probeUsage.output)} | — | — |`,
    `| **Whole run** | ${totalUsage.responses} | ${usageText(totalUsage.input)} | ${usageText(totalUsage.output)} | — | — |`, "");

  lines.push("## Paired arm score differences", "",
    "These are paired model-output changes on the same recordings and tags. Positive means the second arm scored higher. They do not show which arm is more accurate.", "",
    "| Arms | Group | Complete songs | Mean Δ | Mean absolute Δ | Crossed cutoff up / down |",
    "|---|---|---:|---:|---:|---:|");
  for (let left = 0; left < arms.length; left++) for (let right = left + 1; right < arms.length; right++) {
    for (const group of GROUPS) {
      let songs = 0, count = 0, delta = 0, absolute = 0, up = 0, down = 0;
      for (const track of recordingById.keys()) {
        const a = cells.get(cellKey(track, arms[left], false))?.get(group);
        const b = cells.get(cellKey(track, arms[right], false))?.get(group);
        const expected = tagsByGroup.get(group)!.length;
        if (!a || !b || a.scores.size !== expected || b.scores.size !== expected) continue;
        songs++;
        for (const tag of tagsByGroup.get(group)!) {
          const before = a.scores.get(tag.id)!.relevance, after = b.scores.get(tag.id)!.relevance;
          const change = after - before;
          count++; delta += change; absolute += Math.abs(change);
          if (before < cutoff && after >= cutoff) up++;
          if (before >= cutoff && after < cutoff) down++;
        }
      }
      lines.push(`| ${markdown(arms[left])} → ${markdown(arms[right])} | ${group} | ${songs} | ${count ? signedPoints(delta / count) : "incomplete"} | ${count ? `${(absolute / count * 100).toFixed(1)} pp` : "incomplete"} | ${count ? `${up} / ${down}` : "incomplete"} |`);
    }
  }

  lines.push("", "## Deliberate repeat fluctuation", "",
    "Repeats are stability observations only and are excluded from primary arm averages and paired-arm comparisons.", "",
    "| Recording / arm | Compared tags | Any change | ≥5 pp change | Cutoff crossings | Maximum change |",
    "|---|---:|---:|---:|---:|---:|");
  for (const key of [...expectedRepeats].sort()) {
    const grouped = cells.get(key) ?? new Map<Group, Cell>();
    const [trackText, arm] = key.split(":"), track = Number(trackText);
    const primary = cells.get(cellKey(track, arm, false));
    let compared = 0, changed = 0, large = 0, crossings = 0, maximum = 0, complete = true;
    for (const group of GROUPS) {
      const a = primary?.get(group), b = grouped.get(group), expected = tagsByGroup.get(group)!.length;
      if (!a || !b || a.scores.size !== expected || b.scores.size !== expected) { complete = false; continue; }
      for (const tag of tagsByGroup.get(group)!) {
        const before = a.scores.get(tag.id)!.relevance, after = b.scores.get(tag.id)!.relevance;
        const difference = Math.abs(after - before);
        compared++; if (difference > 0) changed++; if (difference >= 0.05) large++;
        if ((before < cutoff) !== (after < cutoff)) crossings++;
        maximum = Math.max(maximum, difference);
      }
    }
    const recording = recordingById.get(track)!;
    const recovered = [...grouped.values()].some(cell => cell.recoveredCases > 0) ? " (journal-recovered)" : "";
    lines.push(`| ${markdown(text(recording.display_name, "Missing display name"))} / ${markdown(arm)}${recovered} | ${complete ? compared : `incomplete (${compared})`} | ${changed} | ${large} | ${crossings} | ${percent(maximum)} |`);
  }

  lines.push("", "## Per-recording review", "");
  for (const [track, recording] of recordingById) {
    const displayName = text(recording.display_name, "Missing display name");
    lines.push(`### [${markdown(displayName)}](<${linkTarget(text(recording.source_path, "Missing source path"))}>) (track ${track})`, "",
      "Reported usage for this recording. Partial rows cover attempted requests only; repeats remain separate.", "",
      "| Recipe | Requests attempted / planned | Input tokens | Output tokens |", "|---|---:|---:|---:|");
    for (const repeat of [false, true]) for (const arm of arms) {
      const key = cellKey(track, arm, repeat), partitions = plannedCases.get(key);
      if (!partitions) continue;
      const planned = [...partitions.values()].reduce((sum, count) => sum + count, 0);
      const value = perRecordingUsage.get(key);
      const label = `${markdown(arm)} ${repeat ? "repeat" : "primary"}`;
      const usageColumn = (kind: "input" | "output"): string => !value ? "not run"
        : `${usageText(value[kind])}${value.attempts < planned ? " (partial)" : ""}`;
      lines.push(`| ${label} | ${value?.attempts ?? 0} / ${planned} | ${usageColumn("input")} | ${usageColumn("output")} |`);
    }
    lines.push("", "| Group | labels3 | labels8 | labels8 physical | labels8 temporal |", "|---|---|---|---|---|");
    for (const group of GROUPS) {
      const rendered = arms.map(arm => renderGroupCell(cells.get(cellKey(track, arm, false))?.get(group), tagsByGroup.get(group)!, cutoff));
      lines.push(`| ${group} | ${rendered.join(" | ")} |`);
    }
    lines.push("", "Period recommendation (at most one; 50% minimum and 15 percentage-point lead):");
    for (const arm of arms) {
      lines.push(`- ${markdown(arm)}: ${periodRecommendation(cells.get(cellKey(track, arm, false))?.get("period"), tagsByGroup.get("period")!)}`);
    }
    lines.push("", "Period audit — all retained era-fit scores:", "",
      `| Period tag | ${arms.map(markdown).join(" | ")} |`, `|---|${arms.map(() => "---:").join("|")}|`);
    for (const tag of tagsByGroup.get("period")!) {
      lines.push(`| ${markdown(tag.name)} | ${arms.map(arm => {
        const cell = cells.get(cellKey(track, arm, false))?.get("period"), score = cell?.scores.get(tag.id);
        return score ? percent(score.relevance) : "incomplete";
      }).join(" | ")} |`);
    }
    lines.push("", "Owner review (leaving a tag blank does not make it a negative label):", "",
      "- Useful displayed tags: ____________________",
      "- Wrong displayed tags: ____________________",
      "- Scores that feel too high or too low: ____________________",
      "- Missing tags or vocabulary concepts: ____________________",
      "- Period assessment: ____________________",
      "- Other listening notes: ____________________", "");
  }
  return lines.join("\n") + "\n";
}

function renderGroupCell(cell: Cell | undefined, expected: Tag[], cutoff: number): string {
  if (!cell) return "**Incomplete (0 scores)**";
  const shown = expected.filter(tag => (cell.scores.get(tag.id)?.relevance ?? -1) >= cutoff)
    .sort((left, right) => cell.scores.get(right.id)!.relevance - cell.scores.get(left.id)!.relevance
      || left.id.localeCompare(right.id))
    .map(tag => `${markdown(tag.name)} ${percent(cell.scores.get(tag.id)!.relevance)}`);
  const complete = cell.scores.size === expected.length && cell.completedCases === cell.plannedCases;
  const values = shown.length ? shown.join("<br>") : "—";
  const recovered = cell.recoveredCases > 0
    ? `*Journal-recovered: ${cell.recoveredCases}/${cell.completedCases} completed partition${cell.recoveredCases === 1 ? "" : "s"}*<br>` : "";
  return complete ? `${recovered}${values}`
    : `${recovered}**Incomplete (${cell.scores.size}/${expected.length} scores)**<br>${values}`;
}

function periodRecommendation(cell: Cell | undefined, periods: Tag[]): string {
  if (!cell || cell.scores.size !== periods.length || cell.completedCases !== cell.plannedCases) return "incomplete";
  const ranked = periods.map(tag => ({ tag, relevance: cell.scores.get(tag.id)!.relevance }))
    .sort((left, right) => right.relevance - left.relevance || left.tag.id.localeCompare(right.tag.id));
  const lead = ranked[0].relevance - ranked[1].relevance;
  if (ranked[0].relevance >= 0.5 && lead >= 0.15) {
    return `**${markdown(ranked[0].tag.name)} ${percent(ranked[0].relevance)}** (lead ${percent(lead)})`;
  }
  return `ambiguous / none (${markdown(ranked[0].tag.name)} ${percent(ranked[0].relevance)}, ${markdown(ranked[1].tag.name)} ${percent(ranked[1].relevance)})`;
}

function isObject(value: unknown): value is ObjectValue {
  return value !== null && typeof value === "object" && !Array.isArray(value);
}

export function main(args: string[]): void {
  const { values } = parseArgs({
    args,
    options: {
      plan: { type: "string" }, result: { type: "string" }, journal: { type: "string" },
      output: { type: "string" }, cutoff: { type: "string" },
    },
  });
  const requiredKeys = ["plan", "result", "journal", "output"] as const;
  assert(requiredKeys.every(key => values[key]),
    "Usage: --plan PLAN --result RESULT --journal REQUESTS_JSONL --output NEW_MARKDOWN [--cutoff 0..1]");
  const required = (key: "plan" | "result" | "journal" | "output"): string => {
    const value = values[key]; assert(value); return value;
  };
  const cutoff = values.cutoff === undefined ? undefined : Number(values.cutoff);
  if (cutoff !== undefined) assert(Number.isFinite(cutoff), "Invalid cutoff override");
  const report = renderGradedReport(
    JSON.parse(bounded(required("plan"), 64 * 1024 * 1024)),
    JSON.parse(bounded(required("result"), 32 * 1024 * 1024)),
    bounded(required("journal"), 256 * 1024 * 1024), cutoff,
  );
  fs.writeFileSync(required("output"), report, { flag: "wx" });
}

if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) {
  try {
    main(process.argv.slice(2));
  } catch {
    console.error("Jev graded report failed. Check exact plan/result/journal membership and completion.");
    process.exitCode = 1;
  }
}
