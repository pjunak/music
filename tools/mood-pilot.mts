// Offline, private listening judgments. Never calls providers or writes library tags.
import { readFile, stat, writeFile } from "node:fs/promises";
import { createHash } from "node:crypto";
import { pathToFileURL } from "node:url";

export const SCHEMA = "song-mood-judgments/v2";
export const INVENTORY_SCHEMA = "song-mood-inventory/v1";
const MAX_TRACKS = 1000;
type JudgmentState = "positive" | "negative" | "uncertain" | "unjudged";
type Category = "all" | "mood" | "session_use" | "period" | "custom";
type PilotSplit = "development" | "confirmation";
export interface MoodVocabularyTag { id: string; name: string }
export interface MoodVocabularyGroup { key: string; tags: MoodVocabularyTag[] }
export interface MoodVocabulary { groups: MoodVocabularyGroup[]; [key: string]: unknown }
export interface MoodInventory { schema_version: string; track_ids: number[] }
export interface PilotManifest {
  kind: "manifest";
  schema_version: string;
  seed: string;
  vocabulary_fingerprint: string;
  annotator: string;
  core_tag_ids: string[];
  separate_by: string[];
  confirmation_fraction: number;
  session_requests: string[];
  selection_notes: string;
  partition_fingerprint: string | null;
}
export interface PilotJudgment {
  kind: "judgment";
  track_id: number;
  file_reference: string;
  recording_group: string;
  duplicate_group: string | null;
  album: string | null;
  composers: string[];
  split: PilotSplit | null;
  duration_seconds: number | null;
  scope: "whole_track" | "excerpt";
  listened_intervals: Array<[number, number]>;
  blind: boolean | null;
  reviewed: boolean;
  labels: Record<string, JudgmentState>;
  session_notes: string;
  notes: string;
}
export type PilotRows = [PilotManifest, ...PilotJudgment[]];
export interface MoodRunResult { track_id: number; tags: string[]; source_signature: string }
export interface MoodRun { schema_version: string; run_id?: string; track_results: MoodRunResult[]; usage?: Record<string, number> }
interface IndexedPrediction { tags: string[]; source_signature: string }
interface VocabularyIndex {
  tags: Map<string, { name: string; category: Exclude<Category, "all"> }>;
  names: Map<string, string>;
  fingerprint: string;
}
interface CountMap {
  tracks: number;
  missing_results: number;
  empty_results: number;
  proposed_tags: number;
  useful_tags: number;
  false_positive_tags: number;
  uncertain_proposals: number;
  unjudged_proposals: number;
  positive_judgments: number;
  negative_judgments: number;
  uncertain_judgments: number;
  unjudged_judgments: number;
  eligible_tracks: number;
  covered_tracks: number;
}
interface Metrics extends CountMap {
  missed_positive_tags: number;
  precision: number | null;
  recall: number | null;
  useful_track_coverage: number | null;
  proposal_judgment_coverage: number | null;
  judgment_coverage: number | null;
}
type MetricMap = Record<string, number | null>;
interface IntervalReport extends Record<string, unknown> {
  precision: [number, number] | null;
  recall: [number, number] | null;
  useful_track_coverage: [number, number] | null;
  independent_groups: number;
  replicates: number;
  defined_replicates: Record<string, number>;
}
interface DeltaCategory extends Record<string, unknown> {
  precision: number | null;
  recall: number | null;
  useful_track_coverage: number | null;
  bootstrap_95: IntervalReport;
}

const STATES = new Set<JudgmentState>(["positive", "negative", "uncertain", "unjudged"]);
const CATEGORIES: Category[] = ["all", "mood", "session_use", "period", "custom"];
const check: (ok: unknown, message: string) => asserts ok = (ok, message) => { if (!ok) throw new Error(message); };
const text = (value: unknown, limit = 512): value is string => typeof value === "string" && value.length > 0 && value.length <= limit;
const id = (value: unknown): value is number => typeof value === "number" && Number.isSafeInteger(value) && value > 0;
const unique = (items: unknown): items is unknown[] => Array.isArray(items) && new Set(items).size === items.length;
function canonical(value: unknown): unknown {
  if (Array.isArray(value)) return value.map(canonical);
  if (value && typeof value === "object") {
    const record = value as Record<string, unknown>;
    return Object.fromEntries(Object.keys(record).sort().map((key) => [key, canonical(record[key])]));
  }
  return value;
}
const hash = (value: unknown): string => createHash("sha256").update(JSON.stringify(canonical(value))).digest("hex");

function vocabularyIndex(vocabulary: MoodVocabulary): VocabularyIndex {
  check(Array.isArray(vocabulary?.groups), "Supply the vocabulary used for these judgments.");
  const tags = new Map<string, { name: string; category: Exclude<Category, "all"> }>(), names = new Map<string, string>();
  for (const group of vocabulary.groups) {
    check(text(group.key) && Array.isArray(group.tags), "Invalid vocabulary group.");
    const category: Exclude<Category, "all"> = ["scene", "setting"].includes(group.key) ? "session_use" : group.key === "mood" || group.key === "period" ? group.key : "custom";
    for (const tag of group.tags) {
      check(text(tag.id, 128) && text(tag.name, 128) && !tags.has(tag.id) && !names.has(tag.name), "Vocabulary IDs and names must be unique.");
      tags.set(tag.id, { name: tag.name, category }); names.set(tag.name, tag.id);
    }
  }
  check(tags.size > 0 && tags.size <= 1200, "Vocabulary must contain 1-1200 tags.");
  return { tags, names, fingerprint: hash(vocabulary) };
}

function runIndex(run: MoodRun, names: Map<string, string>): Map<number, IndexedPrediction> {
  check(run?.schema_version === "assistant-mood-run-export/v1" && Array.isArray(run.track_results) && run.track_results.length <= 5000, "Expected a retained mood-run export.");
  const results = new Map();
  for (const row of run.track_results) {
    check(id(row.track_id) && !results.has(row.track_id) && unique(row.tags) && row.tags.length <= 8, "Invalid or duplicate run result.");
    check(row.tags.every((tag) => names.has(tag)), "Run contains a tag absent from this vocabulary.");
    check(text(row.source_signature), "Every retained result needs its source signature.");
    results.set(row.track_id, { tags: row.tags.map((tag) => {
      const tagId = names.get(tag); check(tagId, "Run contains a tag absent from this vocabulary."); return tagId;
    }), source_signature: row.source_signature });
  }
  return results;
}

export function createPilot(inventory: unknown, vocabulary: MoodVocabulary): PilotRows {
  const { fingerprint } = vocabularyIndex(vocabulary);
  check(inventory !== null && typeof inventory === "object" && !Array.isArray(inventory), "Expected an explicit listening inventory, not model results. Export selected tracks before running the tagger.");
  const candidate = inventory as Partial<MoodInventory> & Record<string, unknown>;
  check(candidate.schema_version === INVENTORY_SCHEMA && Object.keys(candidate).every((key) => ["schema_version", "track_ids"].includes(key)), "Expected an explicit listening inventory, not model results. Export selected tracks before running the tagger.");
  const trackIds = candidate.track_ids;
  check(unique(trackIds) && trackIds.length >= 2 && trackIds.length <= MAX_TRACKS && trackIds.every(id), "Select 2-1000 unique positive library track IDs; aim for 60-100 varied recordings.");
  const validTrackIds = trackIds.filter(id);
  const manifest: PilotManifest = { kind: "manifest", schema_version: SCHEMA, seed: "music-listening-1", vocabulary_fingerprint: fingerprint,
    annotator: "", core_tag_ids: [], separate_by: [], confirmation_fraction: 0.3,
    session_requests: [], selection_notes: "", partition_fingerprint: null };
  const tracks: PilotJudgment[] = validTrackIds.sort((a, b) => a - b).map((track_id) => ({ kind: "judgment", track_id,
    file_reference: "", recording_group: "", duplicate_group: null, album: null, composers: [],
    split: null, duration_seconds: null, scope: "whole_track", listened_intervals: [], blind: null, reviewed: false,
    labels: {}, session_notes: "", notes: "" }));
  return [manifest, ...tracks];
}

export function parsePilot(content: string): PilotRows {
  check(Buffer.byteLength(content) <= 10 * 1024 * 1024, "Pilot input exceeds 10 MiB.");
  const lines = content.trim().split(/\r?\n/);
  check(lines.length >= 3 && lines.length <= MAX_TRACKS + 1, "Expected a manifest and 2-1000 judgment rows in JSONL.");
  return lines.map((line, index) => { try { return JSON.parse(line); } catch { throw new Error(`Invalid JSON on pilot line ${index + 1}.`); } }) as PilotRows;
}
export const serializePilot = (rows: readonly unknown[]): string => `${rows.map((row) => JSON.stringify(row)).join("\n")}\n`;

function validatePilot(rows: PilotRows, vocabulary: MoodVocabulary) {
  check(Array.isArray(rows) && rows.length >= 3 && rows.length <= MAX_TRACKS + 1, "Expected a grouped JSONL pilot.");
  const [manifest, ...tracks] = rows, index = vocabularyIndex(vocabulary);
  check(manifest?.kind === "manifest" && manifest.schema_version === SCHEMA, "Unsupported pilot contract; prepare a new grouped listening pilot.");
  check(manifest.vocabulary_fingerprint === index.fingerprint, "Vocabulary changed; do not mix revisions in a listening pilot.");
  check(text(manifest.seed, 128) && text(manifest.annotator, 128), "Set a split seed and annotator.");
  check(unique(manifest.core_tag_ids) && manifest.core_tag_ids.length > 0 && manifest.core_tag_ids.every((tag) => index.tags.has(tag)), "Select known core tag IDs before freezing the pilot.");
  check(unique(manifest.separate_by) && manifest.separate_by.every((key) => ["album", "composer"].includes(key)), "Unknown grouping dimension.");
  check(Number.isFinite(manifest.confirmation_fraction) && manifest.confirmation_fraction >= 0.1 && manifest.confirmation_fraction <= 0.5, "Confirmation fraction must be between 0.1 and 0.5.");
  check(Array.isArray(manifest.session_requests) && manifest.session_requests.length <= 20 && manifest.session_requests.every((request) => text(request, 2000)), "Session requests must be short descriptions of actual listening needs.");
  const ids = new Set();
  for (const track of tracks) {
    check(track?.kind === "judgment" && id(track.track_id) && !ids.has(track.track_id), "Invalid or duplicate pilot track ID."); ids.add(track.track_id);
    check(text(track.file_reference, 1024) && text(track.recording_group), "Set stable file references and recording groups; related versions/excerpts must share a group.");
    check(track.duplicate_group === null || text(track.duplicate_group), "Invalid duplicate group.");
    check(track.album === null || text(track.album), "Invalid album identity.");
    check(unique(track.composers) && track.composers.length <= 30 && track.composers.every((value) => text(value)), "Invalid composer identities.");
    check(typeof track.duration_seconds === "number" && Number.isFinite(track.duration_seconds) && track.duration_seconds > 0 && track.duration_seconds <= 86400, "Set a finite recording duration in seconds (greater than zero, at most one day) before freezing.");
    check(["whole_track", "excerpt"].includes(track.scope) && typeof track.reviewed === "boolean" && (typeof track.blind === "boolean" || (!track.reviewed && track.blind === null)), "Record listening scope and review status; reviewed tracks need an explicit blinding status.");
    check(Array.isArray(track.listened_intervals) && track.listened_intervals.length <= 100, "Invalid listened intervals.");
    let end = 0;
    for (const interval of track.listened_intervals) {
      check(Array.isArray(interval) && interval.length === 2 && interval.every(Number.isFinite) && interval[0] >= end && interval[1] > interval[0] && interval[1] <= track.duration_seconds, "Intervals must be ordered, disjoint seconds within the recording duration."); end = interval[1];
    }
    check(track.labels && typeof track.labels === "object" && !Array.isArray(track.labels) && Object.entries(track.labels).every(([tag, state]) => index.tags.has(tag) && STATES.has(state)), "Use known tag IDs and positive/negative/uncertain/unjudged labels.");
  }
  return { manifest, tracks, ...index };
}

function groupedTracks(tracks: PilotJudgment[], separateBy: string[]): PilotJudgment[][] {
  const parents = tracks.map((_, index) => index);
  const root = (index: number): number => { while (parents[index] !== index) { parents[index] = parents[parents[index]]; index = parents[index]; } return index; };
  const seen = new Map<string, number>();
  tracks.forEach((track, index) => {
    const keys = [`recording:${track.recording_group}`, `file:${track.file_reference}`];
    if (track.duplicate_group) keys.push(`duplicate:${track.duplicate_group}`);
    if (separateBy.includes("album") && track.album) keys.push(`album:${track.album}`);
    if (separateBy.includes("composer")) keys.push(...track.composers.map((value) => `composer:${value}`));
    for (const key of keys) {
      const previous = seen.get(key);
      if (previous !== undefined) parents[root(index)] = root(previous); else seen.set(key, index);
    }
  });
  const groups = new Map<number, PilotJudgment[]>();
  tracks.forEach((track, index) => { const key = root(index), group = groups.get(key); if (group) group.push(track); else groups.set(key, [track]); });
  return [...groups.values()].map((group) => group.sort((a, b) => a.track_id - b.track_id));
}

function partition(manifest: PilotManifest, tracks: PilotJudgment[]): Map<number, PilotSplit> {
  const groups = groupedTracks(tracks, manifest.separate_by).sort((a, b) => hash([manifest.seed, a.map((t) => t.track_id)]).localeCompare(hash([manifest.seed, b.map((t) => t.track_id)])));
  check(groups.length >= 2, "Grouping leaves fewer than two independent groups; revise the sample, not related-recording identities.");
  const confirmation = Math.max(1, Math.min(groups.length - 1, Math.round(groups.length * manifest.confirmation_fraction)));
  return new Map(groups.flatMap((group, i) => group.map((track): [number, PilotSplit] => [track.track_id, i < confirmation ? "confirmation" : "development"])));
}
function partitionFingerprint(manifest: PilotManifest, tracks: PilotJudgment[]): string {
  return hash({ seed: manifest.seed, vocabulary: manifest.vocabulary_fingerprint, core: [...manifest.core_tag_ids].sort(),
    separate_by: [...manifest.separate_by].sort(), confirmation_fraction: manifest.confirmation_fraction,
    tracks: [...tracks].sort((a, b) => a.track_id - b.track_id).map((t) => [t.track_id, t.file_reference, t.duration_seconds, t.recording_group, t.duplicate_group, t.album, [...t.composers].sort(), t.split]) });
}
export function freezePilot(rows: PilotRows, vocabulary: MoodVocabulary): PilotRows {
  const { manifest, tracks } = validatePilot(rows, vocabulary);
  check(manifest.partition_fingerprint === null && tracks.every((track) => track.split === null), "Pilot is already frozen; do not reshuffle a confirmation cohort.");
  const splits = partition(manifest, tracks);
  const frozen = tracks.map((track) => ({ ...track, split: splits.get(track.track_id) ?? null })).sort((a, b) => a.track_id - b.track_id);
  return [{ ...manifest, partition_fingerprint: partitionFingerprint(manifest, frozen) }, ...frozen];
}

function counts(tracks: PilotJudgment[], predictions: Map<number, IndexedPrediction>, included: string[]): CountMap {
  const result: CountMap = { tracks: tracks.length, missing_results: 0, empty_results: 0, proposed_tags: 0,
    useful_tags: 0, false_positive_tags: 0, uncertain_proposals: 0, unjudged_proposals: 0,
    positive_judgments: 0, negative_judgments: 0, uncertain_judgments: 0, unjudged_judgments: 0, eligible_tracks: 0, covered_tracks: 0 };
  for (const track of tracks) {
    const returned = predictions.get(track.track_id);
    if (!returned) result.missing_results++; else if (returned.tags.length === 0) result.empty_results++;
    const proposals = new Set(returned?.tags ?? []);
    let positive = 0, accepted = 0;
    for (const tag of included) {
      const state = track.labels[tag] ?? "unjudged";
      if (state === "positive") result.positive_judgments++;
      else if (state === "negative") result.negative_judgments++;
      else if (state === "uncertain") result.uncertain_judgments++;
      else result.unjudged_judgments++;
      if (state === "positive") positive++;
      if (!proposals.has(tag)) continue;
      result.proposed_tags++;
      if (state === "positive") { result.useful_tags++; accepted++; }
      else if (state === "negative") result.false_positive_tags++;
      else if (state === "uncertain") result.uncertain_proposals++;
      else result.unjudged_proposals++;
    }
    if (positive) { result.eligible_tracks++; if (accepted) result.covered_tracks++; }
  }
  return result;
}
type SumKey = "useful_tags" | "false_positive_tags" | "positive_judgments" | "eligible_tracks" | "covered_tracks";
type RateKey = "precision" | "recall" | "useful_track_coverage";
function rates(count: Pick<CountMap, SumKey>): Pick<Metrics, RateKey> {
  const judged = count.useful_tags + count.false_positive_tags;
  return { precision: judged ? count.useful_tags / judged : null,
    recall: count.positive_judgments ? count.useful_tags / count.positive_judgments : null,
    useful_track_coverage: count.eligible_tracks ? count.covered_tracks / count.eligible_tracks : null };
}
function metrics(count: CountMap): Metrics {
  const judged = count.useful_tags + count.false_positive_tags;
  const labels = count.positive_judgments + count.negative_judgments;
  const total = labels + count.uncertain_judgments + count.unjudged_judgments;
  return { ...count, missed_positive_tags: count.positive_judgments - count.useful_tags, ...rates(count),
    proposal_judgment_coverage: count.proposed_tags ? judged / count.proposed_tags : null,
    judgment_coverage: total ? labels / total : null };
}
const RATE_KEYS: RateKey[] = ["precision", "recall", "useful_track_coverage"];
const SUM_KEYS: SumKey[] = ["useful_tags", "false_positive_tags", "positive_judgments", "eligible_tracks", "covered_tracks"];
const difference = (before: number | null, after: number | null): number | null => before === null || after === null ? null : after - before;
function metricDelta(before: object, after: object): MetricMap {
  const afterValues = new Map<string, unknown>(Object.entries(after));
  return Object.fromEntries(Object.entries(before)
    .filter(([, value]) => value === null || typeof value === "number")
    .map(([key, value]) => {
      const beforeValue: number | null = typeof value === "number" ? value : null;
      const afterValue = afterValues.get(key);
      return [key, difference(beforeValue, typeof afterValue === "number" || afterValue === null ? afterValue : null)];
    }));
}
function intervals(groups: PilotJudgment[][], runs: Array<Map<number, IndexedPrediction>>, included: string[], seed: unknown): IntervalReport {
  const samples: Record<string, number[]> = Object.fromEntries(RATE_KEYS.map((key) => [key, []]));
  const aggregates = groups.map((group) => runs.map((predictions) => counts(group, predictions, included)));
  let state = Number.parseInt(hash(seed).slice(0, 8), 16) || 1;
  const random = () => { state ^= state << 13; state ^= state >>> 17; state ^= state << 5; return (state >>> 0) / 4294967296; };
  const replicates = groups.length >= 2 ? 1000 : 0;
  for (let iteration = 0; iteration < replicates; iteration++) {
    const totals: Array<Pick<CountMap, SumKey>> = runs.map(() => ({ useful_tags: 0, false_positive_tags: 0,
      positive_judgments: 0, eligible_tracks: 0, covered_tracks: 0 }));
    for (let i = 0; i < groups.length; i++) {
      // Both candidates receive the same resampled groups, including missing results.
      const paired = aggregates[Math.floor(random() * groups.length)];
      paired.forEach((row, side) => { for (const key of SUM_KEYS) totals[side][key] += row[key]; });
    }
    const values = totals.map(rates);
    for (const key of RATE_KEYS) {
      const value = runs.length === 1 ? values[0][key] : difference(values[0][key], values[1][key]);
      if (value !== null) samples[key].push(value);
    }
  }
  const percentile = (values: number[]): [number, number] | null => {
    values.sort((a, b) => a - b);
    return values.length < 900 ? null : [values[Math.floor(values.length * 0.025)], values[Math.min(values.length - 1, Math.floor(values.length * 0.975))]];
  };
  return { precision: percentile(samples.precision), recall: percentile(samples.recall),
    useful_track_coverage: percentile(samples.useful_track_coverage), independent_groups: groups.length, replicates,
    defined_replicates: Object.fromEntries(RATE_KEYS.map((key) => [key, samples[key].length])) };
}

function prepareFrozenPilot(rows: PilotRows, vocabulary: MoodVocabulary, split: string, mode: string) {
  const context = validatePilot(rows, vocabulary), { manifest, tracks } = context;
  check(["development", "confirmation"].includes(split), "Choose development or confirmation explicitly.");
  check(["independent", "diagnostic"].includes(mode), "Choose independent or diagnostic scoring explicitly.");
  check(manifest.partition_fingerprint === partitionFingerprint(manifest, tracks), "Pilot partition or vocabulary changed after freezing.");
  const splits = partition(manifest, tracks);
  check(tracks.every((track) => track.split === splits.get(track.track_id)), "Pilot groups crossed their frozen split.");
  const selected = tracks.filter((track) => track.split === split).sort((a, b) => a.track_id - b.track_id);
  return { ...context, split, mode, selected, groups: groupedTracks(selected, manifest.separate_by) };
}

function coversWholeTrack(track: PilotJudgment): boolean {
  let coveredThrough = 0;
  for (const [start, end] of track.listened_intervals) {
    if (start !== coveredThrough) return false;
    coveredThrough = end;
  }
  return coveredThrough === track.duration_seconds;
}

// Readiness and scoring must reject the same listening conditions, including in diagnostics.
function listeningBlockers({ selected, mode }: { selected: PilotJudgment[]; mode: string }) {
  const checks: Array<[code: string, message: string, blocked: (track: PilotJudgment) => boolean]> = [
    ["unfinished_listening", "Finish listening judgments and intervals for the selected split.",
      (track) => !track.reviewed || track.listened_intervals.length === 0],
    ["incomplete_whole_track", "Whole-track judgments must cover the complete frozen duration without gaps; record partial listening as excerpt scope.",
      (track) => track.scope === "whole_track" && !coversWholeTrack(track)],
    ["independent_listening_required", "Independent scoring requires blind, whole-track judgments for every track in the selected split. Use --diagnostic for assisted or excerpt judgments; no tracks are dropped.",
      (track) => mode === "independent" && (track.blind !== true || track.scope !== "whole_track")],
  ];
  return checks.map(([code, message, blocked]) => ({ code, message,
    track_ids: selected.filter(blocked).map((track) => track.track_id) })).filter((issue) => issue.track_ids.length > 0);
}

function prepareScoring(rows: PilotRows, vocabulary: MoodVocabulary, split: string, mode: string) {
  const context = prepareFrozenPilot(rows, vocabulary, split, mode);
  const blockers = listeningBlockers(context);
  check(blockers.length === 0, blockers[0]?.message);
  return context;
}

export function pilotReadiness(rows: PilotRows, vocabulary: MoodVocabulary, split = "development", mode = "independent") {
  const context = prepareFrozenPilot(rows, vocabulary, split, mode);
  const { manifest, selected, groups } = context, blockers = listeningBlockers(context);
  const blocked = new Set(blockers.flatMap((issue) => issue.track_ids));
  const coreJudgments: Record<JudgmentState, number> = { positive: 0, negative: 0, uncertain: 0, unjudged: 0 };
  let durationSeconds = 0, listenedSeconds = 0;
  for (const track of selected) {
    check(track.duration_seconds !== null, "Frozen tracks require a duration");
    durationSeconds += track.duration_seconds;
    listenedSeconds += track.listened_intervals.reduce((sum, [start, end]) => sum + end - start, 0);
    for (const tag of manifest.core_tag_ids) coreJudgments[track.labels[tag] ?? "unjudged"]++;
  }
  return { schema_version: "song-mood-readiness/v1", split, assessment_mode: mode,
    partition_fingerprint: manifest.partition_fingerprint, ready_for_scoring: blockers.length === 0,
    tracks: selected.length, independent_groups: groups.length, ready_tracks: selected.length - blocked.size,
    reviewed_tracks: selected.filter((track) => track.reviewed).length,
    duration_seconds: durationSeconds, declared_listened_seconds: listenedSeconds,
    unheard_seconds: Math.max(0, durationSeconds - listenedSeconds),
    core_judgments: coreJudgments, blockers,
    scope_note: "Selected split only; confirmation requires --confirmation. Readiness checks listening declarations and the frozen contract, not model results, judgment quality or production acceptance. Core-label counts include unfinished rows; omitted labels are unjudged, never negative. Even a ready cohort may have no judged labels and produce unknown metrics. Declared time cannot prove listening or blinding; assisted listening cannot be made blind by relabeling it. No provider calls or file changes occur." };
}

function scorePrepared(context: ReturnType<typeof prepareScoring>, run: MoodRun, predictions: Map<number, IndexedPrediction>) {
  const { manifest, tracks, tags, split, mode, selected, groups } = context;
  const report = (included: string[]): Metrics => metrics(counts(selected, predictions, included));
  const categories = Object.fromEntries(CATEGORIES.map((category) => {
    const included = manifest.core_tag_ids.filter((tag) => category === "all" || tags.get(tag)?.category === category);
    return [category, { ...report(included), bootstrap_95: intervals(groups, [predictions], included, [manifest.partition_fingerprint, split, category]) }];
  })) as Record<Category, Metrics & { bootstrap_95: ReturnType<typeof intervals> }>;
  const overlap = (values: (track: PilotJudgment) => string[]): string[] => { const dev = new Set(tracks.filter((t) => t.split === "development").flatMap(values)); return [...new Set(tracks.filter((t) => t.split === "confirmation").flatMap(values))].filter((v) => dev.has(v)).sort(); };
  return { schema_version: "song-mood-score/v2", split, assessment_mode: mode, partition_fingerprint: manifest.partition_fingerprint,
    judgments_fingerprint: hash(selected), run_id: run.run_id ?? null, categories,
    per_tag: Object.fromEntries(manifest.core_tag_ids.map((tag) => [tag, { ...tags.get(tag), ...report([tag]) }])),
    non_core_proposals: selected.reduce((sum, track) => sum + (predictions.get(track.track_id)?.tags.filter((tag) => !manifest.core_tag_ids.includes(tag)).length ?? 0), 0),
    listening: { blind_tracks: selected.filter((t) => t.blind).length, assisted_tracks: selected.filter((t) => !t.blind).length, excerpt_tracks: selected.filter((t) => t.scope === "excerpt").length },
    residual_overlap: { albums: overlap((t) => t.album ? [t.album] : []), composers: overlap((t) => t.composers) },
    run_source_signatures: Object.fromEntries(selected.flatMap((track) => {
      const prediction = predictions.get(track.track_id); return prediction ? [[track.track_id, prediction.source_signature]] : [];
    })),
    scope_note: "Independent mode requires declared blind, complete-recording judgments for the whole selected split. Diagnostic mode includes assisted/excerpt judgments and cannot establish independent whole-recording accuracy. Duration, scope and blinding are listener declarations, not proof of listening. Only judged core tags are scored. Precision uses positive/negative proposals; recall uses known positive judgments, including misses from unavailable results. Uncertain and unjudged labels are masked. Intervals resample whole independent groups, not tracks; small or biased samples do not establish general accuracy. Verify file references against the run before comparing. Usage covers the entire exported run.", usage: run.usage ?? null };
}

export function scorePilot(rows: PilotRows, run: MoodRun, vocabulary: MoodVocabulary, split = "development", mode = "independent") {
  const context = prepareScoring(rows, vocabulary, split, mode);
  return scorePrepared(context, run, runIndex(run, context.names));
}

function assessResult(result: IndexedPrediction | undefined) {
  return { availability: !result ? "missing" : result.tags.length ? "proposed" : "abstained", tags: [...(result?.tags ?? [])].sort() };
}
function compareTrack(track: PilotJudgment, before: IndexedPrediction | undefined, after: IndexedPrediction | undefined, coreTags: string[]) {
  const baseline = assessResult(before), candidate = assessResult(after);
  if (JSON.stringify(baseline) === JSON.stringify(candidate)) return null;
  const added = candidate.tags.filter((tag) => !baseline.tags.includes(tag));
  const removed = baseline.tags.filter((tag) => !candidate.tags.includes(tag));
  const judgments = (ids: string[]) => ids.filter((tag) => coreTags.includes(tag))
    .map((tag_id) => ({ tag_id, judgment: track.labels[tag_id] ?? "unjudged" }));
  const addedCore = judgments(added), removedCore = judgments(removed);
  const improvements: string[] = [], regressions: string[] = [];
  if (!before && after) improvements.push("recovered_result");
  if (before && !after) regressions.push("lost_result");
  if (addedCore.some((tag) => tag.judgment === "positive")) improvements.push("gained_useful_tag");
  if (removedCore.some((tag) => tag.judgment === "positive")) regressions.push("lost_useful_tag");
  if (addedCore.some((tag) => tag.judgment === "negative")) regressions.push("added_false_positive");
  // An unavailable candidate did not make a safer decision.
  if (after && removedCore.some((tag) => tag.judgment === "negative")) improvements.push("avoided_false_positive");
  return { track_id: track.track_id, baseline, candidate,
    added_core_tags: addedCore, removed_core_tags: removedCore,
    added_non_core_tags: added.filter((tag) => !coreTags.includes(tag)),
    removed_non_core_tags: removed.filter((tag) => !coreTags.includes(tag)),
    classification: improvements.length && regressions.length ? "mixed" : regressions.length ? "regression" : improvements.length ? "improvement" : "changed",
    improvements, regressions };
}

export function comparePilot(rows: PilotRows, baselineRun: MoodRun, candidateRun: MoodRun, vocabulary: MoodVocabulary, split = "development", mode = "independent") {
  const context = prepareScoring(rows, vocabulary, split, mode);
  const { manifest, selected, groups, tags, names } = context;
  const before = runIndex(baselineRun, names), after = runIndex(candidateRun, names);
  const baseline = scorePrepared(context, baselineRun, before), candidate = scorePrepared(context, candidateRun, after);
  const categories = Object.fromEntries(CATEGORIES.map((category) => {
    const included = manifest.core_tag_ids.filter((tag) => category === "all" || tags.get(tag)?.category === category);
    return [category, { ...metricDelta(baseline.categories[category], candidate.categories[category]),
      bootstrap_95: intervals(groups, [before, after], included, [manifest.partition_fingerprint, split, category]) }];
  })) as Record<Category, DeltaCategory>;
  const differences = selected.map((track) => compareTrack(track, before.get(track.track_id), after.get(track.track_id), manifest.core_tag_ids))
    .filter((track): track is NonNullable<typeof track> => track !== null);
  return { schema_version: "song-mood-comparison/v2", split, assessment_mode: mode, partition_fingerprint: manifest.partition_fingerprint,
    judgments_fingerprint: baseline.judgments_fingerprint, baseline_run_fingerprint: hash(baselineRun), candidate_run_fingerprint: hash(candidateRun),
    baseline, candidate, delta: { categories,
      per_tag: Object.fromEntries(manifest.core_tag_ids.map((tag) => [tag, metricDelta(baseline.per_tag[tag], candidate.per_tag[tag])])) },
    result_availability: {
      both_present: selected.filter((track) => before.has(track.track_id) && after.has(track.track_id)).length,
      baseline_only: selected.filter((track) => before.has(track.track_id) && !after.has(track.track_id)).length,
      candidate_only: selected.filter((track) => !before.has(track.track_id) && after.has(track.track_id)).length,
      neither_present: selected.filter((track) => !before.has(track.track_id) && !after.has(track.track_id)).length },
    unchanged_tracks: selected.length - differences.length,
    tracks_with_improvements: differences.filter((track) => track.improvements.length).length,
    tracks_with_regressions: differences.filter((track) => track.regressions.length).length,
    differences,
    scope_note: "Independent mode requires declared blind, complete-recording judgments for every selected track. Diagnostic comparisons cannot establish independent whole-recording gains. Selected split only; candidate minus baseline on the same judgments. Rate deltas are fractions and remain null if either denominator is empty. Bootstrap intervals resample the same whole independent groups on both sides; missing results remain in the cohort. Availability and tag signals can overlap; mixed changes need review. Uncertain, unjudged and non-core changes establish no quality gain. Verify frozen file references and record candidate settings before comparing; run fingerprints are audit identities, not proof of matching audio or a controlled experiment. Usage covers entire exports, not just scored tracks. No automatic pass threshold." };
}

async function readBounded(path: string): Promise<string> {
  check((await stat(path)).size <= 10 * 1024 * 1024, "Pilot input exceeds 10 MiB.");
  return readFile(path, "utf8");
}
const readJson = async <T,>(path: string): Promise<T> => JSON.parse(await readBounded(path)) as T;
const USAGE = "Usage: node tools/mood-pilot.mts init inventory.json vocabulary.json draft.jsonl | freeze draft.jsonl vocabulary.json pilot.jsonl | status pilot.jsonl vocabulary.json [--confirmation] [--diagnostic] | score pilot.jsonl run.json vocabulary.json [--confirmation] [--diagnostic] | compare pilot.jsonl baseline-run.json candidate-run.json vocabulary.json [--confirmation] [--diagnostic]";
export async function main(args: string[]): Promise<void> {
  if (args[0] === "init" && args.length === 4) {
    const [inventory, vocabulary] = await Promise.all([readJson<MoodInventory>(args[1]), readJson<MoodVocabulary>(args[2])]);
    await writeFile(args[3], serializePilot(createPilot(inventory, vocabulary)), { flag: "wx" });
  } else if (args[0] === "freeze" && args.length === 4) {
    const [content, vocabulary] = await Promise.all([readBounded(args[1]), readJson<MoodVocabulary>(args[2])]);
    await writeFile(args[3], serializePilot(freezePilot(parsePilot(content), vocabulary)), { flag: "wx" });
  } else if (["status", "score", "compare"].includes(args[0])) {
    const fileCount = args[0] === "status" ? 2 : args[0] === "score" ? 3 : 4;
    const paths = args.slice(1, fileCount + 1), flags = args.slice(fileCount + 1);
    check(paths.length === fileCount && paths.every((path) => !path.startsWith("--")) && unique(flags) && flags.every((flag) => ["--confirmation", "--diagnostic"].includes(flag)), USAGE);
    const content = await readBounded(paths[0]);
    const inputs = await Promise.all(paths.slice(1).map((path) => readJson<unknown>(path)));
    const rows = parsePilot(content), split = flags.includes("--confirmation") ? "confirmation" : "development";
    const mode = flags.includes("--diagnostic") ? "diagnostic" : "independent";
    const result = args[0] === "status" ? pilotReadiness(rows, inputs[0] as MoodVocabulary, split, mode)
      : args[0] === "score" ? scorePilot(rows, inputs[0] as MoodRun, inputs[1] as MoodVocabulary, split, mode)
      : comparePilot(rows, inputs[0] as MoodRun, inputs[1] as MoodRun, inputs[2] as MoodVocabulary, split, mode);
    process.stdout.write(JSON.stringify(result, null, 2) + "\n");
  } else throw new Error(USAGE);
}
if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) {
  main(process.argv.slice(2)).catch((error: unknown) => { process.stderr.write(`${error instanceof Error ? error.message : "Mood pilot failed"}\n`); process.exitCode = 1; });
}
