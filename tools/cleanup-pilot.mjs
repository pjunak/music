// Offline catalog evaluation. Never reads audio, contacts providers or applies proposals.
import { readFile, stat, writeFile } from "node:fs/promises";
import { createHash } from "node:crypto";
import { pathToFileURL } from "node:url";

const COHORT_SCHEMA = "library-cleanup-pilot/v1";
const RESULT_SCHEMA = "library-cleanup-enrichment/v1";
const TEXT_FIELDS = ["title", "artist", "album", "album_artist", "genre"];
const NUMBER_FIELDS = ["track_no", "disc_no", "year"];
const hash = (value) => createHash("sha256").update(JSON.stringify(value)).digest("hex");
const validId = (id) => Number.isSafeInteger(id) && id > 0;
const object = (value) => value !== null && typeof value === "object" && !Array.isArray(value);
const name = (value) => typeof value === "string" && value.trim().length > 0 && value.length <= 128;
const mbid = (value) => typeof value === "string" && /^[0-9a-f]{8}(?:-[0-9a-f]{4}){3}-[0-9a-f]{12}$/iu.test(value);
const same = (left, right) => left === right;
const ratio = (count, total) => total ? count / total : null;
function requireValue(condition, message) { if (!condition) throw new Error(message); }
function validField(field, value) {
  return TEXT_FIELDS.includes(field) ? typeof value === "string" && value.length <= 4096
    : NUMBER_FIELDS.includes(field) && (value === null || Number.isSafeInteger(value) && value >= 0 && value <= 0xffffffff);
}
function validLabels(value) {
  return value === null || Array.isArray(value) && value.length <= 100 && value.every(mbid)
    && new Set(value.map((id) => id.toLowerCase())).size === value.length;
}

export function createCohort(manifest) {
  requireValue(Array.isArray(manifest) && manifest.length >= 2 && manifest.length <= 500, "Supply 2–500 tracks grouped by artist/release family.");
  const ids = new Set();
  const tracks = manifest.map((track) => {
    requireValue(object(track) && validId(track.track_id) && !ids.has(track.track_id), "Invalid or duplicate manifest track ID.");
    requireValue(name(track.family) && name(track.stratum), "Each track needs a family and stratum, independently assigned before running retrieval.");
    ids.add(track.track_id);
    return { track_id: track.track_id, family: track.family.trim().normalize("NFC").toLowerCase(), stratum: track.stratum.trim().normalize("NFC").toLowerCase() };
  }).sort((a, b) => a.track_id - b.track_id);
  const families = [...new Set(tracks.map((track) => track.family))].sort((a, b) => hash(a).localeCompare(hash(b)));
  requireValue(families.length >= 2, "Use at least two independent artist/release families; one family cannot span both splits.");
  const holdout = new Set(families.slice(0, Math.max(1, Math.floor(families.length / 5))));
  return { schema_version: COHORT_SCHEMA, manifest_fingerprint: hash(tracks), tracks: tracks.map((track) => ({
    ...track, split: holdout.has(track.family) ? "holdout" : "development", reviewed: false,
    expected_recording_mbids: null, expected_release_mbids: null, fields: {}, evidence_notes: "",
  })) };
}

function validateCohort(cohort) {
  requireValue(cohort?.schema_version === COHORT_SCHEMA, "Expected a cleanup pilot cohort.");
  const expected = createCohort(cohort.tracks);
  requireValue(cohort.manifest_fingerprint === expected.manifest_fingerprint, "Preserve the fixed manifest; prepare a new cohort when scope changes.");
  const splits = new Map(expected.tracks.map((track) => [track.track_id, track.split]));
  for (const track of cohort.tracks) {
    requireValue(track.split === splits.get(track.track_id), "Preserve the family-based development/holdout split.");
    requireValue(track.reviewed === true && name(track.evidence_notes), "Finish independent labels and short evidence notes for every track before scoring.");
    requireValue(validLabels(track.expected_recording_mbids) && validLabels(track.expected_release_mbids), "Expected IDs must be MBID arrays, or null for unknown labels.");
    requireValue(object(track.fields) && Object.keys(track.fields).length <= 8, "Invalid field judgments.");
    for (const [field, judgment] of Object.entries(track.fields)) {
      requireValue(object(judgment) && validField(field, judgment.current) && Array.isArray(judgment.acceptable)
        && judgment.acceptable.length > 0 && judgment.acceptable.length <= 20 && judgment.acceptable.every((value) => validField(field, value)), "Each known field needs its original value and acceptable values of the correct type.");
    }
  }
}

function readPlans(run) {
  const result = run?.schema === RESULT_SCHEMA ? run : run?.result;
  requireValue(result?.schema === RESULT_SCHEMA && Array.isArray(result.plans) && result.plans.length <= 500, "Supply a cleanup enrichment result or its job response, not an apply/rollback journal.");
  const plans = new Map();
  for (const plan of result.plans) {
    requireValue(object(plan) && validId(plan.track_id) && !plans.has(plan.track_id), "Invalid or duplicate result track ID.");
    requireValue(["identified", "fingerprinted", "unmatched", "failed"].includes(plan.status), "Unknown result status.");
    requireValue(plan.partial === undefined || typeof plan.partial === "boolean", "Invalid partial-result flag.");
    const identified = ["identified", "fingerprinted"].includes(plan.status);
    requireValue(identified ? object(plan.identity) && mbid(plan.identity.recording_mbid)
      && (plan.identity.release_mbid == null || mbid(plan.identity.release_mbid)) : plan.identity == null, "Identity contradicts result status or contains invalid IDs.");
    requireValue(plan.candidates === undefined || Array.isArray(plan.candidates) && plan.candidates.length <= 500 && plan.candidates.every((candidate) => object(candidate) && mbid(candidate.id)), "Invalid recording candidates.");
    requireValue(Array.isArray(plan.ops) && plan.ops.length <= 8 && (identified || plan.ops.length === 0), "Invalid metadata operations for this status.");
    const fields = new Set();
    for (const op of plan.ops) {
      requireValue(object(op) && op.kind === "tag" && op.track_id === plan.track_id && !fields.has(op.field)
        && validField(op.field, op.old) && validField(op.field, op.new) && !same(op.old, op.new), "Invalid, unchanged or duplicate field proposal.");
      fields.add(op.field);
    }
    plans.set(plan.track_id, plan);
  }
  return plans;
}

function scoreTracks(tracks, plans) {
  const counts = { tracks: tracks.length, families: new Set(tracks.map((track) => track.family)).size,
    missing_results: 0, failed_results: 0, partial_results: 0, abstained_results: 0,
    known_recordings: 0, retrieved_recordings: 0, known_releases: 0,
    recording_proposals: 0, correct_recordings: 0, wrong_recordings: 0, unscored_recordings: 0,
    release_proposals: 0, correct_releases: 0, wrong_releases: 0, unscored_releases: 0,
    field_proposals: 0, correct_fields: 0, wrong_fields: 0, unscored_fields: 0,
    damaged_correct_fields: 0, changed_already_correct_fields: 0, needed_corrections: 0, useful_corrections: 0 };
  for (const track of tracks) {
    const plan = plans.get(track.track_id);
    const recordings = track.expected_recording_mbids?.map((id) => id.toLowerCase());
    const releases = track.expected_release_mbids?.map((id) => id.toLowerCase());
    if (recordings?.length) counts.known_recordings++;
    if (releases?.length) counts.known_releases++;
    counts.needed_corrections += Object.values(track.fields).filter((judgment) => !judgment.acceptable.includes(judgment.current)).length;
    if (!plan) { counts.missing_results++; continue; }
    if (plan.partial) counts.partial_results++;
    if (plan.status === "failed") { counts.failed_results++; continue; }
    if (plan.status === "unmatched") counts.abstained_results++;
    const retrieved = new Set((plan.candidates ?? []).map((candidate) => candidate.id.toLowerCase()));
    if (plan.identity) retrieved.add(plan.identity.recording_mbid.toLowerCase());
    if (recordings?.some((id) => retrieved.has(id))) counts.retrieved_recordings++;
    for (const [kind, labels, prediction] of [["recording", recordings, plan.identity?.recording_mbid], ["release", releases, plan.identity?.release_mbid]]) {
      if (!prediction) continue;
      counts[`${kind}_proposals`]++;
      counts[`${labels === undefined ? "unscored" : labels.includes(prediction.toLowerCase()) ? "correct" : "wrong"}_${kind}s`]++;
    }
    for (const op of plan.ops) {
      counts.field_proposals++;
      const judgment = track.fields[op.field];
      if (!judgment) { counts.unscored_fields++; continue; }
      requireValue(same(judgment.current, op.old), `Track ${track.track_id} field ${op.field}: labels and run use different original metadata snapshots.`);
      const correct = judgment.acceptable.includes(op.new);
      const alreadyCorrect = judgment.acceptable.includes(judgment.current);
      counts[correct ? "correct_fields" : "wrong_fields"]++;
      if (alreadyCorrect) {
        counts.changed_already_correct_fields++;
        if (!correct) counts.damaged_correct_fields++;
      } else if (correct) counts.useful_corrections++;
    }
  }
  return { ...counts, recording_precision: ratio(counts.correct_recordings, counts.correct_recordings + counts.wrong_recordings),
    release_precision: ratio(counts.correct_releases, counts.correct_releases + counts.wrong_releases),
    retrieval_coverage: ratio(counts.retrieved_recordings, counts.known_recordings),
    recording_coverage: ratio(counts.correct_recordings, counts.known_recordings),
    field_precision: ratio(counts.correct_fields, counts.correct_fields + counts.wrong_fields),
    correction_coverage: ratio(counts.useful_corrections, counts.needed_corrections) };
}

export function scorePilot(cohort, run) {
  validateCohort(cohort);
  const plans = readPlans(run);
  const ids = new Set(cohort.tracks.map((track) => track.track_id));
  return { schema_version: "library-cleanup-pilot-score/v1", cohort_fingerprint: hash(cohort), run_fingerprint: hash(run),
    scope_note: "Proposal accuracy, not applied edits. Unknown labels are unscored; missing/failed runs are not abstentions. Retrieval includes returned candidates or an identified recording. Related tracks are not independent observations. No automatic pass threshold.",
    extra_result_tracks: [...plans.keys()].filter((id) => !ids.has(id)).length,
    all: scoreTracks(cohort.tracks, plans),
    splits: Object.fromEntries(["development", "holdout"].map((split) => {
      const tracks = cohort.tracks.filter((track) => track.split === split);
      const strata = [...new Set(tracks.map((track) => track.stratum))].sort();
      return [split, { all: scoreTracks(tracks, plans), strata: Object.fromEntries(strata.map((stratum) => [stratum, scoreTracks(tracks.filter((track) => track.stratum === stratum), plans)])) }];
    })) };
}

async function readJson(path) {
  requireValue((await stat(path)).size <= 10 * 1024 * 1024, "Pilot input exceeds 10 MiB.");
  return JSON.parse(await readFile(path, "utf8"));
}

export async function main(args) {
  if (args[0] === "init" && args.length === 3) {
    await writeFile(args[2], `${JSON.stringify(createCohort(await readJson(args[1])), null, 2)}\n`, { flag: "wx" });
  } else if (args[0] === "score" && args.length === 3) {
    const [cohort, run] = await Promise.all(args.slice(1).map(readJson));
    process.stdout.write(`${JSON.stringify(scorePilot(cohort, run), null, 2)}\n`);
  } else throw new Error("Usage: node tools/cleanup-pilot.mjs init manifest.json cohort.json | score cohort.json run.json");
}

if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) {
  main(process.argv.slice(2)).catch((error) => { process.stderr.write(`${error.message}\n`); process.exitCode = 1; });
}
