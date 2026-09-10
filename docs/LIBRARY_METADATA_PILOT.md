# Offline library metadata pilot

Use this pilot to decide whether a retrieval change improves albums and game/film soundtracks.
The scorer reads JSON only. It does not scan audio, call a provider, apply tags, rename files,
or grant AI certification. Independent labels are still needed before any accuracy claim.

## Prepare a representative sample

Engineering prepares the manifest and result exports; the operator supplies judgments about
the intended recording, edition and metadata. The operator does not need to write scripts.
Start with a manageable sample from several composers/artists and release families, then grow
toward the research's 150–300-track pilot if its estimates remain uncertain. Include complete
albums, partial and multi-disc soundtracks, compilations, repeated titles, original/remastered
editions, multilingual names, unresolved recordings and already-correct tags. Do not select
only cases that the latest algorithm successfully identifies.

The manifest contains 2–500 distinct track IDs. Assign the same `family` to related artist/release
material, including alternate editions and overlapping compilations. Use consistent names;
the tool normalizes case and surrounding whitespace but cannot discover related artists or
shared recordings for you. A `stratum` describes the case type. For example:

```json
[
  {"track_id": 101, "family": "composer a and related releases", "stratum": "complete soundtrack"},
  {"track_id": 102, "family": "composer a and related releases", "stratum": "partial soundtrack"},
  {"track_id": 201, "family": "artist b and related releases", "stratum": "already correct album"}
]
```

Keep private manifests, labels and exports outside source control. From the repository root:

```powershell
node tools/cleanup-pilot.mjs init manifest.json cohort.json
```

Preparation refuses to overwrite an existing cohort. It assigns approximately one fifth of
families (at least one) to holdout, using a deterministic hash order. Track counts can be
uneven: a large album stays together. Preserve the resulting scope and splits; a changed
scope requires a new cohort and baseline. Develop against the development split and keep
holdout judgments/results out of tuning decisions.

## Supply independent judgments

For every cohort track, set `reviewed: true`, add a short `evidence_notes` value (up to 128
characters) identifying the basis of review, and fill only what can be established:

- `expected_recording_mbids`: acceptable recording IDs. `null` means unknown and is unscored;
  `[]` explicitly means no acceptable catalog identity, so any identification is wrong.
- `expected_release_mbids`: acceptable edition IDs, with the same unknown/no-match meanings.
  Preserve edition uncertainty with `null`; a matching album title alone is insufficient.
- `fields`: each evaluated field has its original `current` value and an `acceptable` array.
  For example, `"artist": {"current": "Various Artists", "acceptable": ["Composer A"]}`.
  List accepted spelling variants explicitly; field values are compared exactly.

Supported fields match current catalog proposals: `title`, `artist`, `album`, `album_artist`,
`genre`, `track_no`, `disc_no`, and `year`. Number fields accept nonnegative integers or `null`,
including zero-valued source tags that need correction.
An omitted field is unknown; it is not a vote to accept a suggestion. Include correct fields
with their existing value in `acceptable` to measure damaging or unnecessary changes.
Full dates and credits are still observations and are not scored as writable fields.

Use listening, original booklets, purchase metadata and independently verified disc/edition
information as appropriate. Copying the matcher's returned identity into the labels is not
independent validation. If the recording cannot be established, retain an explicit unknown.
Record longer research notes separately and reference them in the short evidence note.

## Compare retained catalog runs

Engineering saves the authenticated `GET /api/jobs/{job_id}` response for a completed
`library.cleanup-enrichment` job, or its `result` object, as `run.json`. Use the same indexed
metadata snapshot for baseline and changed runs; do not apply proposed changes between them.
The scorer checks original values for proposed, labeled fields and rejects mismatched
snapshots. It cannot detect a changed source field that produces no operation.
An apply/rollback journal or AI candidate-review result is a different schema and is rejected.
New catalog requests remain governed by the configured source policy; saving an existing
result and scoring it do not make requests. Inputs are limited to 10 MiB each.

```powershell
node tools/cleanup-pilot.mjs score cohort.json baseline-run.json > baseline-score.json
node tools/cleanup-pilot.mjs score cohort.json changed-run.json > changed-score.json
```

Scores include cohort and run fingerprints, overall counts, separate development/holdout
results, and case-type breakdowns. Extra result tracks are counted but excluded from scores.

| Measure | Interpretation |
|---|---|
| Recording/release precision | Correct proposals divided by proposals with known labels; unknown-label proposals are counted separately. |
| Retrieval coverage | Known recordings found among returned candidates or the selected identity, including direct identifier lookup. This is not text-search-only recall. |
| Recording coverage | Correct identifications divided by tracks with known acceptable recording IDs. |
| Field precision | Acceptable field proposals divided by proposals with known field judgments. |
| Correction coverage | Useful proposed fixes divided by labeled fields needing correction, including missed/failed tracks in the denominator. |
| Damaged correct fields | Proposals that would replace an acceptable current value with an unacceptable value; no changes are applied. |
| Changed already-correct fields | Any proposed change to an acceptable current value, including a different acceptable spelling. |
| Missing, failed, partial, abstained | Distinct states; a missing/failed result never receives credit for deliberate abstention. |

Every rate retains its numerator and denominator; an empty denominator produces `null`.
There is no automatic pass threshold or statistical independence claim. Review the actual
mistakes and family counts, not just aggregate percentages. A tiny holdout cannot establish
99% precision. Capture request counts, elapsed time, operator review time, costs and accepted
changes separately; current catalog exports do not provide all of those measurements.

Album-first retrieval is the next priority in the [backlog](../TODO.md). Additional paid
recognition, broader AI, OCR and duplicate deletion need their own concrete scope and
operator discussion. This scorer evaluates the existing catalog proposal format; those
experiments may need additional export adapters and labels before they can be compared.
