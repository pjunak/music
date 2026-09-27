# ADR-026: Native Jev applicability and evidence judgments

Reviewed 27 September 2026 against the owner's Jev quality export and the current native engine.

## What the run establishes

The connection and native conformance passed for pinned jev-1.13.0. The quality job
received 52 responses, reporting 691,154 input and 65,116 output tokens. Twelve
primary cases completed: eleven produced no tags and one produced only march.
A typed response was rejected during forest-hunt. The remaining 50 primary cases
and all 13 safety repeats were not executed after that failure. The resulting
0/63 is a failed certification, not 63 independent completed classification failures.

The export contains final proposals but no rejected Noul scores or raw invalid
response. It cannot identify which early gate discarded a candidate or which
response invariant failed. Do not infer that Jev cannot classify the vocabulary,
normalize an unknown malformed response, or lower the quality gate from this run.

## Design decisions

1. Keep a dedicated native engine behind the existing tagger interface. Share
   consent, budgets, durable attempts, vocabulary validation and human review.
   Jev receives text facts and never receives audio or generates explanations.
2. Put each complete tag meaning, its group meaning and its literal question
   together. Remove references such as tag_group_definitions[group]. Describe
   the musical character or tabletop suitability directly instead of asking
   whether evidence is sufficient to justify proposing a hypothetical tag.
3. Use one independent applicability Noul per non-period tag. Several tags may
   qualify, or none. Keep one full-vocabulary period Choice with an explicit
   no-supported-period option, followed by an absolute applicability Noul for
   its winner. Never rank Choice and Noul scores together.
4. Ground candidates with independent support and contradiction Nouls for each
   actual observation. Inline the observation's content and meaning. Supporting
   sources must not compete for a single probability mass: three valid sources
   are three possible citations. Remove the redundant abstract sufficiency
   question; concrete grounding supplies the independent check instead.
5. If an observation receives both support and contradiction judgments, treat
   it as conflicting rather than use it as positive evidence. Require another
   unambiguous supporting observation or abstain. Such semantic uncertainty is
   not a malformed provider response and must not abort later recordings.
6. Render numerical observations descriptively in code, retaining original
   values, coverage, reliability and the ending. Coarse low/medium/high bands
   describe a normalized physical proxy only; they never assign moods. Missing
   values remain missing. Artist names, release dates and community tags retain
   their evidence limitations.
7. Batch independent questions for the same recording across candidates within
   existing byte and question limits, retaining identical individual questions. Preserve every vocabulary entry, the eight-candidate bound,
   conservative worst-case reservations and the no-automatic-retry rule. Defer
   concurrency, caching services and trained calibration until measured need.
8. Report match versus grounding abstention, bounded top match scores, exact
   response-validation failures and cases not run after an abort. Use existing
   review/export fields rather than introduce another storage or diagnostics UI.

Applicability and concrete support use separate, provisional 0.70 Noul gates;
period selection retains its separate 0.70 Choice gate. These are uncalibrated
starting operating points, not interchangeable probabilities or a claim of mood
accuracy. The unchanged 90% quality gate and safety repeats remain mandatory.

## Validation and adoption

Implemented as `music-jev-decisions/v3`, replacing v2 without a compatibility path.
Local validation passes all 567 Rust and 374 frontend tests, production frontend
build, workspace/fuzz Clippy, formatting, architecture and generated-contract checks.
Native regressions cover full/custom/200-tag vocabularies, independent citations,
conflicts, categorical abstention, retained endings/missingness, strict typed parsing,
durable failure recovery and reservation bounds. A separate regression confirms
candidate batching preserves all questions while reducing requests and input bytes.
Provider errors stay distinct from not-run cases. These are implementation tests,
not a live provider quality score or independent listening result.

After deployment, run the complete unchanged synthetic suite with the exact pinned
model. Inspect stage diagnostics before adjusting wording; change thresholds only
with a separate judged development cohort, not to fit these acceptance cases.
Then compare the same original recordings using independent listening judgments.
Synthetic metadata cases cannot establish real music accuracy. Jev cannot recover
emotional meaning absent from its text evidence; only an observed semantic gap would
justify additional audio models. Existing audio context and accepted tags stay usable.

## Primary references

- [Noul](https://docs.typesafe.ai/primitives/noul): independent binary propositions and code-owned thresholds.
- [Choice](https://docs.typesafe.ai/primitives/choice): categorical alternatives, full distributions and explicit none options.
- [Jev 1.13 limitations](https://docs.typesafe.ai/model-jaggedness/jev-1.13): literal questions, fewer references, numerical work in code and relevant context.
- [State](https://docs.typesafe.ai/concepts/state): shared facts and independent questions; text-only input.
- [Parallel questions](https://docs.typesafe.ai/patterns/fan-out): combine independent questions and let code route their results.
- [Batching comparison](https://docs.typesafe.ai/cookbooks/parallel_questions): shared state is charged once per request; the published example is not a music benchmark.

The architecture above is this project's inference from those contracts, not a
TypeSafe benchmark of music tagging or a promise that the next live run will pass.
