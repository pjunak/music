# Short-question Jev comparison

This developer-only experiment compares a completed graded pilot's `labels8`
results with short Noul questions. It tests the owner's proposal that direct
questions may give more useful rankings at lower input cost. It does not change
production tagging or certify a model.

## Fixed comparison

Use the seven previously unrated recordings from the graded pilot, all 138 shipped
tags, pinned `jev-1.13.0`, and the same seven request partitions per recording.
The provider state is copied exactly, including the 24 audio-classifier labels,
their response values and their shared interpretation. No audio, song identity,
filenames, paths or owner ratings enter the requests.

The old Score question becomes a Noul with two instruction fields:

```json
{
  "type": "noul",
  "instructions": {
    "question": "Is calm a fitting description of this music?",
    "definition": "Peaceful, settled, gentle, or emotionally untroubled tone."
  }
}
```

There are no answer criteria or repeated question-level evidence rules. Retain the
vocabulary definition to keep ambiguous tag meanings fixed. Scene questions ask
about fitting accompaniment; setting questions ask about evoked settings; period
questions ask about musical style or period. These are separate questions for
each tag, not mutually exclusive selections.

Noul values describe the model's probability of answering yes. The previous Score
values describe expected positions among five musical-fit levels divided by four.
Neither is a calibrated measure of musical intensity. Their numbers and thresholds
are not interchangeable. Compare ranked tags and owner usefulness rather than
interpreting numeric increases as quality gains.
[Noul](https://docs.typesafe.ai/primitives/noul),
[Score](https://docs.typesafe.ai/primitives/score).

The historical baseline is reused without new paid calls. Wording, guidance and
question type change together; this cannot isolate the effect of fewer words.
There is no contemporaneous baseline rerun or repeat-variation control. The seven
recordings remain an owner-review sample, not an independently labelled accuracy
benchmark. Every score is retained; threshold counts are descriptive only.

## Execution and validation

The native `jev-compare simple-plan BASELINE OUTPUT --tracks ...` command first
reconstructs and validates the original graded plan, then derives the selected
Noul requests. `simple-run BASELINE PLAN` requires the exact plan hash, request
and input caps, a temporary key file, and a new journal path. It verifies original
file hashes before credential access. The existing bounded HTTPS transport and
durable journal are reused; failures stop the run without retries or resume.

The owner approved this one-time seven-recording test in advance. Its frozen plan
contains **49 requests**, **966 tag answers** and **326,641 conservative input
units**. Plan SHA-256:
`4cb170cea6168d0e0e3027a8bd0e9cdf92083d930132a813754a96e967081ca4`.
At $0.042 per million input tokens, the conservative input allowance corresponds
to about $0.014; actual reported token usage is recorded separately after execution.
[Pricing](https://docs.typesafe.ai/models).

`tools/jev-simple-report.mts` validates the old plan/result/journal and the new
journal, their pairing, question membership and preserved evidence. It writes
separate complete Score and Noul tag reports plus a comparison. Reports include
all four vocabulary groups, actual usage, top-five overlap and descriptive score
distributions. Unknown usage is not zero; incomplete results cannot become a
complete comparison.

All private plans, journals and reports belong in ignored local output. Run the
native example tests, the new report tests and existing graded report tests, tool
typecheck/lint and relevant repository gates before accepting tooling changes.
The [graded experiment](JEV_GRADED_TAGGING_PILOT.md) owns the original audio
extraction and scoring details.

## Observed run, 29 September 2026

All 49 requests completed without retries and returned all 966 tag values on the
pinned model. The provider reported 97,629 input and 18,585 free output tokens,
with no missing usage. The matched historical baseline consumed 319,522 input
and 15,687 output tokens for these same 49 partitions. The simplified questions
used **69.4% fewer input tokens**.

Estimated input charges at the published rate are $0.004100418 for this run and
$0.013420 for the historical baseline. These are provider-token calculations,
not account invoices. Per-song means are 13,947 versus 45,646 input tokens, or
approximately $0.586 versus $1.917 per 1,000 songs, excluding local extraction.
The one-time approval is consumed. No additional paid variant or retry follows
from this run.

Top-five membership overlaps are 23/35 for moods, 28/35 for scenes, 21/35 for
settings and 33/35 for periods, using deterministic tag-ID ordering for ties.
These describe ranking similarity, not accuracy. Broad mood patterns persist,
while some mixed or orchestral recordings change their leading moods materially.
Scene specificity remains an issue: exploration leads on six of seven recordings,
and 10–21 scene tags per song receive Noul values of at least 0.5. No threshold
was selected from these results. Full reports retain all 138 values per song.

Concise questions are a viable lower-cost candidate for owner review; the results
do not establish superior listening quality or repeat stability. No production
tagging configuration, generated profile, or library tag was changed.
