# Offline mood listening pilot

This private pilot measures whether suggestions help the owner find music for
ordinary listening and tabletop sessions. Software conformance is a separate gate.
No commands below call a provider, change a library, or accept generated tags.

## Prepare and freeze the sample

Choose roughly 60-100 varied recordings, including known failures and a random
library sample. Include sparse/rich metadata, quiet/loud masters, vocals, and
tracks with changing endings. Two independent groups are the minimum the tool
can score, not a useful accuracy sample. Keep audio and judgments outside Git.

Export a selected run using **Export retained run results**, and export the exact
Mood vocabulary. The run is only an inventory for initialization: the template
contains no predictions or explanations. Its IDs must refer to the same library.

```powershell
node tools/mood-pilot.mjs init run.json vocabulary.json draft.jsonl
```

The first JSONL line is the `song-mood-judgments/v1` manifest; subsequent lines are
judgments. There is one current format, with no old 30-track cohort parser. Edit:

- Manifest: set `annotator`, choose a small `core_tag_ids` set, describe actual
  `session_requests` and sampling in `selection_notes`. Keep the initial seed fixed.
- Each recording: set a stable `file_reference` (prefer a content hash, otherwise
  an unchanged private file reference) and `recording_group`. Related editions,
  duplicates and excerpts must share that group; use `duplicate_group` to connect
  independently identified copies. The tool cannot discover unidentified duplicates.
- Record stable album and composer identities where known. `separate_by` can contain
  `album` and/or `composer` to keep those together too. Groups combine transitively;
  if everything becomes one group, add independent recordings. Do not split related
  recordings just to obtain a larger apparent sample.

Freeze the membership before tuning or inspecting confirmation predictions:

```powershell
node tools/mood-pilot.mjs freeze draft.jsonl vocabulary.json pilot.jsonl
```

The seed assigns about 30% of independent groups to confirmation and the rest to
development. Track counts may differ. Grouping, core tags, vocabulary and partitions
are fingerprinted. Scoring rejects accidental changes; this is an audit mechanism,
not protection against someone deliberately rewriting the manifest. Commands refuse
to overwrite an existing file. Preserve the original frozen copy privately.

## Listen independently

In each selected row, record ordered `listened_intervals` as seconds, for example
`[[0, 150]]`. Set `scope` to `whole_track` or `excerpt`; excerpt judgments cannot
certify a whole recording. Set the initially unknown `blind` field explicitly: `true` only for independent
listening, or `false` if predictions were visible or influenced the judgment. Mark `reviewed: true` only after listening.

Use vocabulary IDs in `labels`, for example:

```json
{"mood.calm":"positive","mood.tense":"negative","scene.rest":"uncertain"}
```

Use the IDs from your actual exported vocabulary. Each label is `positive`,
`negative`, `uncertain` or `unjudged`; omission also means unjudged. Judge every core
tag where possible. Unselected tags are never silently treated as negatives.
Evaluate perceived mood separately from scene/setting suitability, and use
`session_notes` to record useful candidates, auditioning time and review effort.
Record ambiguity in `notes`; do not overwrite disagreement to match the model.

## Score development; open confirmation once

```powershell
node tools/mood-pilot.mjs score pilot.jsonl candidate-run.json vocabulary.json > development-score.json
node tools/mood-pilot.mjs score pilot.jsonl candidate-run.json vocabulary.json --confirmation > confirmation-score.json
node --test tools/mood-pilot.test.mjs
```

Normal scoring does not read confirmation judgments into its metrics or require
them to be completed. Use the explicit confirmation switch only after freezing
candidate settings. Repeated tuning against it makes it development data.

Reports contain per-tag counts and separate mood, session-use, period and custom
results. Precision uses judged positive/negative proposals only. Judgment coverage,
uncertain/unjudged proposals and proposals outside the chosen core are reported
separately, so a high score with little judging is visible. Missing responses are
not abstentions; an explicitly empty result is. Precision without judged proposals
is unknown. Group bootstrap intervals resample whole independent groups 1,000 times;
intervals with too few defined replicates remain unknown. Small samples and rare tags
cannot establish general accuracy. Album/composer overlap and assisted/excerpt
judgments are reported, including when those identities were not used for splitting.

The report retains candidate source signatures for audit. These signatures include
analysis/provider configuration and can legitimately differ between candidates;
they are not audio hashes. **Verify that the run used the frozen file references**
before comparing. The tool cannot verify remote file contents from a run export.
Usage totals cover the whole exported run, including extra tracks.

Keep an initial baseline as a static private report; the runtime does not need its
old reader. Compare one change at a time with fixed vocabulary and listening groups.
An initial 80% precision / 60% useful-track coverage can guide investigation, but
selection time, disruptive misses and actual review effort decide adoption. There
is deliberately no automatic pass badge for incomplete listening data.

## Native model gate

Discogs-EffNet with matching mood/theme and instrument heads remains an optional
experiment. Adoption needs exact preprocessing/output parity, bounded CPU/RAM and
cancellation, and a useful improvement on development recordings. MTG model weights
have their own license and must be installed explicitly; they are not bundled with
the application. See the [implementation plan](SONG_EVIDENCE_IMPLEMENTATION_PLAN.md)
for the current probe evidence and remaining gates. Jev is an optional comparison
using the same evidence; paid/live comparisons require normal operator consent.
