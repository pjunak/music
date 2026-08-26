# Playlist recommendation evaluation

The evaluation harness runs synthetic, versioned libraries through the same
`PlaylistSuggestionEngine` contract used by the Assistant API. It is intended
to catch ranking and contract regressions before heuristic changes or optional
model providers reach real libraries.

From `backend/`, run the checked-in local baseline:

```powershell
uv run music-cli evaluate-playlists app/assistant/evaluation_suites/playlist-local-v1.json
```

The local engine remains the default and makes no provider requests. To evaluate
the enabled, verified, and tested `playlist_planner` model role instead, use the
explicit disclosure flag:

```powershell
uv run music-cli evaluate-playlists app/assistant/evaluation_suites/playlist-local-v1.json `
  --engine configured-model `
  --send-suite-to-provider
```

Configured-model evaluation sends the suite's request text, synthetic titles,
artists, albums, origins, genres, tags, analysis labels, and numeric evidence to
the selected provider. It strips filesystem paths, locally enforces eligibility
and exclusions, and sends at most 100 candidates per case. Cases marked for
repeatability call the model twice; the bundled suite limits this to representative
manual-priority, sequence, and untrusted-input boundaries instead of repeating every
scenario. The command may therefore incur provider cost. The flag is deliberately
required each time; do not use it with a suite containing data you are unwilling to
disclose.

The AI Setup screen can run the checked-in suite as a durable server job after
the playlist model role is verified, tested, and enabled. Its progress and safe
report survive browser refreshes and reopening. The current pass/fail result is
bound to the exact connection, model, timeout, and response limit; changing or
reverifying that runtime invalidates the result. Evaluation jobs do not restart
automatically after a server restart because repeating model calls may duplicate
provider cost. A deliberate new run remains available from AI Setup.
Suite IDs are certification versions: changing cases or expectations requires a
new ID so every pass against the prior suite becomes stale and must be rerun.

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

The configured model is a hybrid ranker behind the same
`PlaylistSuggestionEngine` contract. It may return only ranked and selected
track IDs. Paths, source metadata, manual and generated tags, numeric evidence,
scores, and explanations in the evaluation response are reconstructed from the
trusted local candidate snapshot. Unknown, duplicate, over-limit, malformed, or
explicitly truncated model output fails the case instead of being repaired.

The local Assistant planner remains the default. After this suite passes through
AI Setup, the exact certified model configuration can also be selected for a
live-library suggestion. That separate durable job requires the current
versioned disclosure and explicit consent, sends only the same bounded path-free
candidate contract, and returns a draft that cannot write a playlist. Evaluation
success does not bypass the normal review and Authoring import preview.

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

## Music metadata tagging evaluation

`app/assistant/evaluation_suites/music-tagging-v1.json` applies the same versioned quality-gate
rule to the optional metadata tagger. Its 50 synthetic cases cover the full setting, scene,
mood, and period contract; sparse and ambiguous metadata; untrusted instructions; time-aware
local context; and classified voice evidence. The tagger must return every numeric track ID
exactly once, use only stable IDs from the controlled vocabulary, and avoid inventing semantic
context from sparse metadata or factual signal evidence. Safety cases can forbid whole
vocabulary groups, and the sparse case requires an empty tag set rather than checking only a
short list of likely false positives.

## Mood-tag cleanup evaluation

`app/assistant/evaluation_suites/tag-cleanup-v1.json` checks 15 conservative catalog-cleanup
scenarios. It covers deterministic spelling and plural fixes, clear semantic synonyms,
many-to-one normalization, ambiguous compound labels that must remain unchanged, prompt-shaped
input, and both all-null and mixed-decision batches at the live 20-source boundary. Every case
must pass; a model does not receive credit for aggressive normalization that discards useful
meaning.

## EQ draft evaluation

`app/assistant/evaluation_suites/eq-assistant-v1.json` checks the optional EQ role with ten
synthetic goals covering individual corrections, semantic uplift beyond local keyword rules,
neutral restraint, and combined warmth-plus-harshness intent. The provider must return exactly ten
bounded gains in the fixed band order; the harness also checks conservative request-specific
directions.
Passing the suite certifies only that exact role fingerprint. A live EQ request still needs the
current disclosure and returns an inert draft that the operator must preview and select through
Authoring import. No songs, audio, library metadata, paths, or existing presets are used by either
the suite or the live EQ request.

## Interpreting differences between models

These suites are role-specific certification gates, not a cross-role leaderboard. Raw case counts
cannot be compared between roles because the work is deliberately different:

- mood tagging is broad multi-label semantic coverage plus negative restraint across a large
  controlled vocabulary;
- cleanup is exhaustive one-to-one-or-null ontology mapping after deterministic fixes have already
  removed easy aliases, spellings, and plurals;
- playlist planning is semantic reranking and sequencing over a locally filtered candidate set;
- EQ starts from a deterministic baseline and accepts only a narrow, server-owned refinement
  envelope, so a smaller model can be entirely adequate when it follows the structured contract.

Use complete reruns and per-case failures to compare models for the same role. A green result means
the model cleared that role's minimum boundary; it does not prove that two green models are equally
good on a real library.
