# Playlist recommendation evaluation

The evaluation harness runs synthetic, versioned libraries through the same
`PlaylistSuggestionEngine` contract used by the Assistant API. It is intended
to catch ranking and contract regressions before heuristic changes or optional
model providers reach real libraries.

From `backend/`, run the checked-in local baseline:

```powershell
uv run music-cli evaluate-playlists evaluation/playlist-local-v1.json
```

The local engine remains the default and makes no provider requests. To evaluate
the enabled, verified, and tested `playlist_planner` model role instead, use the
explicit disclosure flag:

```powershell
uv run music-cli evaluate-playlists evaluation/playlist-local-v1.json `
  --engine configured-model `
  --send-suite-to-provider
```

Configured-model evaluation sends the suite's request text, synthetic titles,
artists, albums, origins, genres, tags, analysis labels, and numeric evidence to
the selected provider. It strips filesystem paths, locally enforces eligibility
and exclusions, and sends at most 100 candidates per case. A deterministic case
calls the model twice. The command may therefore incur provider cost. The flag
is deliberately required each time; do not use it with a suite containing data
you are unwilling to disclose.

Use `--json` for the complete `playlist-evaluation-result/v1` result. The JSON
includes per-case metrics, selected and top-ranked track IDs, failures, and a
stable response fingerprint suitable for CI artifacts or before/after diffs.

## What a suite measures

Each `playlist-evaluation/v1` case supplies a synthetic track library, manual
tags, generated metadata, optional measured signals, a normal playlist request,
and explicit expectations. The evaluator reports:

- precision and recall among the first `k` suggestions;
- reciprocal rank of the first relevant song;
- recall of songs that should be initially selected;
- requested ordering-pair accuracy for energy flows;
- explanation coverage;
- forbidden, excluded, or invented track IDs;
- response-contract integrity and optional repeat-run determinism.

Thresholds belong to each case. A suite fails when a threshold is missed or an
engine violates hard safety invariants: candidates must come from the supplied
library, preserve its source metadata, remain unique, respect exclusions and
limits, and agree with the reported selection plan. An engine error is contained
to its case and reported by exception type without copying provider details into
the result.

## Adding cases

Prefer representative decisions over large artificial libraries. Add a case
when an operator can state which tracks are acceptable and why, especially for:

- D&D settings and scenes such as tavern, medieval, dungeon, dancing, travel,
  stealth, combat, and rest;
- manual-tag priority over generated resemblance;
- measurable constraints such as tempo and duration;
- steady, rising, falling, and build-and-resolve ordering;
- sparse or missing metadata and deliberately unsuitable tracks.

Do not copy private library paths, credentials, or media into the suite. Use
synthetic names and evidence. Keep relevant sets broad enough that the harness
measures playlist quality instead of freezing one accidental exact ranking.

The configured model is a hybrid ranker behind the same
`PlaylistSuggestionEngine` contract. It may return only ranked and selected
track IDs. Paths, source metadata, manual and generated tags, numeric evidence,
scores, and explanations in the evaluation response are reconstructed from the
trusted local candidate snapshot. Unknown, duplicate, over-limit, malformed, or
explicitly truncated model output fails the case instead of being repaired.

This is an evaluation-only provider path. The live Assistant playlist API still
uses `local-planner/v2` and cannot send a real library to a provider.
