# Assistant architecture and contract map

**Status:** Living documentation
**Last audited:** 2026-09-06

This is the current map for the local-first Assistant and its optional model workflows. Use it to
find ownership, privacy boundaries, contract versions, evaluation gates, and regression tests.
The practical deployment and acceptance sequence remains in [the operator guide](../ASSISTANT.md);
the reasons behind durable choices remain in the linked ADRs.

## Reading and harness development

Use the platform/workflow tables to locate the affected owner, then read the
matching task-rule subsection. These rules apply across application, storage,
server, transport, and frontend changes; their ownership is not confined to a
single source directory.

Providers, models, and reasoning settings are replaceable in the server UI.
There is no settled production-model choice. Dated evaluations record a tested
configuration, not a permanent provider decision. Use the coding model selected
for the task, including Astra, to build and tighten the custom harness. Keep the
harness provider-neutral: express task intent, typed inputs/outputs, disclosures,
failure policy, and quality evidence explicitly. Provider-specific behavior
belongs in versioned adapters with declared capabilities. Changing a UI-selected
configuration still requires its applicable verification and certification;
an old result must not certify a replacement model or authorize private data use.

## Source-of-truth order

When two descriptions disagree, resolve the disagreement in the same change:

1. strict Rust task types, provider definitions, and registered job/API code define current runtime
   behavior;
2. tests and checked-in synthetic suites define executable acceptance boundaries;
3. this map records current ownership and version inventory;
4. ADRs record why a decision was made and may contain historical version strings;
5. the operator guide describes deployment and human acceptance.

The documentation tests intentionally bind this file to the runtime contract constants. They do
not prove model quality or provider compatibility; the task-specific quality and conformance gates
remain authoritative for those claims.

## Non-negotiable boundaries

- Local analysis, filtering, identity, and safety limits remain authoritative. A provider may refine
  a bounded draft but cannot invent an executable action or a second state machine.
- Provider input is task-specific, minimized, size-bounded, and disclosed before live-library use.
  Metadata and paths inside the JSON user document are untrusted data, never instructions.
- Provider output is untrusted. It must parse as one object, pass the strict task model, satisfy
  request-specific identity/bounds checks, and be reconstructed from local source data where
  applicable.
- Model results remain inert until explicit operator review. They never write playlists, presets,
  files, embedded metadata, or database mood tags directly.
- Connection verification, role conformance, quality certification, and live-run consent are
  separate gates. Passing one does not imply another.
- Runtime fingerprints include the shared harness and role contract plus a SHA-256 digest of the
  role's executable prompt/schema modules, orchestration, and checked-in evaluation suites. Relevant source or
  suite changes make saved conformance and quality results stale even when a developer forgets to
  advance the human-readable contract fragment.
  Shared policy remains conservative; unknown roles and artifacts use shared coverage.
  Locked dependency changes also expire all roles, including schema/parser updates.
  Runtime inventory tests reject an Assistant source or suite omitted from digest coverage.
- Provider jobs are non-restartable after uncertain external cost. An uncertain attempt is
  checkpointed before network execution; observed outcomes and usage are checkpointed afterward.
  Shared immutable model run manifests bind configuration, scope/evidence fingerprints,
  request budgets, and review destinations. See [ADR-019](ADR-019-model-run-records-and-attempt-outcomes.md).

## Request planning and catalog provenance

Model-tag review is application-owned. `AssistantService` uses current provider
role identity and local context to expose only matching, valid model profiles.
Its typed internal review guard is rechecked with the selected profile, vocabulary,
track, and context inside SQLite's manual-tag transaction. Acceptance adds selected
tags; rejection and reopening preserve manual tags. This local review path grants
no provider access and does not expose model suggestions to the deterministic
playlist projection. See [ADR-021](ADR-021-current-model-tag-review.md).

At the storage boundary, `music-storage/src/assistant/review.rs` rechecks evidence
inside the admitted write transaction. Its `review/plan.rs` helper computes tag
capacity and explicit decisions without I/O; the same transaction applies the
result and rolls back both manual tags and review records on any write failure.
Duplicate suggestions from different analyzers share one manual-tag slot, and an
overflowing selection cannot silently choose a subset based on request order.

Authoring import keeps mode/JSON source adapters, resource dependency validation,
and pure selection/mutation planning under `music-server/src/authoring/service/`.
The service replans stale commits and the existing mode coordinator owns the
recoverable write. Library cleanup similarly isolates typed operation preparation
in `music-application/src/library/cleanup/prepare.rs`, with journalled apply/revert
and recovery in `library/cleanup.rs`; the single library coordinator remains its
execution owner. These modules do not grant generated suggestions new authority.

The Mood Library's current review summary aggregates pending/accepted/rejected
suggestions by analyzer before review-state filtering and pagination. It follows
scope, search, and manual-tag filters and the same profile freshness rules. These
operator decisions are separate from model-quality evidence and lifetime history.
See [ADR-022](ADR-022-current-suggestion-review-metrics.md).

The Mood Library also exposes saved model-processing provenance independently of
tag suggestions: `model_analysis` contains `current`, `stale`, or `missing`, the
source job ID, and the saved timestamp. Empty and fully rejected model outputs
still count as processed. Strict inference/evidence/profile checks remain the
authority for currentness; outdated suggestions stay unavailable for acceptance.
`GET /api/assistant/library-tags` accepts `model_status` (processed/current/stale/missing),
`model_job_id`, and `suggestion_source` (model/metadata/catalog). Model and run filters
apply before the review summary and pagination; the source filter selects suggestions
and therefore controls the meaning of the review-state filter and summary.
Run links select currently retained profiles from that job, not lifetime history;
later inference can replace a profile. No migration or new inference is needed to
expose existing job IDs. Missing means no readable saved model profile, not proof
that no provider attempt ever occurred.

`local-metadata/v1` suggestions are title/album/genre keyword guesses. The legacy
stored prose "Mood metadata" is relabeled in the review UI, with its actual source
explained separately from AI results. This presentation correction does not alter
the inference contract, stored review signatures, or operator-owned tags.

Playlist model candidates supplement the original local pool through current
vocabulary names, aliases and context cues matched to operator-owned tags. Preserve
all local defaults and their original ranks; additions have `local_rank: null` and
start unselected. Recall uses at most a quarter of the pool (20 candidates maximum)
and never exceeds the 100-candidate ceiling. The provider-free `evaluate-playlists
--engine candidates` CLI reports retrieval separately from quality certification.
Input v4 also explains the declared meanings behind request-matched vocabulary
phrases: tag name, definition, exact matching phrases and manual labels actually
present in the disclosed candidate pool. This uses the same phrase matcher as
retrieval. Unrelated vocabulary and generated-only labels are omitted; the mappings
remain untrusted data and do not force a ranking. Disclosure v3 covers this input.
See [ADR-023](ADR-023-bounded-playlist-vocabulary-recall.md).

Application-owned `model_jobs.rs` registers feature and evaluation handlers; its
`model_jobs/` modules hold the established roles' execution paths; `cleanup_enrichment/ai.rs` owns catalog candidate review. `StructuredModelTransport`
is the outbound port. The server composes the HTTP adapter and exposes routes;
application workflows own gates, checkpoints, retry budgets, and proposal writes.

`plan_model_tagger_batches` in the application layer owns exact track partitioning.
The provider adapter validates the actual serialized envelope against the 256 KiB limit
and reserves output capacity before choosing the number of tracks.
Preview, start preflight, execution, and evaluation use the same planner; ordinary and
corrective requests must both fit. Batches contain at most 20 tracks and may be smaller.
No vocabulary entries are dropped. An oversized single-track request prevents enqueueing
a live job. Response order is immaterial, but track membership must be exact and unique.
The provider deadline covers DNS resolution through complete response-body reading.

Mood tagging input v22 uses batch-local slots, a stable vocabulary reference prefix and
per-measurement context reliability. Full membership is validated before resolving slots
back to local IDs. Explicit cache controls are limited to documented native OpenAI model
families; cache reads/writes and reasoning tokens are reported only when supplied by the provider.
The context implementation is `local-context/v2+rustfft/v2`: overlapping FFT windows cover
every half-second frame. The model projection retains all ten sections, rounded trajectory
endpoints/extremes, tempo range, voice coverage and measurement reliability. Sampled tempo
points and repeated prose stay local. `compact_context_evidence` is shared by live requests
and quality fixtures, and its per-track input is saved without the track identifier.

`ModelBatchTransport` is the separate asynchronous port; `model_jobs/batch.rs` owns the
upload/submission/collection lifecycle. SQLite schema 12 stores durable pending batches.
The server's `provider_transport/batch.rs` is restricted to native OpenAI Responses and
uses bounded pinned-DNS HTTP without automatic retries. App limits are 500 requests and
32 MiB per uploaded batch. Only the collection handler is restartable. Collection validates
current inference identity and review evidence, without requiring permission for new inference.
Pending records block model/connection and credential mutations; terminal connection deletion
also deletes its Batch records, while ordinary job history retains usage/results.

The shared mood configuration retains independent gates for tagging and cleanup.
Cleanup is optional manual-tag catalog maintenance, never a prerequisite or second pass
for tagging. Only the task being used needs its own current certification. Inference
identity hashes the task's rendered prompt/schema and meaningful inference settings; runtime
source fingerprints still govern certification. Per-track evidence/vocabulary/context hashes
remain mandatory. See [ADR-025](ADR-025-bounded-shared-mood-inference.md) and the
[context review](MOOD_CONTEXT_REVIEW.md) for the measurements and remaining evaluation work.

Catalog lookups acquire a source execution lease before reading settings or credentials.
Policy and credential edits return `cleanup_source_busy` while a lookup is active; finish
or cancel that lookup before applying an edit. Enrichment and name verification share
the same gate. Queued jobs read the settings in effect when execution begins.

SQLite schema 10 gives catalog evidence a monotonic revision. Source-policy, credential,
vault-reset, and vocabulary edits invalidate cached enrichments and catalog analysis/review
rows in the same transaction. Old worker writes are rejected even after a setting is
changed back. Review signatures include indexed metadata, mapper identity, and evidence
revision, so an old review cannot accept a newly generated proposal by coincidence.
Catalog profiles also record their vocabulary fingerprint. Accepted/manual tags survive
invalidation and the migration; legacy catalog proposals must be regenerated.

Catalog workflow now lives in `music-application/cleanup_enrichment/workflow.rs`.
Its `CatalogConnector` port returns typed observations; server adapters retain HTTP,
credential fallback, rooted fingerprint execution, and response parsing. The application
owns identity thresholds, fallback decisions, vocabulary mapping, cache validity and
review proposals. Malformed collection responses fail instead of being cached as empty
evidence. `catalog-evidence-policy/v8` is included in evidence signatures and invalidates
results created before bounded discovery across disc folders.

The five model tasks derive their static output shapes from the strict Serde result
types with Schemars. Required fields, nested object closure, types, nullability, and
confidence enums share one definition. Dynamic allowed IDs and task bounds extend
the generated schema; cross-field policy and local reconstruction remain in Rust.
Adversarial tests compare an independent schema validator with actual task handlers.
See [ADR-018](ADR-018-derived-model-schemas-and-catalog-ports.md) for the boundary and
the deliberate relational-validation and bounded-prose exceptions.

## End-to-end flow

```text
operator request / indexed library / local audio
                    |
                    v
       local analysis, filtering, and baseline
                    |
                    v
       disclosed, bounded task input document
                    |
                    v
   structured harness: prompt + example + JSON Schema
                    |
                    v
  explicit provider adapter + verified role configuration
                    |
                    v
 strict Serde parse -> identity/bounds validation
                    |
                    v
  local reconstruction -> durable review-only result
                    |
                    v
        explicit preview / selection / commit
```

## Shared platform map

| Concern | Current owner | Executable evidence | Rationale |
|---|---|---|---|
| Task prompt, example, schema, untrusted-data labels | [`structured_harness.rs`](../crates/music-application/src/assistant/structured_harness.rs) | colocated strict-shape and bounds tests | [ADR-007](ADR-007-algorithm-first-structured-model-harness.md) |
| Adapter/capability/role inventory and runtime fingerprints | [`providers.rs`](../crates/music-application/src/assistant/providers.rs), [`runtime_contract.rs`](../crates/music-application/src/assistant/runtime_contract.rs) | colocated inventory and digest tests | [ADR-001](ADR-001-assistant-provider-connections.md), [ADR-002](ADR-002-assistant-model-execution.md) |
| Reviewed model settings and provider alias revisions | [`provider_profiles.rs`](../crates/music-application/src/assistant/provider_profiles.rs) | provider-scoped settings and alias-boundary tests | [ADR-011](ADR-011-in-process-provider-adapter-handlers.md) |
| Provider-specific model IDs, request schemas, inference parameters, and response shapes | [`provider_handlers.rs`](../crates/music-server/src/provider_handlers.rs) | transport-free production-shaped request and parser tests | [ADR-011](ADR-011-in-process-provider-adapter-handlers.md) |
| Bounded request execution | [`provider_transport.rs`](../crates/music-server/src/provider_transport.rs), task types under [`assistant/`](../crates/music-application/src/assistant) | local fixture-server, strict parsing, and bounds tests | [ADR-002](ADR-002-assistant-model-execution.md), [ADR-007](ADR-007-algorithm-first-structured-model-harness.md), [ADR-011](ADR-011-in-process-provider-adapter-handlers.md) |
| URL validation, SSRF boundary, redirect refusal, byte/time limits | [`provider_transport.rs`](../crates/music-server/src/provider_transport.rs) | pinned-DNS, special-range, redirect, timeout, and response-limit tests | [ADR-001](ADR-001-assistant-provider-connections.md) |
| Credential encryption, initialization, reset, and offline rotation | [`crypto.rs`](../crates/music-storage/src/crypto.rs), [`provider_credentials.rs`](../crates/music-server/src/provider_credentials.rs), [`providers.rs`](../crates/music-storage/src/providers.rs) | Python-compatibility fixture plus reset/rotation transaction tests | [ADR-001](ADR-001-assistant-provider-connections.md) |
| Role preparation and stale-gate enforcement | [`providers.rs`](../crates/music-application/src/assistant/providers.rs), [`provider_api.rs`](../crates/music-server/src/provider_api.rs) | role fingerprint, conformance, quality, and active-job tests | [ADR-004](ADR-004-durable-model-quality-gates.md) |
| Run manifests and attempt/token accounting | [`provider_usage.rs`](../crates/music-application/src/assistant/provider_usage.rs), [`model_jobs.rs`](../crates/music-application/src/assistant/model_jobs.rs) | SQLite-backed provider attempt fault tests and local HTTP fixtures | [ADR-019](ADR-019-model-run-records-and-attempt-outcomes.md) |
| Durable job lifecycle | [`jobs.rs`](../crates/music-application/src/jobs.rs), [`jobs.rs`](../crates/music-storage/src/jobs.rs), [`jobs.rs`](../crates/music-server/src/jobs.rs) | persisted-boundary fault tests in `music-storage` | [Repository persistence rules](ENGINEERING.md#persistence-and-deployment) |
| Browser API/types and review workflows | [`frontend/src/core/api.ts`](../frontend/src/core/api.ts), [`frontend/src/views/assistant/`](../frontend/src/views/assistant) | colocated Vitest files | [Assistant UX philosophy](assistant-ux-philosophy.md) |

## Current contract inventory

The exact values below are Rust constants included in the executable runtime fingerprint. Update
the owning code, this table, its evaluation suite, disclosure copy, and tests together.

Shared contracts:

- harness: `assistant-structured-harness/v3`
- provider conformance result: `assistant-provider-conformance/v3`
- provider conformance challenge: `assistant-provider-conformance-challenge/v5`
- model settings profiles: `assistant-model-profiles/v1`
- DeepSeek Chat adapter: `deepseek-chat/v1`
- DeepSeek Responses adapter: `deepseek-responses/v1`
- OpenAI Responses adapter: `openai-responses/v1`
- standard adapter: `openai-compatible/v1`
- strict-schema adapter: `openai-compatible-json-schema/v1`
- Google Gemini adapter: `google-gemini-openai/v1`
- Google Gemini strict-schema adapter: `google-gemini-openai-json-schema/v1`

Use `openai-responses/v1` with the exact base URL `https://api.openai.com/v1` for OpenAI. It sends
native Responses requests with `max_output_tokens`, `reasoning.effort`, and the task schema under
`text.format`; its wire projection removes only unsupported `uniqueItems` while keeping OpenAI's
supported array and string constraints. The generic adapters are reserved for third-party
OpenAI-compatible services.
Both DeepSeek adapters pin `https://api.deepseek.com`. Chat uses JSON-object output,
`thinking.type`, and `reasoning_effort`; Responses uses `text.format` with JSON Schema and
`reasoning.effort`. Both enforce the complete task schema locally. The reviewed model profile
controls available effort levels; discovery alone does not establish a model capability.

Both Gemini adapter IDs use the exact base URL
`https://generativelanguage.googleapis.com/v1beta/openai`, canonicalize `models/` resource IDs,
send Google's integration-identification header, and constrain results with a Gemini-compatible
projection of the task's JSON Schema. The complete generated schema remains in the fixed prompt and
is always enforced by the task's local strict Rust validation; the provider projection removes only
JSON Schema keywords outside Gemini's documented structured-output subset.
The older Gemini strict-schema ID remains a saved-connection compatibility alias. Provider error
payloads may contribute only allowlisted machine codes; upstream messages never reach diagnostics.

| Role | Runtime fingerprint fragment | Disclosure | Engine/storage identity | Quality gate | Live job |
|---|---|---|---|---|---|
| Playlist planning (`playlist_planner`) | `assistant-playlist-planner-input/v4+output/v1+closed-ids/v1` | `assistant-playlist-model-disclosure/v3` | `model-playlist-planner/v2` | `playlist-quality-v1` | `assistant.model-playlist-suggestion` |
| Mood tagging (`music_tagger`) | `assistant-music-tagger-input/v22+output/v4+local-context/v2` | `assistant-model-music-tagging-disclosure/v13` | `model-context-tagger/v7` | `music-tagging-quality-v1` | `assistant.model-music-tagging` |
| Mood-tag cleanup (`tag_cleanup`) | `assistant-model-tag-cleanup-input/v3+output/v2+incidental-text-bounds/v1` | `assistant-model-tag-cleanup-disclosure/v3` | `model-tag-cleanup/v3` | `tag-cleanup-quality-v1` | `assistant.model-tag-cleanup` |
| EQ assistance (`eq_assistant`) | `assistant-eq-draft-input/v2+output/v1+incidental-text-bounds/v1` | `assistant-eq-draft-disclosure/v2` | `model-graphic-eq/v2` | `eq-quality-v1` | `assistant.model-eq-draft` |
| Library metadata (`library_cleanup`) | `assistant-library-cleanup-input/v1+output/v1+closed-evidence/v1` | `assistant-library-cleanup-disclosure/v1` | `model-catalog-adjudication/v1` | `library-cleanup-quality-v1` | `assistant.model-library-cleanup` |

Full task output contracts are `assistant-playlist-planner-output/v1`,
`assistant-music-tagger-output/v4`, `assistant-model-tag-cleanup-output/v2`,
`assistant-eq-draft-output/v1`, and `assistant-library-cleanup-output/v1`.
Only `audio_analyzer` remains reserved (`reserved-audio-analyzer/v1`).
`library_cleanup` is configurable in **AI setup**, with independent conformance,
`library-cleanup-quality-v1` certification and per-request disclosure consent.
Task workspaces choose whether to use a configured model without duplicating configuration.

The Library cleanup workspace preserves a separate local authority boundary. The local engine
produces filename, folder, and embedded-tag proposals; `cleanup_batches` journals only explicitly
selected writes. **History & rollback** reads those server journals, downloads the complete JSON,
and invokes the existing conflict-aware revert path. **Sources** exposes only implemented adapters.
**Rejected suggestions** is a persistent, searchable pool for file, folder and metadata proposals.
Rejecting is an explicit row action; leaving a checkbox unticked does not record a rejection.
Schema 13 stores the bounded proposal, its evidence references, indexed context signature and
rejection time separately from cleanup journals. Items remain available after restart, source-policy
changes, moves or track removal. Search is literal text across path, field and old/new values; pages
contain 50 newest-first items with an ID cursor. There is no automatic retention purge.
Matching is bounded to 100 proposals per request and excludes job-specific operation IDs from
decision identity. Kind/field/value, rules, grading, provenance, stable local/catalog/model context,
catalog revision where applicable and the review policy all participate. Changed evidence can
therefore surface a new suggestion while the old rejection remains in the pool. All potential
disc siblings participate in indexed context, even outside the selected scope.
The browser checks rejections after local/catalog merging, edition changes and model review;
rejected operations are removed from selection as well as display. **Restore to review** checks
current indexed evidence again in the storage write transaction, removes the rejection and returns
the stored proposal to an unchecked review. It does not write files. Applying that reviewed change
uses the existing coordinator and journal, including old-value checks and rollback. Stale entries
offer **Check again** with their track/folder scope; **Remove rejection** clears the decision without
applying its value. All pool endpoints require operator authentication. Database mood-tag decisions
remain in their separate controlled-vocabulary review workflow.
Filename collisions first try a distinguishing indexed artist, album, disc/track position,
artist plus album, or album plus position, in that order. They fall back to a deterministic
numeric suffix (`Song (2).mp3`, then `(3)`, etc.), reserving indexed filenames and earlier
proposals in the same folder, case-insensitively. Unreviewed tag proposals never supply a
collision label. Added labels replace control/Windows-reserved punctuation and the resulting
filename is bounded to 240 UTF-8 bytes including its extension, retaining the suffix.
An unusually long extension that prevents a bounded suffix produces a note without a rename.
These proposals are low-confidence and start unchecked; the note explains that matching names
do not establish duplicate audio. Embedded titles are not suffixed. Existing apply-time conflict
checks still reject destinations occupied since analysis, including files absent from the index.
Folder metadata evidence always uses all indexed siblings, even when only selected tracks are
being cleaned. Selection limits proposals, not the evidence used to infer their metadata.
Conflicting nonempty artist/album-artist values or collective credits (such as Various Artists)
veto artist inheritance. Conflicting album values also veto inherited artist, album, disc and
year suggestions. Missing values are not disagreement. Per-file filename evidence remains
available; consistent compilation albums may still supply album tags. Review notes explain
withheld inheritance. Folder rebuilds require album agreement and prefer a unanimous album
artist over per-track artists. This deliberately leaves more uncertain fields unresolved;
the collective-credit vocabulary is conservative and does not classify every compilation.
`musicbrainz`, `acoustid`, and `lastfm` policies are stored in `cleanup_source_policies`.
The comparison key retains Unicode letters/digits while folding case and accents.
Catalog lookup reads all embedded tag containers for typed recording/release/release-track/group
IDs, ISRCs, barcode, catalog number, secondary text and full dates. These observations stay in
cleanup evidence, outside the playback metadata contract. Staged tag edits verify that existing
recording/release/track identifiers survive readback, including the ID3 TXXX conversion workaround
for Lofty 0.25.1. MP3, FLAC, Ogg and MP4 fixtures exercise this preservation. Selected JSON imports use the same
field types, up to 500 unique tracks in the selected scope; arbitrary paths and executable sidecars
are not accepted. Local cleanup hypotheses can improve retrieval without first writing tags.
Conflicting recording IDs abstain. MusicBrainz recording ID lookup precedes bounded ISRC searches.
If neither identifies the recording, one explicit release ID adds up to two title/release-ID
searches before the ordinary title/artist searches. Without a release ID, an unresolved ordinary
search can add up to two song-title/album-title searches, including tracks with missing artists.
Conflicting release IDs suppress album-scoped retrieval; independent recording lookup remains
available. Duplicate query terms are sent once, including when only position hypotheses differ.
If these queries remain unresolved and there are no explicit release IDs, `discovery.rs` can
query at most three independently titled, already-tagged siblings in deterministic order.
All indexed tracks in the same non-root folder are eligible, even outside the selected scope.
Nonempty album tags must agree with each other and the current album hypothesis; artists may
differ on compilations. Anchor searches use raw indexed title/artist/album, never inferred or
newly proposed values, and use the existing identity score, margin and duration checks.
At least two distinct recording IDs must share one or two eligible release IDs with matching
album titles. Every successfully matched anchor participates in the intersection. No common
release, more than two common releases, or any failed anchor request withholds expansion.
Each shared release allows up to two target-title queries (at most seven added requests total,
using the existing connector cache and rate limit). All responses merge before selection;
a failed shared-release query withholds text selection from this fallback. Discovery IDs are
retrieval hints only and never become explicit release evidence or bypass edition ambiguity.
Notes retain anchor track IDs, title/artist, recording IDs, release counts and searched release IDs.
Explicit sibling disc folders (`Disc 1`, `CD_02`, `disk-3`) under the same non-root album parent
may share this retrieval context. Labels use the same positive 1-99 parser as local cleanup.
Bare numbers, part/bonus folders, root-level discs, nested extras and other edition parents
do not expand the context. More than 20 disc folders, duplicate disc numbers in different
folders, contradictory indexed disc tags or disagreeing nonempty album tags withhold expansion.
The current song must have album evidence; its embedded/imported album and disc observations
must also agree, including observations that do not override already-authored tags.
When expansion is withheld, the existing same-folder discovery remains available with a note.
Anchors prefer the current disc and then previously unsampled disc folders, with path order
breaking ties; duplicate titles remain ineligible as independent evidence. The three-anchor
and seven-request caps still apply, so this is sampled corroboration rather than a complete
validation of every disc. Only raw indexed neighbors supply search anchors. Cross-disc artist
inheritance, edition selection and review application are not introduced by grouping folders.
For remaining unmatched tracks, `aliases.rs` searches up to two distinct original/hypothesized
artist names against the artist name, alias and sort-name indexes (ten hits per query).
More than three unique artist IDs withholds expansion; otherwise each gets one `inc=aliases`
lookup. The requested spelling must match a retrieved name, sort name or alias using the existing
Unicode comparison. Aliases are bounded to 100 unique strings in deterministic order; names are
bounded to 512 bytes. The full credit is never split into guessed artist names.
Each verified artist allows at most two title/artist-ID recording searches, with the normal
duration range and 25-hit cap (at most eleven requests before cache reuse). Returned recordings
must actually credit the requested artist ID; complete joined credits are preserved. All
candidates merge before selection, duplicate queries/IDs are deduplicated, and failures make
results partial. Alias retrieval never substitutes a name in the acceptance query or raises
confidence; exact original/local full title and artist remain required for text acceptance.
An incomplete lookup cannot create a text winner by dropping an unavailable competitor.
Notes show searched spellings, artist IDs, checked names and withheld expansion. Alias spellings
that differ from the full recording credit only provide review candidates, including for the
existing optional model review; no additional provider or model request is enabled.
Opt-in AcoustID remains the fallback after text retrieval.
Text search retrieves 25 candidates with a +/-10-second duration range; zero/unknown duration
omits that search constraint. Exact title/artist, duration, weighted score and margin still govern
text selection. Missing artists are not inferred from an album search hit. Lookup notes identify
the album/release scope and returned count. Repeated recording IDs merge deterministically and
retain linked release observations from all queries; other recording IDs remain competitors.
Fetched recording details must still agree with text-match title/artist evidence and known
duration. Contradicting candidates are withheld from automatic and model-assisted proposals.
Multi-recording fingerprint mappings remain competitors.
Scores are matching heuristics, not calibrated probabilities. A text-request failure permits
fingerprint fallback and makes the result partial rather than hiding that failure in cache.

Release browsing retrieves at most 100 editions, with at most five detailed alternatives.
An album title alone never selects among multiple editions. A typed release ID can target an edition
outside that shortlist. The bounded assignment matches up to 100 folder tracks (including the
current track) against 500 release slots with unmatched alternatives. It preserves compilation
artists, repeated-recording ambiguity and missing tracks. Review chooses an edition for applicable
tracks in one folder; its proposals start unchecked. Corroborated high-confidence positions
from filenames/disc folders can support assignment without changing indexed tags; authored/imported positions take precedence
and conflicting position observations suppress filename fallback. Low-confidence number guesses
are excluded from assignment hypotheses. Recording first-release date is retained
separately from edition year. MusicBrainz genres and credits remain attributed observations;
bounded genre proposals can update embedded genre through the journal. Last.fm receives the
identified recording MBID and maps top tags by exact controlled-vocabulary names or aliases.
Catalog metadata and mood tags are suggestions, never direct writes. Metadata returns through
the cleanup diff/journal; accepted mood tags use the existing database-tag review transaction.
Local and catalog alternatives remain visible together, and selection enforces one value per field.
Bulk selection leaves conflicting values unresolved. AI candidate choices are separately labeled.

`library.cleanup-enrichment` is a restartable provider-lane job bounded to 500 tracks. The browser
uses the local analysis's scanned count to skip oversized catalog jobs, keeps local proposals
reviewable, and explains the smaller folder/selection requirement even when there are no local
issues. The job still checks its resolved scope at execution time for direct callers and index
changes between analysis and execution. Cache keys
include exact indexed source, local observations, raw discovery context (including potential
disc siblings even when their tags veto grouping), same-folder hypotheses in deterministic
path order, and source/vocabulary revision. An anchor's authored tag change expires
the cache even when local inference could reconstruct the previous value.
Complete matches expire after seven days; unmatched results after six hours; future timestamps
and partial connector failures cannot be reused. The connector keeps bounded in-memory entity and
local-fingerprint caches (256 entries each, one hour); fingerprint keys include rooted path and
actual size/mtime, with a post-computation check. Ordinary text, ISRC, album and artist queries
share the entity cache; malformed recording/artist search lists and invalid artist details are
validated before cache insertion so an explicit retry can obtain a corrected response.
**Refresh catalog results** bypasses result reuse
and clears these connector caches. A changed file stat requires library rescan. Extra tag parsing
runs in one awaited blocking task at a time in the serialized provider lane.

Authenticated operators can save, explicitly replace, or
remove AcoustID and Last.fm keys under **Library cleanup → Sources**. They use dedicated records in
the same AES-GCM vault as model-provider credentials; the browser receives only saved state, source,
and a masked hint. A saved key takes precedence immediately, while `CLEANUP_ACOUSTID_API_KEY` and
`CLEANUP_LASTFM_API_KEY` remain deployment-managed fallbacks. Credential replacement or removal
invalidates affected enrichment evidence. Disabling Last.fm atomically clears its cached generated profiles and review decisions while
leaving already accepted operator tags intact. Arbitrary URL scraping is not a supported source
contract.

The optional `assistant.model-library-cleanup` job handles one unresolved track from a completed
catalog job whose evidence is less than six hours old. Expired or changed evidence fails before
the provider call. The raw indexed discovery-context signature is checked both before the request
and before returning proposals, so changed, added or moved neighbors, including other disc folders,
cannot leave sibling-derived candidates current.
It is non-restartable and checkpoints one provider attempt before external cost.
`LibraryCleanupModelTask` discloses bounded indexed title/artist/album/duration and up to 25 catalog
candidates with opaque IDs and local comparison facts. Paths, track IDs, raw sidecars, webpages,
credentials and audio are excluded. The output selects a supplied candidate or explicitly abstains;
selection must cite at least two supporting references belonging to that candidate and cannot
cross a local version/duration contradiction or distinguish two candidates with identical
disclosed identity evidence. The server reconstructs only candidate title/artist
proposals, starts them unchecked and checks role/source revision and track signature again after
the call. No model-authored metadata value reaches the cleanup journal.

The eight synthetic quality cases cover version/order changes, indistinguishable recordings,
Unicode, absent evidence, duration contradiction and injected instructions. All must pass for a
configured model to become usable. This is a pilot gate, not measured accuracy on a private library.
Additional providers, OCR/audio models and broader recognition remain conditional research pilots;
see [metadata research](LIBRARY_METADATA_RESEARCH.md) for source policies and the held-out benchmark.
The [offline metadata pilot](LIBRARY_METADATA_PILOT.md) scores exported catalog proposals against
independent recording/edition/field labels with artist/release families kept in one split.
After a completed catalog lookup, the browser can download the original job JSON, including
empty results. The export preserves provider evidence and proposals before local/edition/model
review changes; downloading neither makes provider requests nor applies metadata changes.
Completed-run summaries expose incomplete evidence, including the unmatched subset,
even when there are no proposed edits. A completed job is not a claim that every
provider lookup succeeded; available proposals remain reviewable without an automatic retry.
Catalog failure notes retain typed HTTP status, timeout/transport and response-format
categories, plus numeric Last.fm API error codes. They exclude request URLs, credentials,
raw response bodies and provider messages. Existing error codes, partial-result cache
rules, matching thresholds and retry policy remain unchanged.
It reports missing/failed results, unknown labels and harmful changes separately. Paired run
comparisons identify per-track regressions, improvements and mixed changes, default to the
development split and require an explicit holdout selection. Unknown labels and unavailable
results cannot establish safer identity/field proposals. It neither certifies a model nor
applies library changes.

## Workflow traceability

| Workflow | Local authority and provider contract | Durable/API layer | Suite and regression tests | Decision record |
|---|---|---|---|---|
| Playlist draft | [`planner.rs`](../crates/music-application/src/assistant/planner.rs), [`model_playlist.rs`](../crates/music-application/src/assistant/model_playlist.rs) | [`assistant/mod.rs`](../crates/music-server/src/assistant/mod.rs), [`model_jobs.rs`](../crates/music-application/src/assistant/model_jobs.rs) | local [`playlist-local-v1.json`](../crates/music-application/src/assistant/evaluation_suites/playlist-local-v1.json), model [`playlist-model-v1.json`](../crates/music-application/src/assistant/evaluation_suites/playlist-model-v1.json), colocated task/runtime tests | [ADR-003](ADR-003-hybrid-model-playlist-evaluation.md), [ADR-005](ADR-005-consent-bound-model-playlist-suggestions.md), [ADR-007](ADR-007-algorithm-first-structured-model-harness.md) |
| Local track context | [`context.rs`](../crates/music-analysis/src/context.rs), [`voice.rs`](../crates/music-analysis/src/voice.rs), [`local_analysis.rs`](../crates/music-application/src/assistant/local_analysis.rs) | [`analysis.rs`](../crates/music-server/src/analysis.rs), [`assistant/mod.rs`](../crates/music-server/src/assistant/mod.rs) | controlled numeric, exact-model, checkpoint, and runtime route tests | [ADR-008](ADR-008-comprehensive-local-track-context.md), [ADR-009](ADR-009-opt-in-local-voice-analysis.md), [ADR-014](ADR-014-perceptual-context-measurements.md) |
| Mood-tag suggestion and review | [`model_tagger.rs`](../crates/music-application/src/assistant/model_tagger.rs), [`vocabulary.rs`](../crates/music-application/src/assistant/vocabulary.rs), [`tags.rs`](../crates/music-application/src/assistant/tags.rs) | [`model_jobs.rs`](../crates/music-application/src/assistant/model_jobs.rs), [`assistant/mod.rs`](../crates/music-server/src/assistant/mod.rs), storage review transactions | [`music-tagging-v1.json`](../crates/music-application/src/assistant/evaluation_suites/music-tagging-v1.json) and colocated contract/runtime tests | [ADR-006](ADR-006-review-only-model-music-tagging.md), [ADR-007](ADR-007-algorithm-first-structured-model-harness.md), [ADR-008](ADR-008-comprehensive-local-track-context.md) |
| Mood-tag cleanup | [`model_tag_cleanup.rs`](../crates/music-application/src/assistant/model_tag_cleanup.rs), [`tags.rs`](../crates/music-application/src/assistant/tags.rs) | [`model_jobs.rs`](../crates/music-application/src/assistant/model_jobs.rs), [`assistant/mod.rs`](../crates/music-server/src/assistant/mod.rs) | [`tag-cleanup-v1.json`](../crates/music-application/src/assistant/evaluation_suites/tag-cleanup-v1.json) and colocated strict-result tests | [ADR-007](ADR-007-algorithm-first-structured-model-harness.md) |
| EQ draft | [`model_eq.rs`](../crates/music-application/src/assistant/model_eq.rs) | [`model_jobs.rs`](../crates/music-application/src/assistant/model_jobs.rs), [`assistant/mod.rs`](../crates/music-server/src/assistant/mod.rs) | [`eq-assistant-v1.json`](../crates/music-application/src/assistant/evaluation_suites/eq-assistant-v1.json) and envelope/runtime tests | [ADR-007](ADR-007-algorithm-first-structured-model-harness.md) |
| Shared quality certification | [`model_quality.rs`](../crates/music-application/src/assistant/model_quality.rs) | [`provider_api.rs`](../crates/music-server/src/provider_api.rs), [`model_jobs.rs`](../crates/music-application/src/assistant/model_jobs.rs) | fixed suites plus complete/retest identity tests | [ADR-004](ADR-004-durable-model-quality-gates.md) |

## Provider disclosure boundaries

| Workflow | Sent | Kept local |
|---|---|---|
| Playlist | operator request and at most 100 locally eligible, path-free candidates with bounded evidence and the local plan | library paths, excluded tracks, credentials, playlists, review history, final public reconstruction |
| Mood tagging | at most 20 tracks per request; bounded artist, album, origin, and genre metadata, complete controlled vocabulary, optional bounded current context | track and display titles, file and folder names, every library path, audio, waveforms, spectrograms, full timelines, database mood tags, generated suggestions, reviews, credentials |
| Mood-tag cleanup | unresolved source IDs/names and usage counts, canonical vocabulary IDs/names/groups/definitions | track metadata, paths, audio, playlists, generated tags, review history, credentials |
| EQ | operator goal, fixed ten-band frequencies, deterministic baseline guidance, per-band limits | songs, audio, library metadata, paths, playlists, existing presets, credentials, final preset document |

Playlist retrieval keeps the original local rank, then appends additional eligible candidates
found through controlled-vocabulary aliases and semantic cues, up to the same 100-candidate
disclosure limit. Canonical display titles override conflicting raw scanner titles; artist names
and filesystem paths remain searchable evidence but cannot create mood axes. Candidate percentages
shown after model ranking are explicitly labeled as local evidence, not model confidence.

Tagging suite `controlled-vocabulary-tagging-baseline-v24` uses 57 bundled-vocabulary,
five custom-vocabulary, and one 200-tag scenario. `tagging_evaluation.rs` isolates
vocabularies during batching and validates fixed fixture identities for retests.
Each vocabulary group and the context-only subset (no descriptive metadata) must independently
meet the existing 90% threshold; all blocking failures remain blocking. Seven added acoustic
cases cover supported calm/urgent/chaotic impressions, gain invariance, conflicting endings,
weak tempo and missing measurements. These fixtures do not establish listening accuracy. The thirteen safety scenarios are repeated once.
Progress and the completed score both count 63 distinct scenarios; safety scenarios finish
only after their rerun. Detailed progress reports the 76 individual checks separately from
provider requests. Diagnostic retests label their selected subset explicitly.
Suite v23 retains all expected tags and the 90% gates, but supplies explicit inquisitive
and suspenseful genre evidence in two previously ambiguous metadata fixtures. Input v22
explains the recording-level contribution to intensity and avoids counting these correlated
measurements as independent mood evidence. Quality result v5 retains bounded public evidence
and confidence for primary and safety-repeat answers; historical reports without it still load.
Suite v24 corrects the sustained-drive fixture's synthetic intensity from 0.83 to 0.677,
consistent with its supplied loudness, drive and density. A regression checks the four steady
acoustic controls' opening/ending intensity against the DSP formula, allowing fixture rounding;
it does not equate independently computed medians or percentiles. All expected tags and gates
remain unchanged. The correction requires fresh matching conformance and full quality evidence;
it does not certify any model or establish that this contradiction caused a prior abstention.
Playlist reports separately record labelled candidate recall before model ranking,
including missing candidate IDs, even when the provider fails. These are synthetic
diagnostics; they do not establish live-library recall or change retrieval policy.
See [ADR-020](ADR-020-vocabulary-quality-and-candidate-recall.md).

Cleanup suite `controlled-vocabulary-cleanup-baseline-v7` uses 15 bundled cases,
four custom cases, and a 20-source case with 200 canonical tags. It retains the
all-cases-must-pass rule and reports vocabulary groups separately. Case identity,
required labels, production inputs, and complete result membership are validated
before certification; deterministic aliases never require a provider call.

Quality suites exercise the same production request shape: 20-track tagging batches,
20-source cleanup batches, and the 100-candidate playlist boundary. Playlist certification
also scores target-duration error and selected-artist diversity, and requires one semantic
uplift case that cannot pass by merely echoing the local tie order. EQ includes semantic goals
outside its deterministic keyword rules so copying the baseline is not sufficient.

The concrete `shared_with_provider` and `never_shared` lists returned by each availability endpoint
are the consent surface. Any data-category change requires a new disclosure version and invalidates
prior consent.

## Safe change procedure

1. Identify whether the change affects only local evidence, provider input, provider output,
   disclosure, storage identity, execution transport, or review/commit behavior.
2. Change the strict Rust type and local validators first. Generate the provider schema from that same
   model; do not hand-maintain a second schema description.
3. Advance the smallest owning contract version. If runtime behavior or harness semantics changed,
   also update `MODEL_ROLE_RUNTIME_CONTRACTS` or the shared harness version so old gates become stale.
4. Update the fixed task example, request-specific schema closure, synthetic suite, negative cases,
   and privacy assertions. Never use private library data as a checked-in fixture.
5. Update the task disclosure when any sent/retained data category or retry/cost boundary changes.
6. Put provider-specific model-ID, endpoint, schema-dialect, inference-parameter, or response-shape
   differences in a versioned handler. Keep network I/O in the shared transport, update the handler
   fingerprint coverage, and never select behavior from a connection name, URL, or model-name guess.
7. Update this inventory and amend the relevant ADR when the reasoning or trade-off changed.
8. Use [the validation matrix](VALIDATION.md) for the changed surface. Prose-only edits
   require document checks; harness/runtime changes require affected Rust, schema,
   negative-case and frontend gates. Before enabling a changed configuration in
   production, require its current real-provider conformance and quality evidence.
9. Treat real-provider and real-audio checks as manual acceptance. Passing mocked automation does
   not establish compatibility with a provider or accuracy on the operator's library.

## External foundations

The harness-specific standards, vendor behavior, security guidance, and the reasoning derived from
them are recorded beside the decision in [ADR-007](ADR-007-algorithm-first-structured-model-harness.md#sources-and-rationale).
Those references support the architecture; they do not replace local validation, synthetic quality
suites, provider conformance, or explicit human review.

## Detailed task rules

These implementation constraints were consolidated from the root agent guidance.
Read the subsection for the task being changed; the inventory above owns current
version values and the tables locate the corresponding code and tests.

### Draft authority

- Assistant suggestions are read-only drafts until the operator explicitly previews and commits
  them through Authoring import. Keep local heuristics and future model providers behind the same
  suggestion contracts; never let a ranking engine write playlists or mutate the library directly.

### Playlist evaluation

- Playlist recommendation changes must run the versioned synthetic suites under
  `crates/music-application/src/assistant/evaluation_suites/` through the provider-neutral evaluator. Add
  representative cases and explicit thresholds without copying private library data or freezing
  one incidental exact ranking. Future model providers must pass the same unknown-track,
  source-integrity, exclusion, selection-plan, and candidate-limit checks before UI integration.

### Connections, roles, and credentials

- Optional model providers use encrypted connection records and per-task role mappings. Each
  connection owns exactly one credential; roles reference connections so tasks may deliberately
  reuse one credential or choose separate connections, including separate keys for the same
  provider. Never store a credential directly on a role.
  Credential presence is an explicit server-derived state; never infer it from a masked hint.
  Removing or replacing a connection credential keeps role drafts but resets verification,
  conformance, and quality results, so enabled roles remain ineffective until the new credential is
  saved and every gate passes again.
  Adapters declare transport capabilities; `/models` verification proves access and discovers IDs
  only. The legacy `verified_capability_ids` field is returned empty. Roles declare required
  capabilities, and their exact settings must pass conformance before execution. Provider-scoped,
  reviewed model profiles describe supported effort levels and known alias revisions. Unknown
  models remain explicitly unreviewed and require an operator-triggered test. Validate settings
  during role save, testing, enablement, and execution; do not guess a provider from a model name.
  Roles without a complete feature, quality, consent, and review contract stay explicitly
  unavailable for configuration even if their future role ID is already reserved.
  Never return or log a provider key, never infer provider capabilities from a saved URL, and never
  enable a role until the operator explicitly verifies its connection and its exact runtime
  configuration passes the fixed synthetic conformance challenge. Provider I/O must stay off the
  event loop, bounded by request size, time, and response size, and protected against redirects and
  unsafe destinations. Keep provider-specific model-ID normalization and inference parameters in
  explicit versioned adapter handlers; handlers shape requests but must not bypass the shared
  pinned-DNS transport or infer behavior from connection names or URLs. Exact model IDs are matched only within the
  selected adapter's reviewed profile registry.
  OpenAI-compatible structured requests must carry the generated task JSON
  Schema. The standard adapter uses JSON-object response mode; the explicit strict adapter may use
  `json_schema` only when selected and proven by conformance. Each fixed feature prompt includes a
  locally validated example of its strict output shape. Include the versioned harness, conformance,
  model-profile revision (including announced remote alias transitions), and per-role feature
  contracts in the runtime fingerprint so a transport or task-contract change
  makes existing model tests and quality results stale instead of silently reusing them. Feature
  code resolves usable roles through `prepare_role_execution()` and
  owns a fixed prompt plus strict result schema; do not expose a browser-facing general prompt
  endpoint. Saving, verifying, or testing a role does not authorize sending library data or
  replacing a local engine. Preserve the Assistant credential master key separately from database
  backups. `ASSISTANT_CREDENTIAL_KEY` takes precedence over the fixed
  `ASSISTANT_CREDENTIAL_KEY_FILE`; the authenticated API may exclusively create only that configured
  file and must never accept a path, return the key, overwrite an existing file, or generate a new
  key while saved provider credentials exist. Saved provider credentials are write-once and must be
  explicitly deleted before another key can be added. The password-confirmed browser reset may
  remove only the configured file-backed key after atomically erasing all saved credentials and
  resetting every provider/model gate; preserve connection and role drafts, refuse active provider
  jobs, and report post-commit file-removal failure as a partial result. Environment-key removal and
  credential-preserving rotation remain explicit console/offline maintenance workflows.

### Offline credential recovery

- Credential recovery checks and master-key rotation are offline operator workflows. Keep audit
  output secret-free and identify keys only by a short one-way fingerprint. Rotation must decrypt
  every saved credential before mutating any row, re-encrypt all credentials in one transaction,
  and reset provider verification, role conformance, and model-quality gates. Require an explicit
  server-stopped acknowledgement before applying it; a dry run is the default.

### Playlist planning

- The optional model playlist planner may run only through the dedicated consent-bound durable job.
  Keep `local-planner/v2` as the default, require the exact current `playlist-quality-v1` pass and
  disclosure version before enqueueing, and make model jobs non-restartable to avoid silently
  repeating provider cost. Locally enforce eligibility and exclusions, send a privacy-reduced pool
  of at most 100 candidates, and preserve the original local rank while unioning additional recall
  candidates found through controlled-vocabulary aliases and context cues.
  Recall uses only operator-owned tags, reserves at most a
  quarter of the bounded pool (20 maximum), and never evicts a local default selection.
  Preserve original local ranks; additional candidates have null local_rank and start
  unselected. Candidate-only CLI reports do not certify a model. Treat a non-empty
  display title as canonical. Explain declared vocabulary meanings using only
  request-matched names/aliases/context cues, their definitions, and manual labels
  present in the disclosed candidate pool. Treat that text as untrusted data;
  never send unrelated vocabulary, infer new tags, or force a model selection.
  Do not infer mood axes from artist names or filesystem paths.
  Choose the review default with bounded duration-error improvement. Inject the exact candidate
  IDs into the output schema, and accept only
  ranked/selected IDs. Never send library-relative paths
  or trust model-supplied source fields, tags, scores, reasons, or evidence; reconstruct the public
  response from the local candidate snapshot. Model results remain drafts and must use the existing
  Authoring import preview/select/commit path. Configured-model CLI evaluation separately requires
  the explicit `--send-suite-to-provider` disclosure flag.

### EQ drafting

- The optional EQ assistant may run only through `assistant.model-eq-draft`. Build a deterministic
  intent baseline and per-band refinement envelope before the provider call. Require the exact
  current `eq-quality-v1` pass and disclosure consent, make jobs non-restartable, and send only the
  operator's sound goal plus the fixed ten-band frequencies, local guidance, and gain limits.
  Accept exactly ten gains in the local envelope and in 0.5 dB steps; construct every frequency
  and Authoring field locally. Deterministically bound overlong rationale and caution text because
  it is incidental review prose; never repair or coerce gains, frequency order, schema identity,
  missing fields, or unexpected fields.
  The result is a review-only draft and may create a preset only through the existing Authoring
  import preview/select/commit transaction. Never send songs, audio, library metadata, paths,
  playlists, existing presets, or credentials to the EQ role.

### Retired cleanup interface and planned audio tests

- AI setup, connection role labels, quality polling, and the test console exclude
  `tag_cleanup`. Mood vocabulary offers manual editing and rename/merge; local cleanup
  suggestions and legacy model-cleanup panels and browser API helpers are removed.
  Existing server cleanup contracts and stored records are retained; this UI change
  does not remove HTTP endpoints or migrate persisted configuration.
- The console includes reserved roles. `audio_analyzer` has a selectable **Planned**
  entry and a **View test plan** link from its role card, without model-test prompts,
  quality polling, or executable controls. Dedicated audio adapters, bounded input,
  separate consent, validated results, and conformance/quality suites must precede
  enabling it. Existing local track-context analysis stays independent.

### Tagging bounds and inference identity

- Tagging plans must enforce explicit track/request/reservation limits before provider I/O.
  Reservation units are conservative input bytes plus output allowance, never billed tokens or money.
  Separate result inference identity from operational certification; timeout/credential or unrelated
  source changes must not automatically rebill unchanged evidence. Meaningful task/schema/adapter
  semantics must change the inference contract. New inference still requires current certification.

Standard pilots default to 20 tracks with `stop_on_empty_batch=true`. A valid zero-tag
response is saved with its explanation, is never corrected merely for being empty, and
stops the run before another request. Count current profiles before truncating the work
list; deferred tracks are not current. Standard result v7 and Batch result v2 distinguish
processed tracks, tracks with/without tags, returned tags, saved profiles and unavailable
work. Per-track outcomes survive in bounded job results; checkpoints retain prior feature
progress through later uncertain attempts. The export reflects returned results, including
changed tracks that could not be saved, rather than claiming every row became a profile.

`ModelAnalysisStatus` projects the latest saved model profile even if empty or outdated:
count, evidence, model confidence, recorded context status and optional bounded input snapshot.
Old results show missing information explicitly; current context is not reconstructed as
historical input. The inspector separates acoustic facts, musical impressions and session
uses. Filters `with_suggestions`/`without_suggestions` are independent of freshness/review.
Selected-track reconsideration uses an explicit forced scope and normal plan/consent gates.
Accepted/manual tags never change as part of inference or reconsideration.

### Batch recovery

- With the empty-request guard enabled, reject plans containing more than one asynchronous
  request (`batch_pilot_required`) before upload. A deliberate guard override permits a larger
  Batch after review; already submitted requests cannot be stopped by local yield checks.
- OpenAI Batch uses the same strict task and review contracts. Persist state before upload/submit;
  never automatically repeat an uncertain submission. Batch collection alone is restartable and may
  collect already-paid responses without fresh certification, after checking current inference,
  vocabulary, context and profile shape. Keep remote-file IDs durable until deletion succeeds.
  Pending batches block role/connection/credential reset and rotation, including while no job runs.
  Explicitly disclose provider files, retention, cancellation costs and unknown-submission recovery.

### Mood tagging

- Optional mood tagging may run only through `assistant.model-music-tagging`. Require the
  exact current `music-tagging-quality-v1` pass and disclosure consent, batch at most 20 tracks
  per provider request, and keep jobs non-restartable. Resolve whole-library, folder
  (recursive/direct), or explicit-track scope locally. Provider input is limited to indexed
  artist, album, origin, and genre metadata, duration, BPM, batch-local numeric slots, the full revisioned
  operator vocabulary's IDs/names/groups/definitions/
  exact aliases and bounded semantic context cues, and an optional bounded projection of current
  `local-context/v2` evidence:
  loudness, intensity, rhythmic-drive, brightness, density and spectral-change trajectories;
  tempo development; major acoustic sections/transitions; repetition; confidence; and optional
  local voice/instrumental classifier score and coverage (or explicit unknown/unavailable status).
  Never send track titles, display titles, file or folder names, library-relative paths, the absolute media root, paths outside the indexed library,
  audio, waveforms, spectrograms, full-resolution timelines, database mood tags, stored
  suggestions, playlists, review history, or credentials. Local context analysis must remain
  factual and may never propose setting, period, scene, mood, genre, or instrument tags.
  Context cues are global operator-managed vocabulary guidance, not per-track local tag
  hypotheses. Broad mood impressions may use multiple consistent acoustic cues at restrained
  confidence. Emotional nuances and setting/scene/period choices need semantic support.
  Scene and setting suggestions describe editorial suitability, not a literal depicted event.
  Never force a tag, infer periods from recording technology, or turn loudness into combat.
  Keep each tag's ID, name, definition, aliases, and cues together in the provider input so the
  model never has to join a compact index to a second definition table. A run may spend at most
  two disclosed correction requests on malformed JSON, schema-invalid output, track-set mismatch,
  or unsupported tag IDs. Each correction is a fresh strict classification; never edit, coerce,
  or locally repair the rejected output, and never retry provider, network, timeout, or truncation
  failures through this budget.
  Period feel is separate from physical setting and describes the era evoked by the complete
  evidence, not release date or recording technology. It is a zero-or-one categorical group;
  `cross era` replaces rather than accompanies its component period tags. The model must choose
  zero through eight exact IDs from the full controlled vocabulary and
  return confidence plus one to four bounded evidence strings. Do not ask it for signal axes and
  do not generate a local tag-ID hypothesis before the call. Reject unknown/duplicate IDs,
  missing track IDs, malformed confidence, extra fields, and truncated output; only incidental
  evidence text may be bounded. Store output under `model-context-tagger/v7` in
  `track_analyses` and bind its source signature to metadata, current context signature (or its
  absence), vocabulary fingerprint, contract version, and role fingerprint.
  Before a live run, report full, partial, missing/stale, and failed context coverage. Let the
  operator either include incomplete tracks using metadata alone or skip every track without
  full current context. The model may never write `track_user_tags`; accepted suggestions become
  database mood tags only through explicit single or bulk review.

### Manual-tag cleanup

- Optional model-assisted manual-tag cleanup may run only through
  `assistant.model-tag-cleanup`. Run declared-alias and deterministic spelling/plural cleanup first and make no
  provider call when it resolves every candidate. Require the exact current
  `tag-cleanup-quality-v1` pass and versioned disclosure consent, allow at most 500 catalog tags,
  make the provider job non-restartable, batch at most 20 unresolved names per call, and send only
  source IDs/names and usage counts plus canonical vocabulary IDs, names, groups, and definitions.
  Require one ordered canonical-ID-or-null decision per source. Bound overlong reason text locally,
  but never repair source order, source or target IDs, confidence, missing decisions, or unexpected
  fields. Never send song metadata,
  paths, audio, playlists, generated tags, review
  history, or credentials. Store only a review-only proposal bound to the exact role fingerprint
  catalog signature, and vocabulary fingerprint. Apply only explicitly selected source/target pairs from that stored job,
  reject stale or invented selections, and commit all selected manual-tag renames atomically.

### Quality certification

- Task-specific model quality checks run as durable, non-restartable jobs and persist their current
  certification separately from job history. Bind every result to the exact model-role runtime
  fingerprint, clear it after connection reverification or runtime changes, and keep historical
  reports synthetic and secret-free. A quality pass does not authorize live-library access.
  Connection changes, credential deletion, and reverification must refuse to reset assigned roles
  while their model jobs are queued or running; the UI must warn that deliberate reverification
  clears their model tests and quality results.
  The mood-tagging suite batches 20 synthetic tracks per provider request, matching live work,
  while preserving
  per-scenario progress and diagnostics, then repeats every safety scenario once to catch unstable
  forbidden output. Provider/contract failures and forbidden false positives block certification;
  a scenario's safety label alone does not turn a required semantic-tag miss into a blocking error.
  All scenarios contribute to the suite's explicit minimum scored pass rate. Each
  fixed vocabulary group must independently meet that same threshold; keep custom
  and maximum-size fixtures isolated during batching, safety repeats, and retests. A
  failed-scenario recheck may call the provider only for failures from the exact current complete
  result and may merge those results only for diagnosis. Only another complete suite may update
  certification.

### Usage accounting

- Durable quality, playlist, tagging, and tag-cleanup model jobs record the shared bounded provider-usage
  summary: attempted calls, provider-reported model IDs, and reported input/output token totals.
  Checkpoint it after every provider attempt so failures, cancellation, and graceful shutdown keep
  the usage already incurred. Preserve missing-usage counts explicitly; never infer unreported
  tokens or portable cost from provider-specific pricing.

### Analysis identity

- Generated tag profiles remain keyed by `(track_id, analyzer_id)` in `track_analyses`.
  Comprehensive factual audio context is keyed the same way in `track_contexts` and stores its
  summary, condensed timeline, major sections, technical facts, and stage status separately from
  semantic tag suggestions. Preserve source signatures, confidence, and analyzer versioning.
  Consumers may use only current, well-formed context/profiles and must fall back safely when data
  is absent, partial, stale, failed, or malformed.

### Local voice analysis

- Optional local voice analysis may use only the checksum-pinned Essentia MusiCNN model through the
  explicit deployment setting. Keep it off by default, local-only, and non-fatal; include its
  model/runtime identity in context staleness, preserve unknown/unavailable states, and never label
  spectral heuristics as human-voice detection.

### Manual tags and suggestion review

- Database mood tags are operator-owned rows in `track_user_tags`, independent from embedded file
  tags such as album, artist, year, and genre and independent from generated analysis. Never write
  these rows into media files. Update them with additive/removal deltas, display their source explicitly,
  and pass them separately to suggestion engines. An analyzer or provider must never overwrite or
  silently promote its output into manual tags. Bulk updates commit valid tracks together and
  report missing/limited tracks; library-wide rename merges duplicate target rows atomically.
  Generated-tag decisions live in `track_analysis_tag_reviews` and bind to the reviewed source
  signature. Acceptance atomically adds the manual tag; rejection and reopening never remove or
  rewrite manual tags, and a changed analysis signature returns the suggestion to pending review.
  Current-profile consumers omit rejected tag labels without deleting the analyzer's stored profile.
  Bulk review applies only explicitly selected suggestions, commits valid decisions together, and
  reports stale, missing, or tag-limited items individually. Never add a select-all implicit write.
  Model-tag review listing and acceptance must validate current role, vocabulary, metadata,
  local-context identity, and profile shape. Recheck the typed server-owned guard inside
  the manual-tag transaction; never trust a client-provided currentness assertion.
  Review summaries count current suggestions by source after scope/search/manual-tag
  filters but before review-state filtering and pagination. Never infer decisions
  from manual tags or present these counts as model accuracy or lifetime history.
  Mood Library model-processing status comes from saved profiles, independently of
  tag count and review decisions. Preserve run ID/time for current and outdated
  results; empty results still count as processed. Apply model/run/source filters
  before review counts and pagination. Run views are retained profiles, not lifetime
  history. Label local metadata keyword guesses honestly, never as AI detection or
  embedded mood metadata.
  Tag cleanup detection is pure and conservative. Bind its preview to the current manual-tag
  catalog, require explicit per-suggestion selection, reject stale or invented selections, and apply
  all selected renames in one transaction without changing unselected tags.
