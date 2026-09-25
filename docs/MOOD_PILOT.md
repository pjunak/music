# Offline mood listening pilot

This private pilot measures whether suggestions help the owner find music for
ordinary listening and tabletop sessions. Software conformance is a separate gate.
No commands below call a provider, change a library, or accept generated tags.

## Prepare and freeze the sample

Choose roughly 60-100 varied recordings, including known failures and a random
library sample. Include sparse/rich metadata, quiet/loud masters, vocals, and
tracks with changing endings. Two independent groups are the minimum the tool
can score, not a useful accuracy sample. Keep audio and judgments outside Git.

Before running the tagger, select the recordings in **Mood Library** using its
existing track checkboxes. Open **Listening comparison** in the selection panel
and choose **Export listening sample**. In **Vocabulary**, choose **Export saved
vocabulary**; this downloads the saved revision, excluding unsaved editor changes.
Keep that vocabulary fixed for all candidates.

The inventory contains only explicit library IDs, with no predictions or inferred
file identities. It accepts 2-1,000 unique positive IDs from the same library that
will produce the later run exports:

```json
{"schema_version":"song-mood-inventory/v1","track_ids":[7,14,21]}
```

```powershell
node tools/mood-pilot.mjs init inventory.json vocabulary.json draft.jsonl
```

Initialization rejects retained run exports. Deriving the sample from returned
answers would exclude failed or missing tracks before freezing and bias the
comparison. Export selected IDs first, including tracks that have never been tagged.

The first JSONL line is the `song-mood-judgments/v2` manifest; subsequent lines are
judgments. There is one current format. Superseded manifests are rejected; prepare
and freeze a new pilot instead of adapting old results. Edit:

- Manifest: set `annotator`, choose a small `core_tag_ids` set, describe actual
  `session_requests` and sampling in `selection_notes`. Keep the initial seed fixed.
- Each recording: set a stable `file_reference` (prefer a content hash, otherwise
  an unchanged private file reference) and `recording_group`. Related editions,
  duplicates and excerpts must share that group; use `duplicate_group` to connect
  independently identified copies. The tool cannot discover unidentified duplicates.
- Set `duration_seconds` from the complete original recording before freezing. It
  must be a finite positive number, at most 86,400 seconds; fractional seconds are
  supported. Initialization leaves it unknown rather than estimating it from results
  or the amount you happened to listen to.
- Record stable album and composer identities where known. `separate_by` can contain
  `album` and/or `composer` to keep those together too. Groups combine transitively;
  if everything becomes one group, add independent recordings. Do not split related
  recordings just to obtain a larger apparent sample.

Freeze the membership before tuning or inspecting confirmation predictions. Leave
unlistened rows at `reviewed: false`, `blind: null` with empty intervals and labels;
freezing does not require listening or invent whether it was blind:

```powershell
node tools/mood-pilot.mjs freeze draft.jsonl vocabulary.json pilot.jsonl
```

The seed assigns about 30% of independent groups to confirmation and the rest to
development. Track counts may differ. File references, durations, grouping, core
tags, vocabulary and partitions are fingerprinted. Scoring rejects accidental
changes; this is an audit mechanism, not protection against someone deliberately rewriting the manifest. Commands refuse
to overwrite an existing file. Preserve the original frozen copy privately.

## Listen independently

In each selected row, record ordered, non-overlapping `listened_intervals` in seconds,
within the frozen duration. For a 150-second recording, `[[0, 150]]` or adjacent
intervals `[[0, 60], [60, 150]]` cover the complete track. Set `scope` to `whole_track`
only when those intervals cover the beginning, every intervening section and the
exact recorded ending without gaps. Use `excerpt` for partial listening; it cannot
certify a whole recording. Both scoring modes reject inconsistent whole-track claims.

Set the initially unknown `blind` field explicitly: `true` only for independent
listening before seeing candidate predictions, or `false` if predictions were visible
or influenced the judgment. Re-listening after seeing predictions does not restore
blinding. Mark `reviewed: true` only after listening. These fields record listener
declarations; the tool cannot verify that listening occurred or that a duration is correct.

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

## Check readiness before model calls

After freezing, check the selected split without supplying predictions:

```powershell
node tools/mood-pilot.mjs status pilot.jsonl vocabulary.json
```

The read-only `song-mood-readiness/v1` report lists blocking library track IDs,
ready/reviewed track counts, independent groups, frozen duration, declared listening
time and unheard time. It uses the same listening gates as scoring and comparison.
Whole-track declarations with gaps remain blocked even in diagnostic mode. The
report includes all four core-label states; omitted labels stay unjudged. Counts
include labels entered on unfinished rows, so read them alongside the blockers.

Only development readiness is reported by default. Add `--confirmation` when ready
to open that cohort, or `--diagnostic` to check assisted/excerpt declarations. The
flags may be combined. This command changes no judgments, opens no audio, calls no
provider and needs no run export. An incomplete valid cohort returns a report with
`ready_for_scoring: false` and exit code zero; invalid/frozen-contract changes fail
with a nonzero exit code. Scripts must inspect the readiness field.

Readiness means the listening declarations satisfy the selected mode. It is not a
quality pass: a ready cohort with few or no judged core tags can still produce
unknown or uninformative scores. Do not change assisted listening to blind merely
to clear a blocker; record it honestly and use diagnostics. Completing the pilot,
comparing real candidate results and checking production remain separate steps.

## Score development; open confirmation once

After freezing the sample and preparing independent judgments, run each candidate
through the normal consent/budget flow. Use **Export retained run results** from
its finished job or asynchronous batch. Failed, expired or cancelled runs can also
export their retained answers, including an empty result set. Do not drop those
runs or their selected tracks from the comparison. A missing row is unavailable;
a retained row with `tags: []` is an explicit abstention.

```powershell
node tools/mood-pilot.mjs score pilot.jsonl candidate-run.json vocabulary.json > development-score.json
node tools/mood-pilot.mjs score pilot.jsonl candidate-run.json vocabulary.json --confirmation > confirmation-score.json
node --test tools/mood-pilot.test.mjs
```

Normal scoring and comparison require blind, whole-track judgments for **every**
recording in the selected split. A single assisted or excerpt judgment prevents an
independent report; the tool never drops inconvenient tracks to improve the score.
Reports use `song-mood-score/v2` or `song-mood-comparison/v2` and record
`assessment_mode: "independent"`. This describes the declared listening conditions,
not certification, representative sampling or proof of accuracy.

Normal scoring does not read confirmation judgments into its metrics or require
them to be completed. Use the explicit confirmation switch only after freezing
candidate settings. Repeated tuning against it makes it development data.

Reports contain per-tag counts and separate mood, session-use, period and custom
results. Precision uses judged positive/negative proposals only. Recall measures
recovered known positive judgments; `missed_positive_tags` includes known positives
lost to empty or unavailable results. It cannot estimate undiscovered positives
among unjudged labels. Judgment coverage, uncertain/unjudged proposals and proposals outside the chosen core are reported
separately, so a high score with little judging is visible. Missing responses are
not abstentions; an explicitly empty result is. Precision without judged proposals
is unknown. Group bootstrap intervals resample whole independent groups 1,000 times;
intervals with fewer than 900 defined replicates remain unknown. Reports include
the number of independent groups, attempted replicates and defined replicates for
precision, recall and useful-track coverage. Small samples and rare tags
cannot establish general accuracy. Album/composer overlap and listening conditions
are reported, including when those identities were not used for splitting.

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

## Compare candidates on the same judgments

Save both current-contract retained run exports before another run replaces results.
Use the frozen pilot and vocabulary for both sides:

```powershell
node tools/mood-pilot.mjs compare pilot.jsonl baseline-run.json candidate-run.json vocabulary.json > development-comparison.json
node tools/mood-pilot.mjs compare pilot.jsonl baseline-run.json candidate-run.json vocabulary.json --confirmation > confirmation-comparison.json
```

The comparison contains both score reports, per-category and per-tag deltas, result
availability, and changed recordings with added/removed tags and their judgments.
All deltas mean **candidate minus baseline**; rate deltas are fractions, so `0.05`
means five percentage points. Missing results remain in the selected cohort and
cannot count as avoided false positives. Explicit abstention can avoid a false
positive while losing useful tags; the report marks such a change as mixed.
Unknown and non-core labels do not establish a gain. A reduction in proposals alone
does not establish a better system; read recall, coverage and availability together.

Category delta intervals use a **paired recording-group bootstrap**: each replicate
draws the same whole groups on both sides. They are not differences between two
independent confidence intervals. An undefined denominator on either side leaves
that delta unknown; two unmeasured precisions do not become a zero difference.
Per-tag deltas are point estimates, with counts for judging sparse evidence.
There is no automatic winner, pass threshold, or correction for trying many candidates.

For a source comparison, keep the interpreter, vocabulary and recordings fixed and
change only the supplied evidence: catalog-only, audio-only, then combined where
the candidate setup supports it. Record the exact candidate settings and normal
consent/budget separately. This offline tool does not create those runs, call Jev,
or prove that their files/settings match; full export fingerprints identify the
snapshots and source signatures remain available for audit. Entire-run usage is
reported without attributing it to the selected split. Keep any original pre-cutover
baseline as a static report; do not adapt its retired analysis into current inputs.

Choose settings on development results before opening confirmation. Use the same
listening/session requests to record auditioning time, corrections and disruptive
false positives alongside the numbers. Repeated confirmation-guided changes require
new independent confirmation recordings.

## Assisted or excerpt diagnostics

Assisted corrections and partial listening can help investigate failures. To inspect
those judgments, use the explicit diagnostic mode with the same frozen cohort:

```powershell
node tools/mood-pilot.mjs score pilot.jsonl candidate-run.json vocabulary.json --diagnostic > diagnostic-score.json
node tools/mood-pilot.mjs compare pilot.jsonl baseline-run.json candidate-run.json vocabulary.json --diagnostic > diagnostic-comparison.json
```

Every report, including both sides of a comparison, records
`assessment_mode: "diagnostic"`. These numbers cannot establish independent
whole-recording accuracy or gains, even if precision is high. Read the assisted and
excerpt counts alongside them. All selected tracks, including missing results, stay
in the denominator; the tool does not substitute a smaller blind subset. Diagnostic
mode still requires completed listening declarations and consistent intervals/scope.
It never changes a judgment, reassigns a split or implies that excerpt judgments
apply to unheard sections. Confirmation remains unopened unless `--confirmation`
is explicitly added; both flags can be supplied in either order.

## Native model gate

Discogs-EffNet with matching mood/theme and instrument heads remains an optional
experiment. Adoption needs exact preprocessing/output parity, bounded CPU/RAM and
cancellation, and a useful improvement on development recordings. MTG model weights
have their own license and must be installed explicitly; they are not bundled with
the application. See the [implementation plan](SONG_EVIDENCE_IMPLEMENTATION_PLAN.md)
for the current probe evidence and remaining gates. Jev is an optional comparison
using the same evidence; paid/live comparisons require normal operator consent.
