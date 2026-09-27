# ADR-026: Native Jev applicability and evidence judgments

Reviewed 27 September 2026 against the owner's latest Jev quality export and the official API/SDK contracts.

## What the latest run establishes

Pinned jev-1.13.0 passed native conformance. Engine v3 received 90 responses,
reporting 1,036,505 input and 57,544 output tokens. Nineteen primary cases completed;
case 20, infernal-dark-ritual, failed with
`model_execution_typed_choice_distribution_invalid`. The remaining 43 primary cases
and all 13 safety repeats were not run. The resulting 3/63 certification is not 63
independent completed classification failures. None of the completed cases returned
a forbidden tag, but many missed required tags, so fixing transport alone cannot
establish adequate classification quality.

That error uniquely identifies our Choice unit-sum check (absolute tolerance
0.0001). The official SDK says the values sum to approximately one and publishes
no rounding bound. The export does not retain the raw distribution, so its exact
sum and the size of its deviation are unknown. Treat the stricter application
invariant as an integration defect; do not claim the model returned a specific sum.

There were separate semantic inconsistencies: the applicability question asked
whether the music matched a tag while its negative criterion rejected “only a name,”
despite explicitly allowing descriptive album phrases elsewhere. Some fixtures
also required an emotional mood without supplying evidence for it. A castle
procession is not by itself courageous music; Arctic geography is not by itself
an emotionally detached musical impression. The effect of revised questions on
live model scores remains unmeasured.

## Response compatibility follow-up

A local audit against the current SDK contract found two further parser mismatches:
nullable/omitted token counts and ignored extension fields. Both were reproduced as
`typed_response_shape_invalid` before correction. They are separate from the unit-sum
abort seen in the owner's export; no new live Jev failure is claimed.

The adapter now accepts optional counts inside the required usage object and preserves
unknowns. The existing ledger retains reservations, records which counts were reported,
and marks incomplete totals in the UI. Additional envelope, usage and answer fields are
discarded; only validated typed decisions are returned. Known required fields, exact
question/option membership, the pinned model, score validity and semantic gates remain
mandatory. This follows the current response contract, without a legacy fallback.

## Design decisions

1. Keep a dedicated native engine behind the existing tagger interface. Share
   consent, budgets, durable attempts, vocabulary validation and human review.
   Jev receives text facts and never receives audio or generates explanations.
2. Put each complete tag and group meaning beside a literal evidence question.
   Mood concerns a musical impression; setting/scene concern reasons for tabletop
   use. Descriptive metadata can support a tentative tag without proving how the
   recording sounds. Isolated artist/company names and embedded commands cannot.
3. Use independent applicability Nouls for non-period tags. Several tags may qualify,
   or none. Keep one full-vocabulary period Choice with an explicit none option,
   followed by an absolute applicability Noul for its winner. Never rank Choice
   against Noul scores. Validate finite 0..1 scores, exact options and a positive
   maximal winner, but do not enforce an undocumented sum tolerance or normalize
   a score across an acceptance threshold. The independent Noul and grounding
   remain necessary even when a Choice passes parsing.
4. Ground candidates with independent support and contradiction Nouls per actual
   observation. Inline its content and meaning. Several sources can support a tag;
   they do not compete for probability. No abstract sufficiency question remains.
5. If an observation is judged both supporting and contradictory, retain it as a
   conflict. Require another unambiguous supporting observation or abstain.
   Semantic uncertainty must not become a malformed-response error.
6. Render numerical observations descriptively in code, retaining original values,
   reliability, missingness and the ending. Low/medium/high bands describe physical
   proxies only; they never assign moods. Artist names, release dates and community
   tags retain their evidence limitations.
7. Batch independent questions for a recording within the existing context limits.
   Preserve every vocabulary entry, eight-candidate bounds, worst-case reservations
   and the no-automatic-retry rule. No SDK, service, cache or compatibility branch
   is introduced.
8. Keep one shared quality benchmark for providers. Suite v27 gives seven ambiguous
   positive cases explicit musical/theme evidence without removing required tags.
   Add paired safety cases rejecting geography-to-emotion and procession-to-heroism
   guesses. All 65 cases, 15 safety repeats, full/custom/200-tag vocabularies and
   the separate nine-case context-only gate remain required. Native transport
   tests cover approximate scores separately from semantic quality.

Applicability and grounding retain separate provisional 0.70 Noul gates; period
selection keeps its 0.70 Choice gate. Conformance's 0.90/0.10/0.90 thresholds,
90% quality requirements and blocking safety rules are unchanged. These scores
are uncalibrated operating points, not measured music-tagging accuracy.

## Validation and adoption

Engine `music-jev-decisions/v4` replaces v3 without a compatibility path. Local
regressions cover rounded Choice totals on both sides of one, ties, malformed
scores/options/winners, threshold preservation, HTTP handling and durable multi-song
execution. Additional regressions cover optional usage, discarded extensions, malformed
known fields and durable accounting with unreported counts. Existing full-vocabulary,
evidence, abstention, numerical-context and
budget regressions remain. These are implementation checks, not a paid provider
quality result. The [implementation log](SONG_EVIDENCE_IMPLEMENTATION_PLAN.md)
records completed validation for this batch.

After deployment, refresh AI setup, rerun **Test and make available**, then run the
complete new quality suite with pinned jev-1.13.0. An old or partial report cannot
certify this engine/suite. If completed cases still miss supported tags, inspect
those outcomes before further prompt changes. Adjust thresholds only with a separate
judged development cohort, not to fit this acceptance suite. Independent listening
on the same original recordings remains necessary; synthetic metadata cannot
establish musical accuracy. Current local audio context and accepted tags stay usable.

## Primary references

- [SDK answer contract](https://docs.typesafe.ai/sdk/python/api/types/responses): approximate Choice probabilities and per-field semantics.
- [Noul](https://docs.typesafe.ai/primitives/noul): independent binary propositions and code-owned thresholds.
- [Choice](https://docs.typesafe.ai/primitives/choice): categorical alternatives and explicit none options.
- [Jev 1.13 limitations](https://docs.typesafe.ai/model-jaggedness/jev-1.13): literal questions, consistent criteria, numerical work in code and relevant context.
- [State](https://docs.typesafe.ai/concepts/state): shared facts and independent questions; text-only input.
- [Parallel questions](https://docs.typesafe.ai/patterns/fan-out): independent questions with code-owned routing.
- [Batching comparison](https://docs.typesafe.ai/cookbooks/parallel_questions): shared state is charged once per request; this is not a music benchmark.

This architecture is the project's inference from those contracts, not a TypeSafe
benchmark of music tagging or a promise that the next live run will pass.
