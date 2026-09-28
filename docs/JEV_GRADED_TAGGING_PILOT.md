# Graded musical fit: local experiment

Research and preparation: 28 September 2026. This is a listening experiment,
not a production tagging migration or a model-quality certificate.

## Decision

Use Jev's native **Score** primitive to rate each vocabulary tag independently.
The old pipeline asks yes/no questions, filters at 0.70 and keeps at most eight
candidates before additional evidence checks. A Noul is the probability of a
particular yes/no answer; it is not emotional intensity. That makes simply storing
its value as a tag percentage the wrong contract. Score supplies an ordered
descriptive scale, a fractional position, and a separate answer distribution.
[Noul](https://docs.typesafe.ai/primitives/noul),
[Score](https://docs.typesafe.ai/primitives/score).

The new adapter accepts Score alongside existing Noul/Choice. It validates exact
question and level membership, finite ranges and the pinned model, then discards
provider text extensions. Existing production conformance and binary tagging
behavior remain unchanged. The isolated experiment exercises Score with two
synthetic semantic probes before any private observations are sent.

## What the values mean

Five descriptions cover absent/conflicting, faint, recognizable, strong and
defining musical fit. Arithmetic stays in Rust: `relevance = score / 4`.
Keep `raw_score`, `score_confidence`, and all five `level_probabilities` separately.
A displayed 60% means a position on this rubric, **not** 60% certainty, 60% of the
recording, or a measured quantity of emotion. Jev explicitly documents weak
numerical calibration and sensitivity to wording. We must evaluate rankings and
usefulness with listening judgments before using these values in playlists.
[Jev limitations](https://docs.typesafe.ai/model-jaggedness/jev-1.13).

The initial display cutoff is 0.25, frozen before the unrated songs are evaluated.
It is provisional, not an accuracy threshold. Every raw score remains in the
local result, including scores below the cutoff. Reports can change the cutoff
without another provider call. Confidence describes concentration across levels;
it is neither correctness nor another multiplier on relevance. No confidence
threshold silently removes tentative suggestions.

Unavailable, failed or incomplete analysis/results are not zero relevance. The
preparation/importer refuses missing or partial whole-track evidence. Every
completed prediction remains `needs_listening_review`; this experiment does not
claim it can distinguish all musical absences from missing semantic information.

## Vocabulary and question boundaries

Evaluate the current shipped library, without renaming IDs or adding inferred
tags: 138 entries, with the group's membership authoritative even for the existing
`setting.medieval` ID in the period group.

| Group | Count | Specific judgment |
| --- | ---: | --- |
| Mood | 39 | The music's emotional character; quietness alone does not prove calm. |
| Scene | 42 | Suitability as accompaniment to a tabletop activity; the activity need not literally occur in the audio. |
| Setting | 49 | An evoked atmosphere suitable for a location; never actual recording geography. Generic compatibility is only a faint association. |
| Period | 8 | Evoked stylistic era; never release date or the technology used to record it. |

All period scores remain available for inspection. A report recommends at most
one period when its fit is at least 0.50 and its lead is at least 0.15; otherwise
it reports ambiguity. This provisional display rule is not a factual era claim.
`cross-era` requires recognizable blended influences, and `timeless` requires
era-neutral character; neither is a fallback for uncertainty.

Existing cross-group overlaps are useful: festive mood, dancing scene and tavern
setting describe different retrieval dimensions. Owner language maps imperfectly:
happy maps to joyful, celebration to festive and potentially festival, and
worshipful to sacred/worship. Safe space has no exact tag. Snowy mountains can
suggest a setting, but emotional `cold` is not a weather tag. Preserve those
distinctions and ask for owner judgments before expanding the library.

## Local data gathering

`tools/jev-corpus-prepare.mts` accepts an explicit JSON list of original paths.
It requires a new output directory, hashes originals before and after processing,
and never modifies media. FFmpeg decodes mono 16 kHz float PCM, with hashes binding
each recording to its analyzed samples. The current Rust analyzer produces
whole-track physical descriptors. The existing bounded Essentia/ONNX reference
produces full-track window predictions, including an explicit ending anchor.
The exporter retains time-weighted means, peaks, opening/ending responses, and
the share of time a label ranks in its classifier's top three.

One cached Discogs-EffNet encoder provides 400 style responses plus its embedding;
matching heads provide 40 instrument and 56 mood/theme responses. Retain all
496 dimensions locally. Pin graphs, taxonomy order and runtime; responses are
uncalibrated and correlated through their shared encoder. Their ranking can help
describe music but is not verified mood truth. Existing parity references qualify
the local computation; they do not establish owner-level tagging accuracy.
[Encoder](https://essentia.upf.edu/models/feature-extractors/discogs-effnet/discogs-effnet-bsdynamic-1.json),
[instrument head](https://essentia.upf.edu/models/classification-heads/mtg_jamendo_instrument/mtg_jamendo_instrument-discogs-effnet-1.json),
[mood/theme head](https://essentia.upf.edu/models/classification-heads/mtg_jamendo_moodtheme/mtg_jamendo_moodtheme-discogs-effnet-1.json).

Only small reconstructed observation cards go to Jev. No audio, artist, album,
title, filename, collection, local path, hash, lyrics or owner ratings are sent.
Tag names and definitions belong to the vocabulary, not to the song's evidence.
No catalog lookup or generative caption is introduced in this comparison.

Useful next candidates depend on the observed errors:

- Dance/pulse mistakes: evaluate the compatible EffNet danceability head against
  owner-rated dancing examples. Its small in-house training dataset and reported
  cross-validation score are not evidence of accuracy on this music. Reusing an
  embedding requires validating the exact graph/preprocessing pairing.
  [Danceability model card](https://essentia.upf.edu/models/classification-heads/danceability/danceability-discogs-effnet-1.json).
- Persistent mood confusion: add a separately qualified valence/arousal predictor
  and check whether it changes useful rankings. Valence/arousal cannot identify
  a specific fantasy place or social activity by itself.
- Missing semantic associations: compare music/text embeddings through multiple
  contrasting paraphrases. The cached DCLAP challenger previously had unanimous
  agreement in only 30/72 prompt contrasts; similarity is not probability. A
  larger music-specific CLAP is a candidate, not an automatic improvement.
  [DCLAP](https://github.com/NeptuneHub/AudioMuse-AI-DCLAP),
  [LAION music CLAP](https://huggingface.co/laion/larger_clap_music).
- Vocal errors: enable the existing separately qualified voice detector when
  it matters to an actual tag/use case. Do not add transcription, separation or
  lyrics retrieval merely to increase feature count.

Model licensing/redistribution remains a separate production gate. Reusing cached
research assets here does not add them to the application image or repository.

## Frozen comparison

The six previously rated development recordings and seven new unrated recordings
use identical questions across four recipes:

| Recipe | Input change being tested |
| --- | --- |
| `labels3` | Three highest mean responses per head: nine learned labels total. |
| `labels8` | Eight per head: 24 learned labels; only label count changes. |
| `labels8_physical` | Adds normalized brightness, density, rhythmic-drive and flux trajectories, their reliability and acoustic structure. |
| `labels8_temporal` | Adds peaks, opening/ending responses and top-three ranking persistence; no physical measurements. |

Means/ranking use local arithmetic. Raw values are rounded only in provider cards;
original precision stays in the private artifact. Physical indices carry their
descriptive meaning and limitations. Unverified tempo and unclassified voice are
omitted. Head type/order and all tag questions are stable across recipes. This
factor design can attribute a difference to input amount, physical data, or
temporal data; it does not claim the entire new system is a single-variable
comparison with the previous binary engine.

Seven group-homogeneous partitions of at most 25 questions cover all 138 tags.
Four recipes on 13 songs plus one smallest-recipe repeat on the first and last
recording and two semantic probes require **380 requests**, bounded at
**10,633,174 conservative input units**. Plan SHA-256:
`84d7c06b99fed0e7347d827ab39f0efc2fd3ab8db924f394f41f25d02d47b4ce`.
The separately approved run must use these exact bounds and a new output directory.
No retries, adaptive follow-ups, production database access or tag writes occur.
The native durable ledger checkpoints every attempt before provider cost.
At preparation, this run is **awaiting separate paid approval**.

The request body cap, exact plan reconstruction, source-content verification and
fixed HTTPS target are checked before accessing the temporary key. Failed jobs
retain their ledger/journal and cannot resume automatically. Missing usage remains
unknown rather than zero. Synthetic probes test basic Score operation, not
listening quality or the application's existing quality certificate.

## Reproduce and review

Build the existing `jev-compare` example. The preparation helper's `--analyzer`
argument points to that executable; its other arguments explicitly supply FFmpeg,
FFprobe and the already qualified Essentia/ONNX/model directories. Do not run
provider calls as part of normal tests.

`graded-plan SOURCES.json PLAN.json` reads entries with `corpus`, `predictions`,
`tracks`, and `cohort` (`development` or `unrated`). The frozen plan contains
private local identities for review, but each request contains only observation
cards. `graded-run` requires that plan plus its exact hash and budget flags.
`tools/jev-graded-report.mts` checks the matching plan/result/journal and renders
the owner report; `--cutoff` changes presentation without changing raw scores.

Owner feedback remains outside all inference inputs. Preserve the six supplied
judgments as qualitative ordering/association constraints, never invented numeric
targets. Unmentioned tags are unknown, not negatives. The new seven-song review
should identify useful suggestions, clearly wrong suggestions and missing tags,
using faint/recognizable/strong/defining descriptions if helpful. Review each
recipe before choosing a default; more returned tags alone is not improvement.

Report actual tokens per song/recipe, displayed useful tags, wrong suggestions,
score changes and repeat variation separately. These 13 songs can guide the next
iteration but cannot establish general accuracy, calibrated thresholds or playlist
quality. Production numeric persistence, review UI and playlist weighting follow
only after this experiment demonstrates useful graded judgments.
