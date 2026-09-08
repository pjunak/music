// Offline listening-pilot preparation and scoring. No provider or library writes.
import { readFile, stat, writeFile } from "node:fs/promises";
import { createHash } from "node:crypto";
import { pathToFileURL } from "node:url";

const hash = (value) => createHash("sha256").update(JSON.stringify(value)).digest("hex");
const validId = (id) => Number.isSafeInteger(id) && id > 0;
const strings = (items) => Array.isArray(items) && items.length <= 200 && items.every((item) => typeof item === "string" && item.length > 0 && item.length <= 128) && new Set(items).size === items.length;
function requireValue(condition, message) { if (!condition) throw new Error(message); }

export function createCohort(ids) {
  requireValue(Array.isArray(ids) && ids.length === 30 && ids.every(validId) && new Set(ids).size === 30, "Supply exactly 30 distinct positive track IDs from a varied listening sample.");
  const ordered = [...ids].sort((a, b) => hash(a).localeCompare(hash(b)));
  return { schema_version: "assistant-mood-pilot/v1", tracks: ordered.map((track_id, index) => ({ track_id, split: index < 20 ? "development" : "holdout", reviewed: false, expected_tags: [], notes: "" })) };
}

export function scorePilot(cohort, run, vocabulary) {
  requireValue(cohort?.schema_version === "assistant-mood-pilot/v1" && Array.isArray(cohort.tracks) && cohort.tracks.length === 30, "Expected a 30-track mood pilot.");
  const ids = new Set();
  for (const track of cohort.tracks) {
    requireValue(validId(track.track_id) && !ids.has(track.track_id), "Pilot contains invalid or duplicate IDs.");
    ids.add(track.track_id);
    requireValue(track.reviewed === true && strings(track.expected_tags), "Finish independent listening judgments for every track before scoring.");
    requireValue(["development", "holdout"].includes(track.split), "Unknown pilot split.");
  }
  requireValue(cohort.tracks.filter((track) => track.split === "holdout").length === 10, "Preserve the 20 development / 10 holdout split.");
  requireValue(Array.isArray(vocabulary?.groups), "Supply the vocabulary used for the run.");
  const groupOf = new Map();
  for (const group of vocabulary.groups) {
    requireValue(typeof group.key === "string" && Array.isArray(group.tags), "Invalid vocabulary group.");
    const category = group.key === "scene" || group.key === "setting" ? "session_use" : group.key === "mood" || group.key === "period" ? group.key : "custom";
    for (const tag of group.tags) {
      requireValue(typeof tag.name === "string" && !groupOf.has(tag.name), "Vocabulary names must be unique.");
      groupOf.set(tag.name, category);
    }
  }
  requireValue(run?.schema_version === "assistant-mood-run-export/v1" && Array.isArray(run.track_results) && run.track_results.length <= 5000, "Expected a retained mood-run export.");
  const predictions = new Map();
  for (const track of run.track_results) {
    requireValue(validId(track.track_id) && !predictions.has(track.track_id) && strings(track.tags) && track.tags.length <= 8, "Invalid or duplicate run result.");
    requireValue(track.tags.every((tag) => groupOf.has(tag)), "Run contains a tag absent from this vocabulary.");
    predictions.set(track.track_id, track.tags);
  }
  requireValue(cohort.tracks.every((track) => track.expected_tags.every((tag) => groupOf.has(tag))), "Listening judgments contain unknown vocabulary tags.");
  const score = (tracks, group) => {
    let proposed = 0, useful = 0, eligible = 0, covered = 0, missing = 0, abstained = 0;
    for (const track of tracks) {
      const included = (tag) => group === "all" || groupOf.get(tag) === group;
      const expected = new Set(track.expected_tags.filter(included));
      const returned = predictions.get(track.track_id);
      if (!returned) missing++;
      else if (returned.length === 0) abstained++;
      const tags = (returned ?? []).filter(included);
      const accepted = tags.filter((tag) => expected.has(tag)).length;
      proposed += tags.length; useful += accepted;
      if (expected.size > 0) { eligible++; if (accepted > 0) covered++; }
    }
    const precision = proposed ? useful / proposed : null;
    const coverage = eligible ? covered / eligible : null;
    return { tracks: tracks.length, missing_results: missing, empty_results: abstained, proposed_tags: proposed, useful_tags: useful, false_positive_tags: proposed - useful, eligible_tracks: eligible, covered_tracks: covered, precision, useful_track_coverage: coverage,
      meets_proposed_targets: precision === null || coverage === null ? null : missing === 0 && precision >= 0.8 && coverage >= 0.6 };
  };
  return { schema_version: "assistant-mood-pilot-score/v1", cohort_fingerprint: hash(cohort.tracks), run_id: run.run_id ?? null,
    scope_note: "Scores use the fixed listening cohort. Usage covers the entire exported run, including any extra tracks. A small pilot does not establish general accuracy.",
    splits: Object.fromEntries(["development", "holdout"].map((split) => [split, Object.fromEntries(["all", "mood", "session_use", "period", "custom"].map((group) => [group, score(cohort.tracks.filter((track) => track.split === split), group)]))])),
    usage: run.usage ?? null };
}

async function readJson(path) {
  requireValue((await stat(path)).size <= 10 * 1024 * 1024, "Pilot input exceeds 10 MiB.");
  return JSON.parse(await readFile(path, "utf8"));
}

export async function main(args) {
  if (args[0] === "init" && args.length === 3) {
    await writeFile(args[2], `${JSON.stringify(createCohort(await readJson(args[1])), null, 2)}\n`, { flag: "wx" });
  } else if (args[0] === "score" && args.length === 4) {
    const [cohort, run, vocabulary] = await Promise.all(args.slice(1).map(readJson));
    process.stdout.write(`${JSON.stringify(scorePilot(cohort, run, vocabulary), null, 2)}\n`);
  } else throw new Error("Usage: node tools/mood-pilot.mjs init ids.json cohort.json | score cohort.json run.json vocabulary.json");
}

if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) {
  main(process.argv.slice(2)).catch((error) => { process.stderr.write(`${error.message}\n`); process.exitCode = 1; });
}
