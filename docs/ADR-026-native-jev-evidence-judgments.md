# ADR-026: Native Jev applicability and evidence judgments

Reviewed 28 September 2026 against the owner's latest Jev quality export and the official API/SDK contracts.

## Initial v3 failure

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

### Completed-run follow-up: engine v5

The owner's 27 September v4/v27 export completes all 65 scenarios and 15 repeats:
55/65 pass, including 14/15 safety, 4/5 custom and 6/9 context-only cases. All 418
requests received responses, reporting 5,081,125 input tokens. This is a completed
semantic failure, not another transport abort. It is not a listening-accuracy result.

The heroic negative case exposes conflicting criteria: the bundled vocabulary sends
"royal procession" as a heroic context cue, then the test forbids that association.
Heroic scores 0.76 in both attempts. The origin-only castle expectation is also
ambiguous: Origin is a game/film/album provenance field. The urgent acoustic case
describes a fast pulse even though normalization withholds coarse tempo. Rejected
tag scores are absent when other tags were accepted, limiting further diagnosis.

V5 supplies definitions and synonyms without display names or retrieval context
cues. Mood, use and period have aligned, distinct positive criteria. Musical
questions omit identities, absolute recording level and other non-supporting
technical facts; custom groups retain the facts their definitions may concern.
Coverage qualifies the input and measurement reliability stays on acoustic cards.
Questions sharing a view are batched. The maximum frame in each group bounds
grounding budgets; the eight-candidate limit and all 0.70 gates remain unchanged.

Quality-only diagnostics retain bounded provider states and every tag's fit,
period probability, shortlist outcome, grounding scores and decision stage, even
for partially successful or interrupted cases. Required tags are added by the
evaluator after execution, never sent to Jev. Safety-repeat traces remain separate.
Scores describe model judgments; multiple supporting observations are not presented
as independent verification. The existing console and JSON export own this output.

Suite v28 preserves every prior required tag and all acceptance thresholds. It
supplies the castle use in the positive case's album and adds a provenance-only
negative case: 66 scenarios, 16 safety repeats and nine context-only cases. The
urgent fixture's description now matches the actual supplied proxies. Its numerical
inputs and expected tag remain unchanged. These handcrafted acoustic positives
still require independent listening qualification; no measurements or owner labels
were invented to make them pass.

V5 introduced no additional model, database, legacy reader or automatic retry.
Existing listening-pilot comparisons remain the route to measure real usefulness.
The following paid investigation uses v5/v28 as its baseline; prior v4 outcomes
do not certify either later engine.

### Controlled framing repair: engine v6

The owner's completed v5/v28 export passed 15/66 scenarios and returned only
6/70 required mood assignments. All 269 requests completed, and all 16 safety
checks passed. Of 100 missing required assignments, 94 failed initial matching,
three failed period applicability and three failed grounding.

A separately authorized 30-request experiment used six fixed synthetic cases,
two selected initial Noul questions each and five framing variants. The model,
definitions, underlying evidence and 0.70 gate stayed fixed. Scores below are
single observations, not calibrated probabilities of a recording's mood:

| Question/case | V5 baseline | Names only | Literal question | Neutral cards | Literal + neutral |
|---|---:|---:|---:|---:|---:|
| Festive / jubilant holiday folk | .57 | .59 | .87 | .84 | .96 |
| Heroic / courageous, valorous resolve | .65 | .63 | .94 | .87 | .97 |
| Castle / castle procession | .67 | .69 | .77 | .76 | .80 |
| Heroic / same non-heroic procession | .45 | .49 | .38 | .59 | .39 |
| Quiet focus / lamplit study | .70 | .69 | .94 | .62 | .92 |
| Custom dark / reassuring low light | .79 | .80 | .96 | .86 | .96 |
| Calm / settled acoustic proxies | .65 | .68 | .64 | .65 | .68 |

The result supports a framing defect: the old question mixed whether supplied
content expressed a definition with whether it independently established a
recording's character. “Unverified” metadata-card wording contributed to that
conservatism. Restoring display names alone did not resolve it. This small
comparison did not test full-vocabulary interactions or grounding and does not
show that every remaining miss has the same cause.

V6 asks directly whether supplied descriptions or measurements express the
definition, states that this is semantic matching rather than independent
verification, and labels album/genre cards as supplied descriptions. It adopts
the exact measured combined variant, including the criteria. Group scopes,
definition/synonym meanings, excluded provenance, numerical facts, period Choice,
candidate limits, all gates and observation grounding remain. Suggestions still
carry tentative support and attributable evidence.

Fresh conformance and the full unchanged v28 suite then completed: **44/66**
scenarios, **16/16** safety, **60/70** required mood assignments, **4/5** custom
and **6/9** context-only scenarios. Thirty scenarios improved; the redefined-label
case regressed by adding an unsupported second custom tag. The model remains
uncertified. See the [acceptance record](AI_ACCEPTANCE.md#jev-controlled-comparison-and-full-validation-2026-09-27)
for complete usage, remaining fixture contradictions and semantic failures.

The developer-only `jev-compare` example creates offline, hash-bound plans and
uses synthetic fixtures without application configuration or library access.
Paid runs are explicit, bounded, checkpointed before I/O and never retried.
Full evaluation reuses the native executor, durable SQLite jobs and shared scorer;
it never publishes application acceptance. Variants identical to the current engine
are omitted instead of spending requests on adopted changes.
The v5 experiment's exact requests remain in its original local journal; there
is no legacy production engine or replay of old generated analysis.

### Descriptive support repair: engine v7

A subsequent v6 run reached 45/66, retaining the same fixture, observation-support,
custom-purpose and acoustic-proxy failures. Suite v29 corrects only the four
positive settings previously supplied through excluded provenance. Required labels,
safety controls and quality thresholds are unchanged.

An authorized thirty-call comparison over fifteen fixed cases isolated question
wording while preserving state, definitions and conflicts. Direct semantic support
recovered explicit calm, ritual, majestic and defiant descriptions, and rejected
quiet focus when only warm low-light ambience was described. It did not improve
general first-pass or numeric-acoustic matching. V7 therefore adopts exactly the
measured question only for album/genre support; all other question types retain
v6 behavior. Inference identity changes and invalidates old proposals/certification.
The [dated results](AI_ACCEPTANCE.md#jev-metadata-support-repair-2026-09-28) preserve
the measured scores, rejected broader variant, usage and acceptance limitations.

### Shared inference rules

1. Keep a dedicated native engine behind the existing tagger interface. Share
   consent, budgets, durable attempts, vocabulary validation and human review.
   Jev receives text facts and never receives audio or generates explanations.
2. Put each tag definition/synonyms and group meaning beside a literal evidence question.
   Mood concerns a musical impression; setting/scene concern reasons for tabletop
   use. Descriptive metadata can support a tentative tag without proving how the
   recording sounds. Isolated artist/company/source names and embedded commands cannot.
3. Use independent applicability Nouls for non-period tags. Several tags may qualify,
   or none. Keep one full-vocabulary period Choice with an explicit none option,
   followed by an absolute applicability Noul for its winner. Never rank Choice
   against Noul scores. Validate finite 0..1 scores, exact options and a positive
   maximal winner, but do not enforce an undocumented sum tolerance or normalize
   a score across an acceptance threshold. The independent Noul and grounding
   remain necessary even when a Choice passes parsing.
4. Ground candidates with independent support and contradiction Nouls per actual
   eligible observation. Inline its content and meaning. Several sources can support a tag;
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
8. Keep one shared quality benchmark for providers. Suite v27 gave seven ambiguous
   positive cases explicit musical/theme evidence without removing required tags.
   Add paired safety cases rejecting geography-to-emotion and procession-to-heroism
   guesses. V28 retains 66 cases, 16 safety repeats, full/custom/200-tag vocabularies and
   the separate nine-case context-only gate remain required. Native transport
   tests cover approximate scores separately from semantic quality.

Applicability and grounding retain separate provisional 0.70 Noul gates; period
selection keeps its 0.70 Choice gate. Conformance's 0.90/0.10/0.90 thresholds,
90% quality requirements and blocking safety rules are unchanged. These scores
are uncalibrated operating points, not measured music-tagging accuracy.

## Validation and adoption

Engine `music-jev-decisions/v6` replaces v5 without a compatibility path. Local
regressions cover rounded Choice totals on both sides of one, ties, malformed
scores/options/winners, threshold preservation, HTTP handling and durable multi-song
execution. Additional regressions cover optional usage, discarded extensions, malformed
known fields and durable accounting with unreported counts. Existing full-vocabulary,
evidence, abstention, numerical-context and
budget regressions remain. The developer example also checks reviewed-plan identity,
cost bounds, durable checkpoints and non-replay. The [implementation log](SONG_EVIDENCE_IMPLEMENTATION_PLAN.md)
records local validation; the separately identified paid result above still fails
the unchanged quality gate.

After a subsequent measured repair and deployment, refresh AI setup, rerun
**Test and make available**, then run the complete current quality suite with
pinned jev-1.13.0. V6/v28 is known to fail; repeating it unchanged is not a repair.
An old or partial report cannot certify a new engine/suite. Inspect the recorded
outcomes before further prompt changes. Adjust thresholds only with a separate
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
