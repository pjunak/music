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

After a completed lookup, **Download catalog results** saves the original
`library.cleanup-enrichment` job as JSON from the cleanup screen, including runs with
no proposals. The export retains scope, results and evidence before local suggestions,
edition choices or review selections are merged into the review. If the browser cannot
save the file, expand **Copy or view catalog results**, copy the JSON into a text file
and save it with a `.json` extension. The full text remains selectable if clipboard
permission is unavailable. Exporting does not run a provider or apply changes.
Only succeeded jobs with a valid catalog result are offered, including completed
runs whose provider evidence is partial. For older deployments, engineering can save the authenticated
`GET /api/jobs/{job_id}` response, or its `result` object, as `run.json`. Use the same indexed
metadata snapshot for baseline and changed runs; do not apply proposed changes between them.
The scorer checks original values for proposed, labeled fields and rejects mismatched
snapshots. It cannot detect a changed source field that produces no operation.
The cleanup summary also counts tracks with incomplete evidence and how many of
those are unmatched, including runs with no proposed edits. Keep these availability
gaps separate from complete no-match results when deciding what to improve next.
New lookup notes include safe HTTP/transport/response categories and Last.fm API
error numbers, including numeric codes carried inside HTTP error responses.
An HTTP 400 alone cannot distinguish a missing parameter, unknown resource or
credential problem; retain the API code before deciding on a remedy.
Older jobs cannot recover discarded error details; retain them as
baselines and inspect a new bounded lookup after deploying the diagnostics. A rate
limit, invalid credential and missing catalog record need different remedies; an
unavailable response alone does not justify changing acceptance thresholds.
An apply/rollback journal or AI candidate-review result is a different schema and is rejected.
New catalog requests remain governed by the configured source policy; saving an existing
result and scoring it do not make requests. Inputs are limited to 10 MiB each.

### When an HTTP error has no API code

A fresh Last.fm lookup can still retain only an HTTP status after the GET/error
handling fix. This means no usable numeric code was parsed under the response
bounds; it does not prove the body was empty, the key was invalid, or the track was
absent. Preserve the original job and confirm the running backend image revision.
A newly visible frontend control alone cannot establish that backend revision.

To distinguish basic endpoint access from a configured lookup, an operator can run
this read-only public example from the server host. Keep `YOUR_API_KEY` literally
as written; it deliberately avoids reading or transmitting a saved credential or
private library metadata. It requires curl and makes one request, with no retries:

```sh
curl --silent --show-error --max-time 20 --include --user-agent 'music-dnd-orchestrator/0.1 (https://github.com/pjunak/music)' 'https://ws.audioscrobbler.com/2.0/?method=track.gettoptags&api_key=YOUR_API_KEY&autocorrect=0&format=json&artist=radiohead&track=paranoid%20android'
```

The local check on 11 September 2026 returned HTTP 403 and JSON error 10 with both
PowerShell and reqwest 0.13.4 using the application client configuration. Last.fm
documents code 10 as an invalid API key ([method reference](https://www.last.fm/api/show/track.getTopTags)).
Compare the status, response type and numeric code; do not assume every environment
will return the same status. A different response is evidence to investigate the
network path, not proof of its cause. A matching response establishes access for
this public request only: the application's actual credential, recording lookup,
container network and deployed build still need separate verification. Do not
replace a scoped recording lookup with a title search solely to suppress an error.

If that control returns the normal placeholder-key error, the next useful comparison
uses the same key configured in Music: one known public song, followed by one of the
already reviewed recordings by MBID and by exact artist/title. Run this as an explicit
operator diagnostic; prompt for the key locally without echoing it, prevent redirects,
bound requests and bodies, and print only status, body category, numeric API code and
tag count. Never put the real key in a pasted command, process arguments or output.
A failed public control points toward the credential or provider before recording
coverage is assessed. Different ID/name outcomes warrant investigating that resource;
they do not authorize changing the application's identity scope. If the diagnostic
succeeds while the application fails, compare the actual backend image and container
network before changing matching rules.

The current operator decision is to keep recording-ID matching strict and prioritize
album metadata. The configured-key control succeeded; the tested ID query returned
HTTP 400/JSON without a usable numeric code, and its name query succeeded with zero
tags. This establishes different outcomes for that request pair, not a wrong
MusicBrainz ID or a failure of every Last.fm MBID lookup. No name fallback benefit
was demonstrated on that track. Continue evaluating available recording/edition and
field proposals while reporting optional tag failures as partial evidence. Do not
silently turn the HTTP error into a successful empty result or label it as a missing
recording without an identified provider error.

### Development baseline and title-style replay

The 11 September 2026 private baseline collection now covers all 114 development
tracks in four artist/release families; the fixed 75-track holdout remains unused.
The four original jobs are preserved separately with byte counts, hashes, scope
checks and metadata-snapshot checks. Their unmodified plans form a derived baseline
collection, not a single live run or a controlled deployment comparison. The first
job predates the Last.fm GET fix; later jobs retain the new request diagnostics.

The baseline contains 63 field proposals: 30 independently correct, three unwanted
changes to already-correct titles, and 30 unscored. Recording and release IDs remain
unlabeled, so the 65 proposed recording identities and one selected edition do not
establish identity precision. Evidence is partial for 111 tracks, including 46 of
the 49 unmatched tracks. Availability gaps must remain separate from matching failures.

An offline replay of only the title-style filter removes 12 cosmetic title
proposals: the three unwanted labeled edits and nine unscored edits. All 30 known
useful corrections remain, with no labeled field regression; 21 remaining field
proposals are unscored. The filter preserves authored capitalization, apostrophe
style and typographic hyphens while retaining the catalog spelling as evidence.
It does not change recording acceptance, retrieve new evidence or resolve editions.
The replay is a derived artifact, not a newly executed catalog job or holdout result.

The next live check requires the updated application and a small refreshed sample.
New edition notes distinguish an unchanged catalog position, a matched slot with
missing numbers, and an unusable assignment. Older exports cannot recover those
previously omitted observations. Remaining development gaps include the absent
confirmed reissue in the returned edition choices, Romanized composer credits being
replaced by native-script catalog names, and MusicBrainz HTTP 503/timeouts. Some
native-script names may identify the same composer; this still requires explicit
credit/alias evidence and does not establish a useful metadata replacement.

The application shares one MusicBrainz limiter between name and catalog lookups,
waiting 1.1 seconds after response headers. A 503 alone does not prove an application
pacing defect: MusicBrainz documents application, source-IP and global load limits
([rate-limiting reference](https://musicbrainz.org/doc/MusicBrainz_API/Rate_Limiting)).
Use bounded availability checks before changing retries or acceptance thresholds.

### Score saved results

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

## Find improvements and regressions between runs

Engineering can compare the same fixed cohort against two retained runs:

```powershell
node tools/cleanup-pilot.mjs compare cohort.json baseline-run.json changed-run.json > development-comparison.json
# Only after development decisions are fixed:
node tools/cleanup-pilot.mjs compare cohort.json baseline-run.json changed-run.json holdout > holdout-comparison.json
```

Comparison defaults to the development split. It requires independent judgments and evidence
notes only for the selected split, so pending holdout labels do not block development. The
holdout report requires an explicit selection; there is no combined comparison mode. Each
report includes only the selected tracks' counts and differences. The original `score` command
still requires all labels and returns both splits, so reserve it for the final complete report.

The comparison retains both sets of denominators, count/rate deltas, run fingerprints and a
fingerprint of the selected labels. Rate deltas are fractions, not percentage points; they stay
`null` if either rate has no denominator. Per-track differences show the original and proposed
field values, identity outcomes, retrieval coverage and availability. They flag recovered/lost
correct identities, useful corrections, damaged correct fields and unnecessary changes to
already-acceptable values. An acceptable alternative spelling remains an unnecessary change
when the original was already acceptable; it is not counted as damage.

One track can improve identity detection while acquiring a harmful field proposal. Such a
track is marked `mixed` and appears in both the improvement and regression counts. Inspect
these cases before the totals: precision can remain perfect while useful matches disappear.
Unknown labels produce unscored differences, and a missing/failed result never receives credit
for avoiding a wrong proposal. Returning a result after a failure is an availability gain,
not proof that its unknown identity is correct. Candidate recovery and incomplete results are
also reported separately.

Neither comparison nor scoring proves that all source metadata stayed identical: the
original-value check covers proposed fields with known judgments. Preserve the indexed
snapshot even when an untouched field produces no proposal. Family counts, evidence and
operator review still determine whether the observed changes justify a retrieval change;
the tool does not automatically accept a change or establish statistical significance.

The independently labeled album/soundtrack pilot is the next priority in the [backlog](../TODO.md). Additional paid
recognition, broader AI, OCR and duplicate deletion need their own concrete scope and
operator discussion. This scorer evaluates the existing catalog proposal format; those
experiments may need additional export adapters and labels before they can be compared.
