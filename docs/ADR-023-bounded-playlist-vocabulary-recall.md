# ADR-023: Bounded vocabulary recall for playlist candidates

**Status:** Accepted

**Date:** 2026-09-05

## Problem and measurement

The Rust model task used only the first `min(3 * candidate_limit, 100)` local
candidates. The alias/context-cue union described in ADR-003 was absent. A model
cannot select a relevant track that is missing from its input, however capable its
reasoning or however good the final ranking evaluator.

Provider-free measurement found full candidate recall in the original 11 synthetic
scenarios. Three added 101-track scenarios isolate a declared multiword alias, a
declared context cue, and a custom vocabulary. Relevant and distractor tracks use
the same artist so diversity does not accidentally rescue an otherwise missed
track. Each new scenario had recall 0 before the change and 1 afterward, with
unchanged pool sizes of 15, 100, and 15. Original-case recall remains 1.

These controlled development fixtures demonstrate the omission and repair. They
are not independent held-out measurements or evidence of model ranking quality.

## Decision

Keep the original local planner and its candidate order, scores, default selection,
sequence and eligibility rules. Supplement its bounded model pool through a
separate, local vocabulary matcher:

1. Match complete ordered phrases from canonical names, exact aliases, or context
   cues against the request. Normalize Unicode and word boundaries, so `inn` does
   not match `inner`, and one word does not stand in for a multiword cue. Do not
   interpret description prose or infer undeclared synonyms.
2. Retrieve tracks with matching operator-owned manual tags (canonical names or
   exact aliases). Do not turn generated profiles, titles, artist names or paths
   into new authored tags or new recall evidence. Existing local ranking remains
   authoritative for the original pool.
3. Run retrieved tracks through the same local planner, including exclusions,
   numeric and unknown-BPM rules, current analysis, scoring and source projection.
   Borrow the matching evidence instead of cloning library profiles.
4. Reserve at most one quarter of the expanded pool, capped at 20, for additional
   matches. Replace only trailing non-default candidates. Every original default
   selection remains; if defaults fill the pool, add nothing. The total remains at
   most `min(3 * candidate_limit, 100)`. Recall candidates start unselected.

This bounded allocation is a conservative policy, not a measured optimum. A future
labelled study should assess recall gains, displaced-candidate relevance, latency,
and provider ranking before adjusting it. If the vocabulary contains no matching
mapping, retrieval does not invent one. Richer semantic retrieval remains separate.

The live feature reads the current operator vocabulary. Fixed evaluations use the
bundled vocabulary unless a case supplies its own validated synthetic document.
No real operator vocabulary or library is loaded by the evaluation CLI.

Input contract v3 retains original `local_rank` values and uses `null` for additions
outside that original pool. The fixed prompt explains this distinction. All
existing defaults and their duration plan remain intact. The model still returns
only bounded ranked/selected IDs, and the server reconstructs public fields from
the local snapshot. The private request changes no public HTTP/WebSocket DTO,
database schema, browser flow, output protocol, or provider disclosure fields.

## Evaluation and verification

`music-cli evaluate-playlists SUITE --engine candidates [--json]` reports pool size,
labelled relevant count, relevant IDs present, recall and missing IDs. It does not
load application configuration, connect to a database, call a provider, or certify
a model. Successful exit means the diagnostic ran; inspect its counts for misses.
Its document is `playlist-candidate-evaluation/v1` and has no quality `passed` flag.

Playlist quality suite v6 retains all 11 existing scenarios and thresholds and adds
the three large-pool scenarios, including custom vocabulary. All 14 cases must pass
the existing per-case ranking, selection, duration and safety rules to certify the
chosen model. Synthetic expected outputs test only harness consistency. Existing
configured-model evaluation still requires `--send-suite-to-provider`.

Regressions cover before/after omissions, original recall, repeated preparation,
custom vocabulary, original ranks/defaults, path-free requests, source-preserving
reconstruction, full and saturated pools, alias boundaries, unaccepted/partial
labels, excluded IDs, BPM restrictions, and the provider's closed ID schema.
The new policy module and suite are included in playlist runtime fingerprints;
affected saved certifications require explicit full re-evaluation.
