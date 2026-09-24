// Offline, private listening judgments. Never calls providers or writes library tags.
import { readFile, stat, writeFile } from "node:fs/promises";
import { createHash } from "node:crypto";
import { pathToFileURL } from "node:url";

export const SCHEMA = "song-mood-judgments/v1";
const MAX_TRACKS = 1000;
const STATES = new Set(["positive", "negative", "uncertain", "unjudged"]);
const CATEGORIES = ["all", "mood", "session_use", "period", "custom"];
const check = (ok, message) => { if (!ok) throw new Error(message); };
const text = (value, limit = 512) => typeof value === "string" && value.length > 0 && value.length <= limit;
const id = (value) => Number.isSafeInteger(value) && value > 0;
const unique = (items) => Array.isArray(items) && new Set(items).size === items.length;
function canonical(value) {
  if (Array.isArray(value)) return value.map(canonical);
  if (value && typeof value === "object") return Object.fromEntries(Object.keys(value).sort().map((key) => [key, canonical(value[key])]));
  return value;
}
const hash = (value) => createHash("sha256").update(JSON.stringify(canonical(value))).digest("hex");

function vocabularyIndex(vocabulary) {
  check(Array.isArray(vocabulary?.groups), "Supply the vocabulary used for these judgments.");
  const tags = new Map(), names = new Map();
  for (const group of vocabulary.groups) {
    check(text(group.key) && Array.isArray(group.tags), "Invalid vocabulary group.");
    const category = ["scene", "setting"].includes(group.key) ? "session_use" : ["mood", "period"].includes(group.key) ? group.key : "custom";
    for (const tag of group.tags) {
      check(text(tag.id, 128) && text(tag.name, 128) && !tags.has(tag.id) && !names.has(tag.name), "Vocabulary IDs and names must be unique.");
      tags.set(tag.id, { name: tag.name, category }); names.set(tag.name, tag.id);
    }
  }
  check(tags.size > 0 && tags.size <= 1200, "Vocabulary must contain 1-1200 tags.");
  return { tags, names, fingerprint: hash(vocabulary) };
}

function runIndex(run, names) {
  check(run?.schema_version === "assistant-mood-run-export/v1" && Array.isArray(run.track_results) && run.track_results.length <= 5000, "Expected a retained mood-run export.");
  const results = new Map();
  for (const row of run.track_results) {
    check(id(row.track_id) && !results.has(row.track_id) && unique(row.tags) && row.tags.length <= 8, "Invalid or duplicate run result.");
    check(row.tags.every((tag) => names.has(tag)), "Run contains a tag absent from this vocabulary.");
    check(text(row.source_signature), "Every retained result needs its source signature.");
    results.set(row.track_id, { tags: row.tags.map((tag) => names.get(tag)), source_signature: row.source_signature });
  }
  return results;
}

export function createPilot(run, vocabulary) {
  const { names, fingerprint } = vocabularyIndex(vocabulary);
  const results = runIndex(run, names);
  check(results.size >= 2 && results.size <= MAX_TRACKS, "Select 2-1000 tracks; aim for 60-100 varied recordings.");
  return [{ kind: "manifest", schema_version: SCHEMA, seed: "music-listening-1", vocabulary_fingerprint: fingerprint,
    annotator: "", core_tag_ids: [], separate_by: [], confirmation_fraction: 0.3,
    session_requests: [], selection_notes: "", partition_fingerprint: null },
  ...[...results].sort(([a], [b]) => a - b).map(([track_id]) => ({ kind: "judgment", track_id,
    file_reference: "", recording_group: "", duplicate_group: null, album: null, composers: [],
    split: null, scope: "whole_track", listened_intervals: [], blind: true, reviewed: false,
    labels: {}, session_notes: "", notes: "" }))];
}

export function parsePilot(content) {
  check(Buffer.byteLength(content) <= 10 * 1024 * 1024, "Pilot input exceeds 10 MiB.");
  const lines = content.trim().split(/\r?\n/);
  check(lines.length >= 3 && lines.length <= MAX_TRACKS + 1, "Expected a manifest and 2-1000 judgment rows in JSONL.");
  return lines.map((line, index) => { try { return JSON.parse(line); } catch { throw new Error(`Invalid JSON on pilot line ${index + 1}.`); } });
}
export const serializePilot = (rows) => `${rows.map((row) => JSON.stringify(row)).join("\n")}\n`;

function validatePilot(rows, vocabulary) {
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
    check(["whole_track", "excerpt"].includes(track.scope) && typeof track.blind === "boolean" && typeof track.reviewed === "boolean", "Record listening scope, blinding and review status.");
    check(Array.isArray(track.listened_intervals) && track.listened_intervals.length <= 100, "Invalid listened intervals.");
    let end = 0;
    for (const interval of track.listened_intervals) {
      check(Array.isArray(interval) && interval.length === 2 && interval.every(Number.isFinite) && interval[0] >= end && interval[1] > interval[0] && interval[1] <= 86400, "Intervals must be ordered, disjoint seconds within a day."); end = interval[1];
    }
    check(track.labels && typeof track.labels === "object" && !Array.isArray(track.labels) && Object.entries(track.labels).every(([tag, state]) => index.tags.has(tag) && STATES.has(state)), "Use known tag IDs and positive/negative/uncertain/unjudged labels.");
  }
  return { manifest, tracks, ...index };
}

function groupedTracks(tracks, separateBy) {
  const parents = tracks.map((_, index) => index);
  const root = (index) => { while (parents[index] !== index) { parents[index] = parents[parents[index]]; index = parents[index]; } return index; };
  const seen = new Map();
  tracks.forEach((track, index) => {
    const keys = [`recording:${track.recording_group}`, `file:${track.file_reference}`];
    if (track.duplicate_group) keys.push(`duplicate:${track.duplicate_group}`);
    if (separateBy.includes("album") && track.album) keys.push(`album:${track.album}`);
    if (separateBy.includes("composer")) keys.push(...track.composers.map((value) => `composer:${value}`));
    for (const key of keys) { if (seen.has(key)) parents[root(index)] = root(seen.get(key)); else seen.set(key, index); }
  });
  const groups = new Map();
  tracks.forEach((track, index) => { const key = root(index); if (!groups.has(key)) groups.set(key, []); groups.get(key).push(track); });
  return [...groups.values()].map((group) => group.sort((a, b) => a.track_id - b.track_id));
}

function partition(manifest, tracks) {
  const groups = groupedTracks(tracks, manifest.separate_by).sort((a, b) => hash([manifest.seed, a.map((t) => t.track_id)]).localeCompare(hash([manifest.seed, b.map((t) => t.track_id)])));
  check(groups.length >= 2, "Grouping leaves fewer than two independent groups; revise the sample, not related-recording identities.");
  const confirmation = Math.max(1, Math.min(groups.length - 1, Math.round(groups.length * manifest.confirmation_fraction)));
  return new Map(groups.flatMap((group, i) => group.map((track) => [track.track_id, i < confirmation ? "confirmation" : "development"])));
}
function partitionFingerprint(manifest, tracks) {
  return hash({ seed: manifest.seed, vocabulary: manifest.vocabulary_fingerprint, core: [...manifest.core_tag_ids].sort(),
    separate_by: [...manifest.separate_by].sort(), confirmation_fraction: manifest.confirmation_fraction,
    tracks: [...tracks].sort((a, b) => a.track_id - b.track_id).map((t) => [t.track_id, t.file_reference, t.recording_group, t.duplicate_group, t.album, [...t.composers].sort(), t.split]) });
}
export function freezePilot(rows, vocabulary) {
  const { manifest, tracks } = validatePilot(rows, vocabulary);
  check(manifest.partition_fingerprint === null && tracks.every((track) => track.split === null), "Pilot is already frozen; do not reshuffle a confirmation cohort.");
  const splits = partition(manifest, tracks);
  const frozen = tracks.map((track) => ({ ...track, split: splits.get(track.track_id) })).sort((a, b) => a.track_id - b.track_id);
  return [{ ...manifest, partition_fingerprint: partitionFingerprint(manifest, frozen) }, ...frozen];
}

function counts(tracks, predictions, included) {
  const result = { tracks: tracks.length, missing_results: 0, empty_results: 0, proposed_tags: 0,
    useful_tags: 0, false_positive_tags: 0, uncertain_proposals: 0, unjudged_proposals: 0,
    positive_judgments: 0, negative_judgments: 0, uncertain_judgments: 0, unjudged_judgments: 0, eligible_tracks: 0, covered_tracks: 0 };
  for (const track of tracks) {
    const returned = predictions.get(track.track_id);
    if (!returned) result.missing_results++; else if (returned.tags.length === 0) result.empty_results++;
    const proposals = new Set(returned?.tags ?? []);
    let positive = 0, accepted = 0;
    for (const tag of included) {
      const state = track.labels[tag] ?? "unjudged";
      result[`${state}_judgments`]++;
      if (state === "positive") positive++;
      if (!proposals.has(tag)) continue;
      result.proposed_tags++;
      if (state === "positive") { result.useful_tags++; accepted++; }
      else if (state === "negative") result.false_positive_tags++;
      else result[`${state}_proposals`]++;
    }
    if (positive) { result.eligible_tracks++; if (accepted) result.covered_tracks++; }
  }
  return result;
}
function metrics(count) {
  const judged = count.useful_tags + count.false_positive_tags;
  const labels = count.positive_judgments + count.negative_judgments;
  const total = labels + count.uncertain_judgments + count.unjudged_judgments;
  return { ...count, precision: judged ? count.useful_tags / judged : null,
    proposal_judgment_coverage: count.proposed_tags ? judged / count.proposed_tags : null,
    judgment_coverage: total ? labels / total : null,
    useful_track_coverage: count.eligible_tracks ? count.covered_tracks / count.eligible_tracks : null };
}
function intervals(groups, predictions, included, seed) {
  if (groups.length < 2) return { precision: null, useful_track_coverage: null, independent_groups: groups.length };
  const aggregates = groups.map((group) => counts(group, predictions, included));
  let state = Number.parseInt(hash(seed).slice(0, 8), 16) || 1;
  const random = () => { state ^= state << 13; state ^= state >>> 17; state ^= state << 5; return (state >>> 0) / 4294967296; };
  const samples = { precision: [], useful_track_coverage: [] };
  for (let iteration = 0; iteration < 1000; iteration++) {
    let useful = 0, wrong = 0, eligible = 0, covered = 0;
    for (let i = 0; i < groups.length; i++) { const row = aggregates[Math.floor(random() * groups.length)]; useful += row.useful_tags; wrong += row.false_positive_tags; eligible += row.eligible_tracks; covered += row.covered_tracks; }
    if (useful + wrong) samples.precision.push(useful / (useful + wrong));
    if (eligible) samples.useful_track_coverage.push(covered / eligible);
  }
  const percentile = (values) => { values.sort((a, b) => a - b); return values.length < 900 ? null : [values[Math.floor(values.length * 0.025)], values[Math.min(values.length - 1, Math.floor(values.length * 0.975))]]; };
  return { precision: percentile(samples.precision), useful_track_coverage: percentile(samples.useful_track_coverage), independent_groups: groups.length };
}

export function scorePilot(rows, run, vocabulary, split = "development") {
  const { manifest, tracks, tags, names } = validatePilot(rows, vocabulary);
  check(["development", "confirmation"].includes(split), "Choose development or confirmation explicitly.");
  check(manifest.partition_fingerprint === partitionFingerprint(manifest, tracks), "Pilot partition or vocabulary changed after freezing.");
  const splits = partition(manifest, tracks);
  check(tracks.every((track) => track.split === splits.get(track.track_id)), "Pilot groups crossed their frozen split.");
  const selected = tracks.filter((track) => track.split === split);
  check(selected.every((track) => track.reviewed && track.listened_intervals.length > 0), "Finish independent listening judgments and intervals for the selected split.");
  const predictions = runIndex(run, names);
  const groups = groupedTracks(selected, manifest.separate_by);
  const report = (included) => metrics(counts(selected, predictions, included));
  const categories = Object.fromEntries(CATEGORIES.map((category) => {
    const included = manifest.core_tag_ids.filter((tag) => category === "all" || tags.get(tag).category === category);
    return [category, { ...report(included), bootstrap_95: intervals(groups, predictions, included, [manifest.partition_fingerprint, split, category]) }];
  }));
  const overlap = (values) => { const dev = new Set(tracks.filter((t) => t.split === "development").flatMap(values)); return [...new Set(tracks.filter((t) => t.split === "confirmation").flatMap(values))].filter((v) => dev.has(v)).sort(); };
  return { schema_version: "song-mood-score/v1", split, partition_fingerprint: manifest.partition_fingerprint,
    judgments_fingerprint: hash(selected), run_id: run.run_id ?? null, categories,
    per_tag: Object.fromEntries(manifest.core_tag_ids.map((tag) => [tag, { ...tags.get(tag), ...report([tag]) }])),
    non_core_proposals: selected.reduce((sum, track) => sum + (predictions.get(track.track_id)?.tags.filter((tag) => !manifest.core_tag_ids.includes(tag)).length ?? 0), 0),
    listening: { blind_tracks: selected.filter((t) => t.blind).length, assisted_tracks: selected.filter((t) => !t.blind).length, excerpt_tracks: selected.filter((t) => t.scope === "excerpt").length },
    residual_overlap: { albums: overlap((t) => t.album ? [t.album] : []), composers: overlap((t) => t.composers) },
    run_source_signatures: Object.fromEntries(selected.filter((t) => predictions.has(t.track_id)).map((t) => [t.track_id, predictions.get(t.track_id).source_signature])),
    scope_note: "Only core tags with positive/negative judgments affect precision. Uncertain and unjudged labels are masked. Intervals resample whole independent groups, not tracks; small or biased samples do not establish general accuracy. Verify file references against the run before comparing. Usage covers the entire exported run.", usage: run.usage ?? null };
}

async function readBounded(path) {
  check((await stat(path)).size <= 10 * 1024 * 1024, "Pilot input exceeds 10 MiB.");
  return readFile(path, "utf8");
}
const readJson = async (path) => JSON.parse(await readBounded(path));
export async function main(args) {
  if (args[0] === "init" && args.length === 4) {
    const [run, vocabulary] = await Promise.all(args.slice(1, 3).map(readJson));
    await writeFile(args[3], serializePilot(createPilot(run, vocabulary)), { flag: "wx" });
  } else if (args[0] === "freeze" && args.length === 4) {
    const [content, vocabulary] = await Promise.all([readBounded(args[1]), readJson(args[2])]);
    await writeFile(args[3], serializePilot(freezePilot(parsePilot(content), vocabulary)), { flag: "wx" });
  } else if (args[0] === "score" && (args.length === 4 || (args.length === 5 && args[4] === "--confirmation"))) {
    const [content, run, vocabulary] = await Promise.all([readBounded(args[1]), readJson(args[2]), readJson(args[3])]);
    process.stdout.write(`${JSON.stringify(scorePilot(parsePilot(content), run, vocabulary, args[4] ? "confirmation" : "development"), null, 2)}\n`);
  } else throw new Error("Usage: node tools/mood-pilot.mjs init run.json vocabulary.json draft.jsonl | freeze draft.jsonl vocabulary.json pilot.jsonl | score pilot.jsonl run.json vocabulary.json [--confirmation]");
}
if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) {
  main(process.argv.slice(2)).catch((error) => { process.stderr.write(`${error.message}\n`); process.exitCode = 1; });
}
