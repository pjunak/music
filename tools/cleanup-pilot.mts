// Offline catalog evaluation. Never reads audio, contacts providers or applies proposals.
import { readFile, stat, writeFile } from "node:fs/promises";
import { createHash } from "node:crypto";
import { pathToFileURL } from "node:url";

const COHORT_SCHEMA = "library-cleanup-pilot/v1";
const RESULT_SCHEMA = "library-cleanup-enrichment/v1";
const TEXT_FIELDS = ["title", "artist", "album", "album_artist", "genre", "release_date", "original_release_date", "composer"];
const NUMBER_FIELDS = ["track_no", "disc_no", "year"];
type Split = "development" | "holdout";
type FieldValue = string | number | null;
export interface CleanupManifestTrack { track_id: number; family: string; stratum: string }
export interface FieldJudgment { current: FieldValue; acceptable: FieldValue[] }
export interface CleanupCohortTrack extends CleanupManifestTrack {
  split: Split;
  reviewed: boolean;
  expected_recording_mbids: string[] | null;
  expected_release_mbids: string[] | null;
  fields: Record<string, FieldJudgment>;
  evidence_notes: string;
}
export interface CleanupCohort { schema_version: string; manifest_fingerprint: string; tracks: CleanupCohortTrack[] }
export interface CleanupIdentity { recording_mbid: string; release_mbid: string | null }
export interface CleanupCandidate { id: string }
export interface CleanupOperation { track_id: number; kind: "tag"; field: string; old: FieldValue; new: FieldValue }
export interface CleanupPlan {
  track_id: number;
  status: "identified" | "fingerprinted" | "unmatched" | "failed";
  partial?: boolean;
  identity: CleanupIdentity | null;
  candidates?: CleanupCandidate[];
  ops: CleanupOperation[];
}
export interface CleanupRun { schema: string; plans: CleanupPlan[] }
interface CleanupJobResponse { result: CleanupRun }
interface IdentityAssessment { value: string | null; state: string }
interface FieldAssessment { state: string; proposal: { old: FieldValue; new: FieldValue } | null }
interface TrackAssessment {
  availability: string;
  recording: IdentityAssessment;
  release: IdentityAssessment;
  retrieval: string;
  fields: Record<string, FieldAssessment>;
}

const hash = (value: unknown): string => createHash("sha256").update(JSON.stringify(value)).digest("hex");
const compareCodeUnits = (left: string, right: string): number => left < right ? -1 : left > right ? 1 : 0;
const validId = (id: unknown): id is number => typeof id === "number" && Number.isSafeInteger(id) && id > 0;
const array = (value: unknown): value is unknown[] => Array.isArray(value);
const object = <T,>(value: T): value is T & Record<string, unknown> => value !== null && typeof value === "object" && !Array.isArray(value);
const name = (value: unknown): value is string => typeof value === "string" && value.trim().length > 0 && value.length <= 128;
const mbid = (value: unknown): value is string => typeof value === "string" && /^[0-9a-f]{8}(?:-[0-9a-f]{4}){3}-[0-9a-f]{12}$/iu.test(value);
const same = (left: FieldValue, right: FieldValue): boolean => left === right;
const ratio = (count: number, total: number): number | null => total ? count / total : null;
function requireValue(condition: unknown, message: string): asserts condition { if (!condition) throw new Error(message); }
function validField(field: string, value: unknown): value is FieldValue {
  return TEXT_FIELDS.includes(field) ? typeof value === "string" && value.length <= 4096
    : NUMBER_FIELDS.includes(field) && (value === null || typeof value === "number" && Number.isSafeInteger(value) && value >= 0 && value <= 0xffffffff);
}
function validLabels(value: unknown): value is string[] | null {
  return value === null || Array.isArray(value) && value.length <= 100 && value.every(mbid)
    && new Set(value.map((id) => id.toLowerCase())).size === value.length;
}

export function createCohort(manifest: CleanupManifestTrack[]): CleanupCohort {
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

function validateCohort(cohort: CleanupCohort, reviewedSplit: Split | null = null): void {
  requireValue(cohort?.schema_version === COHORT_SCHEMA, "Expected a cleanup pilot cohort.");
  const expected = createCohort(cohort.tracks);
  requireValue(cohort.manifest_fingerprint === expected.manifest_fingerprint, "Preserve the fixed manifest; prepare a new cohort when scope changes.");
  const splits = new Map(expected.tracks.map((track) => [track.track_id, track.split]));
  for (const track of cohort.tracks) {
    requireValue(track.split === splits.get(track.track_id), "Preserve the family-based development/holdout split.");
    if (reviewedSplit === null || track.split === reviewedSplit) {
      requireValue(track.reviewed === true && name(track.evidence_notes), "Finish independent labels and short evidence notes for every scored track.");
    }
    requireValue(validLabels(track.expected_recording_mbids) && validLabels(track.expected_release_mbids), "Expected IDs must be MBID arrays, or null for unknown labels.");
    requireValue(object(track.fields) && Object.keys(track.fields).length <= TEXT_FIELDS.length + NUMBER_FIELDS.length, "Invalid field judgments.");
    for (const [field, judgment] of Object.entries(track.fields)) {
      requireValue(object(judgment) && validField(field, judgment.current) && Array.isArray(judgment.acceptable)
        && judgment.acceptable.length > 0 && judgment.acceptable.length <= 20 && judgment.acceptable.every((value) => validField(field, value)), "Each known field needs its original value and acceptable values of the correct type.");
    }
  }
}

function assertCleanupPlan(plan: unknown, plans: ReadonlyMap<number, CleanupPlan>): asserts plan is CleanupPlan {
  requireValue(object(plan) && validId(plan.track_id) && !plans.has(plan.track_id), "Invalid or duplicate result track ID.");
  requireValue(plan.status === "identified" || plan.status === "fingerprinted" || plan.status === "unmatched" || plan.status === "failed", "Unknown result status.");
  requireValue(plan.partial === undefined || typeof plan.partial === "boolean", "Invalid partial-result flag.");
  const identified = plan.status === "identified" || plan.status === "fingerprinted";
  requireValue(identified ? object(plan.identity) && mbid(plan.identity.recording_mbid)
    && (plan.identity.release_mbid == null || mbid(plan.identity.release_mbid)) : plan.identity == null, "Identity contradicts result status or contains invalid IDs.");
  requireValue(plan.candidates === undefined || array(plan.candidates) && plan.candidates.length <= 500 && plan.candidates.every((candidate) => object(candidate) && mbid(candidate.id)), "Invalid recording candidates.");
  requireValue(array(plan.ops) && plan.ops.length <= TEXT_FIELDS.length + NUMBER_FIELDS.length && (identified || plan.ops.length === 0), "Invalid metadata operations for this status.");
  const fields = new Set<string>();
  for (const op of plan.ops) {
    requireValue(object(op) && op.kind === "tag" && op.track_id === plan.track_id && typeof op.field === "string" && !fields.has(op.field)
      && validField(op.field, op.old) && validField(op.field, op.new) && !same(op.old, op.new), "Invalid, unchanged or duplicate field proposal.");
    fields.add(op.field);
  }
}

function readPlans(run: unknown): Map<number, CleanupPlan> {
  const result = object(run) && run.schema === RESULT_SCHEMA ? run : object(run) ? run.result : undefined;
  requireValue(object(result) && result.schema === RESULT_SCHEMA && array(result.plans) && result.plans.length <= 500, "Supply a cleanup enrichment result or its job response, not an apply/rollback journal.");
  const plans = new Map<number, CleanupPlan>();
  for (const plan of result.plans) {
    assertCleanupPlan(plan, plans);
    plans.set(plan.track_id, plan);
  }
  return plans;
}

function scoreTracks(tracks: CleanupCohortTrack[], plans: Map<number, CleanupPlan>): Record<string, number | null> {
  const counts: Record<string, number> = { tracks: tracks.length, families: new Set(tracks.map((track) => track.family)).size,
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
    const identities: Array<["recording" | "release", string[] | undefined, string | null | undefined]> = [
      ["recording", recordings, plan.identity?.recording_mbid],
      ["release", releases, plan.identity?.release_mbid],
    ];
    for (const [kind, labels, prediction] of identities) {
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

export function scorePilot(cohort: CleanupCohort, run: unknown) {
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

function assessTrack(track: CleanupCohortTrack, plan: CleanupPlan | undefined, fields: string[]): TrackAssessment {
  const available = plan !== undefined && plan.status !== "failed";
  const identity = (kind: "recording" | "release"): IdentityAssessment => {
    const value = plan?.identity?.[`${kind}_mbid`]?.toLowerCase() ?? null;
    const labels = track[`expected_${kind}_mbids`];
    return { value, state: !available ? "unavailable" : value === null ? "abstained"
      : labels === null ? "unscored" : labels.some((id) => id.toLowerCase() === value.toLowerCase()) ? "correct" : "wrong" };
  };
  const candidates = new Set((plan?.candidates ?? []).map((candidate) => candidate.id.toLowerCase()));
  if (plan?.identity) candidates.add(plan.identity.recording_mbid.toLowerCase());
  return {
    availability: !plan ? "missing" : plan.status === "failed" ? "failed" : plan.partial ? "partial" : "complete",
    recording: identity("recording"), release: identity("release"),
    retrieval: !available ? "unavailable" : !track.expected_recording_mbids?.length ? "unscored"
      : track.expected_recording_mbids.some((id) => candidates.has(id.toLowerCase())) ? "retrieved" : "missed",
    fields: Object.fromEntries(fields.map((field) => {
      const op = plan?.ops.find((op) => op.field === field);
      const judgment = track.fields[field];
      const alreadyCorrect = judgment?.acceptable.includes(judgment.current);
      const correct = op && judgment?.acceptable.includes(op.new);
      const state = !available ? "unavailable" : !judgment ? "unscored"
        : !op ? alreadyCorrect ? "kept_correct" : "missed_correction"
          : alreadyCorrect ? correct ? "changed_correct" : "damaged_correct"
            : correct ? "useful_correction" : "wrong_correction";
      return [field, { state, proposal: op ? { old: op.old, new: op.new } : null }];
    })),
  };
}

function compareTrack(track: CleanupCohortTrack, before: TrackAssessment, after: TrackAssessment) {
  const available = (value: TrackAssessment): boolean => ["complete", "partial"].includes(value.availability);
  const improvements = new Set<string>(), regressions = new Set<string>();
  if (available(before) && !available(after)) regressions.add("lost_result");
  if (!available(before) && available(after)) improvements.add("recovered_result");
  if (before.availability === "complete" && after.availability === "partial") regressions.add("became_partial");
  if (before.availability === "partial" && after.availability === "complete") improvements.add("completed_partial_result");
  for (const kind of ["recording", "release"] as const) {
    const previous = before[kind].state, current = after[kind].state;
    if (current === "wrong" && previous !== "wrong") regressions.add(`new_wrong_${kind}`);
    if (previous === "correct" && current !== "correct") regressions.add(`lost_correct_${kind}`);
    if (current === "correct" && previous !== "correct") improvements.add(`recovered_correct_${kind}`);
    if (previous === "wrong" && current === "abstained") improvements.add(`avoided_wrong_${kind}`);
  }
  if (before.retrieval === "retrieved" && after.retrieval === "missed") regressions.add("lost_candidate");
  if (before.retrieval === "missed" && after.retrieval === "retrieved") improvements.add("recovered_candidate");
  for (const field of Object.keys(before.fields)) {
    const previous = before.fields[field].state, current = after.fields[field].state;
    if (current === "damaged_correct" && previous !== current) regressions.add("new_damaged_field");
    if (current === "wrong_correction" && previous !== current) regressions.add("new_wrong_field");
    if (current === "changed_correct" && previous === "kept_correct") regressions.add("new_unnecessary_field_change");
    if (previous === "useful_correction" && previous !== current) regressions.add("lost_useful_correction");
    if (current === "useful_correction" && previous !== current) improvements.add("recovered_useful_correction");
    if (previous === "damaged_correct" && current === "kept_correct") improvements.add("preserved_correct_field");
    if (previous === "damaged_correct" && current === "changed_correct") improvements.add("avoided_damaged_field");
    if (previous === "wrong_correction" && current === "missed_correction") improvements.add("avoided_wrong_field");
    if (previous === "changed_correct" && current === "kept_correct") improvements.add("avoided_unnecessary_field_change");
  }
  return { track_id: track.track_id, family: track.family, stratum: track.stratum,
    classification: improvements.size && regressions.size ? "mixed" : regressions.size ? "regression" : improvements.size ? "improvement" : "changed",
    improvements: [...improvements].sort(compareCodeUnits), regressions: [...regressions].sort(compareCodeUnits), baseline: before, changed: after };
}

export function comparePilot(cohort: CleanupCohort, baselineRun: unknown, changedRun: unknown, split: string = "development") {
  requireValue(split === "development" || split === "holdout", "Choose development or holdout explicitly; comparisons never combine the splits.");
  validateCohort(cohort, split);
  const baselinePlans = readPlans(baselineRun), changedPlans = readPlans(changedRun);
  const tracks = cohort.tracks.filter((track) => track.split === split).sort((a, b) => a.track_id - b.track_id);
  // Scoring both sides also validates the original metadata values before any
  // gains are reported. Unavailable results cannot count as safer abstentions.
  const baseline = scoreTracks(tracks, baselinePlans), changed = scoreTracks(tracks, changedPlans);
  const differences = [];
  for (const track of tracks) {
    const oldPlan = baselinePlans.get(track.track_id), newPlan = changedPlans.get(track.track_id);
    const fields = [...new Set([...Object.keys(track.fields), ...(oldPlan?.ops ?? []).map((op) => op.field), ...(newPlan?.ops ?? []).map((op) => op.field)])].sort();
    const before = assessTrack(track, oldPlan, fields), after = assessTrack(track, newPlan, fields);
    if (JSON.stringify(before) !== JSON.stringify(after)) differences.push(compareTrack(track, before, after));
  }
  const baselineValues: Record<string, number | null> = baseline;
  const changedValues: Record<string, number | null> = changed;
  return { schema_version: "library-cleanup-pilot-comparison/v1", split, manifest_fingerprint: cohort.manifest_fingerprint,
    selected_labels_fingerprint: hash(tracks), baseline_run_fingerprint: hash(baselineRun), changed_run_fingerprint: hash(changedRun),
    scope_note: "Selected split only. Track signals can overlap; mixed changes need review. Unknown labels do not establish improvement. Missing/failed results do not count as safer abstentions. Rate deltas are fractions, and stay null when either denominator is empty. No automatic pass threshold.",
    baseline, changed,
    delta: Object.fromEntries(Object.keys(baselineValues).map((key) => [key, baselineValues[key] === null || changedValues[key] === null ? null : changedValues[key] - baselineValues[key]])),
    unchanged_tracks: tracks.length - differences.length,
    tracks_with_regressions: differences.filter((track) => track.regressions.length).length,
    tracks_with_improvements: differences.filter((track) => track.improvements.length).length,
    differences,
  };
}

async function readJson<T>(path: string): Promise<T> {
  requireValue((await stat(path)).size <= 10 * 1024 * 1024, "Pilot input exceeds 10 MiB.");
  return JSON.parse(await readFile(path, "utf8")) as T;
}

export async function main(args: string[]): Promise<void> {
  if (args[0] === "init" && args.length === 3) {
    await writeFile(args[2], `${JSON.stringify(createCohort(await readJson<CleanupManifestTrack[]>(args[1])), null, 2)}\n`, { flag: "wx" });
  } else if (args[0] === "score" && args.length === 3) {
    const [cohort, run] = await Promise.all([readJson<CleanupCohort>(args[1]), readJson<CleanupRun | CleanupJobResponse>(args[2])]);
    process.stdout.write(`${JSON.stringify(scorePilot(cohort, run), null, 2)}\n`);
  } else if (args[0] === "compare" && [4, 5].includes(args.length)) {
    const [cohort, baseline, changed] = await Promise.all([readJson<CleanupCohort>(args[1]), readJson<CleanupRun | CleanupJobResponse>(args[2]), readJson<CleanupRun | CleanupJobResponse>(args[3])]);
    process.stdout.write(`${JSON.stringify(comparePilot(cohort, baseline, changed, args[4]), null, 2)}\n`);
  } else throw new Error("Usage: node tools/cleanup-pilot.mts init manifest.json cohort.json | score cohort.json run.json | compare cohort.json baseline.json changed.json [development|holdout]");
}

if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) {
  main(process.argv.slice(2)).catch((error: unknown) => { process.stderr.write(`${error instanceof Error ? error.message : "Cleanup pilot failed"}\n`); process.exitCode = 1; });
}
