# AI setup and acceptance guide

This is the operator guide for finishing and validating the local-first Assistant,
optional model connections, review-first authoring, and automatic playlists. The
local playlist and library tools work without any provider key. Model-backed tools
are optional and fail closed when their setup or quality checks are incomplete.
Only section 1 is relevant to deploying the code; the remaining sections are
first-time setup and functional checks, not release prerequisites.

For code ownership, current contract versions, provider disclosure boundaries, linked tests, and
the safe change procedure, use the
[Assistant architecture and contract map](docs/ASSISTANT_ARCHITECTURE.md). The numbered steps below
remain the operator-facing acceptance procedure.

## What is ready

- Comprehensive local track-context analysis runs as a durable job.
- Optional local voice/instrumental analysis can enrich the same factual track context.
- Database mood tags remain separate from embedded file metadata and generated suggestions.
- The local playlist planner creates a reviewable draft and is the default.
- Optional provider models can assist playlist planning, mood tagging, mood-tag
  cleanup, library metadata candidate review, and ten-band EQ drafting.
- Every model task has a separate role. Roles may share one connection and key or
  use different connections, providers, models, and keys.
- Playlist and EQ drafts go through Authoring import preview and explicit commit.
- Model tag suggestions go through explicit generated-tag review.
- Automatic playlists use only manual/accepted tags and optional current local
  metadata analysis. They never consume unreviewed provider suggestions.

Specialized models that receive audio remain reserved. Library cleanup text AI is available
as a gated candidate-review pilot; it does not author arbitrary tags or change files itself.

## 1. Prepare or recover a deployment

The one-time Python-to-Rust cutover completed on 2026-08-29. `main` is the production Rust line;
`legacy/python` and `legacy-python-2026-08-29` preserve the final Python source for disaster
recovery only. The following procedure applies to a new deployment, a schema-changing upgrade, or
an isolated restore test:

1. Back up `app.db`, legacy `devices.json` when present, authored modes, and the separately held
   Assistant key. Keep that restore set isolated from the live container. Music and SFX remain
   under the deployment's filesystem backup policy.
2. Run `music-cli db doctor` against the copy before migration. Unknown or damaged schema shapes
   are a stop condition; `music-cli db migrate` creates and verifies its own pre-migration database
   backup before applying the ordered SQLx migrations.
3. If provider credentials are already saved, confirm that the matching Assistant
   credential master key is still available through `ASSISTANT_CREDENTIAL_KEY` or the
   dedicated key-file mount. A database backup and this key are one restore set.
4. Confirm the deployment still mounts the existing music, SFX, modes, and legacy device paths in
   their intended locations. Database migration does not rewrite media or mode documents;
   `devices.json` is imported once without modifying the source and SQLite is authoritative after
   that. Keep the normal long-term backup for media and authored modes.
5. Build and deploy only an accepted release revision through the normal CI and infrastructure
   workflow. The canonical `Dockerfile` is the Rust release image; do not copy a development
   database over production.
6. After startup, sign in and confirm that normal playback, output-device selection,
   Authoring, and the Library still work before enabling optional models.

The server refuses incompatible or future schema versions before opening them for writes. Music
and SFX remain under the normal long-term backup policy because they are valuable source data, not
because the Rust migration mutates them.

## 2. Establish the local baseline first

1. Open **Assistant -> Mood library -> Analysis**.
2. Run the comprehensive local context analysis. The job is server-owned, checkpoints each
   track, and may be closed and reopened without losing progress. It decodes audio locally into
   condensed dynamics, rhythm, spectrum, tempo, and structural development; it does not suggest
   semantic tags or send audio anywhere.
3. Open **Assistant -> Mood library -> Track context**, browse a representative set of folders, play several
   songs, and confirm that quiet openings, builds, peaks, and major sections look reasonable.
4. Add or edit database mood tags such as `medieval`, `tavern`, `dancing`, `combat`, `travel`,
   and custom campaign terms. Generated model tags are reviewed only after optional provider setup.
5. Open **Assistant -> Mood library -> Mood vocabulary**. Review the canonical names, definitions, and
   aliases that models may use; promote any deliberate custom term that models should
   be able to generate.
6. Open **Assistant -> Playlist Builder**, create at least one local suggestion, audition
   several songs, adjust the final selection, preview the Authoring import, and create
   a test playlist.
7. Create or choose a normal playlist, configure an automatic local rule, review its
   exact matching songs, and enable it. Change a relevant accepted tag and confirm the
   playlist refreshes when opened or played. Switch it back to manual and confirm its
   current songs remain.

Do not continue to provider setup until this local path is satisfactory. It remains the
privacy-preserving fallback; operator-owned tags, indexed metadata, and local analysis provide
the bounded evidence and candidates used by model workflows.

### Optional local voice analysis

Until a model path is configured, the application reports voice as `not_classified`. The supported
opt-in runs Essentia's purpose-trained `voice_instrumental-musicnn-msd-2.pb` graph directly through
the Rust tract runtime and never sends audio over the network. The MTG model weights are licensed
CC BY-NC-SA 4.0; confirm those terms fit the deployment before proceeding. The application does not
download or accept a different model or checksum silently. Runtime license handling is recorded in
[`docs/THIRD_PARTY_NOTICES.md`](docs/THIRD_PARTY_NOTICES.md).

On a supported development machine:

```bash
mkdir -p ./models
curl --fail --location \
  --output ./models/voice_instrumental-musicnn-msd-2.pb \
  https://essentia.upf.edu/models/classifiers/voice_instrumental/voice_instrumental-musicnn-msd-2.pb
echo "b734bca3fc99257cf0088211b44bd36e8a26fbb1f9ce67e1e97d39f188094b0a  ./models/voice_instrumental-musicnn-msd-2.pb" | sha256sum --check
export ASSISTANT_VOICE_MODEL_PATH="$PWD/models/voice_instrumental-musicnn-msd-2.pb"
```

The Rust image includes the native inference code but never downloads or embeds the separately
licensed model. Point it at a checksum-verified, read-only model mount:

```bash
docker build -t music-rust .
docker run -d --name music \
  -p 8000:8000 \
  -v /srv/music-data:/data \
  -v /srv/music-models:/models:ro \
  -e ASSISTANT_VOICE_MODEL_PATH=/models/voice_instrumental-musicnn-msd-2.pb \
  music-rust
```

After restarting, existing context built without that exact classifier is shown as stale. Run the
normal comprehensive context job. It first checkpoints signal, structure, tempo, and loudness for
every eligible track, then sends voice-only work to one capacity-one, job-scoped model-owning Rust
thread. The thread is joined and the compiled graph is dropped when the voice pass completes or is
cancelled, so idle service memory returns to its non-inference working set. The Library Context page
shows one progress bar for each pass. If native voice inference is interrupted, retrying the job resumes
the remaining voice rows from the saved first-pass context instead of decoding the library again.
Then inspect several known vocal, instrumental, and intermittent-vocal tracks in
**Assistant -> Mood library -> Track context**. The UI reports the normalized two-class score and
the fraction of windows where voice led instrumental. These are classifier measurements, not a
calibrated probability or guarantee; library-specific threshold calibration is not required for
this bounded factual-evidence use.
Disabling or changing the model also requires a normal context rebuild. A per-track optional-stage
failure keeps the rest of that track context and reports voice as `unavailable`; a worker failure
leaves unprocessed rows as partial checkpoints for the next retry.

## 3. Enable encrypted provider credentials

The server needs one deployment-owned 32-byte master key before it can save provider
API keys. For the standard Docker image, create and mount a private host directory:

```bash
sudo install -d -m 0700 -o 1000 -g 1000 /srv/music-secrets
# Include this option in the existing docker run command:
# -v /srv/music-secrets:/run/music-secrets
```

Restart the container, sign in, open **Assistant -> AI Setup**, and select
**Initialize secure storage**. The server creates the fixed private key file; no key
material is returned to the page.

Managed deployments may instead generate a URL-safe base64 key outside the repository:

```powershell
[Convert]::ToBase64String(
  [Security.Cryptography.RandomNumberGenerator]::GetBytes(32)
).Replace('+', '-').Replace('/', '_')
```

1. Save the result as `ASSISTANT_CREDENTIAL_KEY` in the deployment's secret store.
2. Restart the Music server. This environment value takes precedence over the key file.
3. Open **Assistant -> AI Setup** and confirm credential storage is ready.
4. Keep the master key out of `.env` files that are copied, logs, screenshots, source
   control, and provider-connection names.

The key encrypts provider credentials in `app.db`; it is not a provider API key. Losing
it does not expose a provider key, but it makes every saved credential unreadable and
requires those credentials to be entered again. For file-backed storage, **Reset AI secure
storage** can deliberately start over through the UI after current-password confirmation.
It erases all saved provider credentials and their verification/quality gates before it
removes the fixed key file, while retaining connection and role drafts. Environment-backed
keys remain deployment-managed. Use the offline rotation workflow in section 7 when saved
credentials must be preserved.

## 4. Add and verify provider connections

Create one connection for each credential or trust boundary you want. Reusing one key
is allowed, but separate connections make rotation, revocation, provider limits, and
task ownership clearer.

For each connection:

1. For OpenAI, choose `openai-responses/v1` with `https://api.openai.com/v1`.
   For DeepSeek, choose `deepseek-chat/v1` (JSON-object output) or
   `deepseek-responses/v1` (native JSON Schema) with `https://api.deepseek.com`.
   Use `deepseek-flash` for new DeepSeek configurations. Both paths require a model test.

   Choose `openai-compatible/v1` for other OpenAI-shaped services. If the
   provider explicitly documents OpenAI-style `response_format` with
   `type: json_schema`, you may instead choose
   `openai-compatible-json-schema/v1` for API-enforced strict output. Do not choose
   the strict adapter merely because the endpoint is otherwise OpenAI-compatible;
   the role conformance test is the authoritative check.

   For a Google AI Studio Gemini key, choose `google-gemini-openai/v1` and use
   `https://generativelanguage.googleapis.com/v1beta/openai`. The explicit Gemini
   handler canonicalizes model resource names and maps task thinking controls to
   Gemini's documented `reasoning_effort` field. The
   `google-gemini-openai-json-schema/v1` variant is available when the exact selected
   model passes strict-schema conformance. These profiles accept only Google's
   documented public base URL; use the generic adapter for a deliberate proxy or
   gateway.
2. Enter a clear local name, the provider's documented API base URL, and its API key.
3. Leave private-network access off for public providers. Enable it only for a service
   you intentionally run on a trusted private address.
4. Save the connection. Confirm the UI says a credential is saved and shows only a
   masked hint.
5. A saved API key is write-once. To use another key, explicitly delete the current one
   from the connection and then enter the replacement.
6. Click **Verify connection**. Verification establishes connection access and lists model IDs;
   it does not prove structured-output support or send library data.
7. If verification fails, correct the base URL, credential, TLS, or provider access.
   Do not work around a failure by enabling private-network access for a public host.

To switch an existing DeepSeek connection from a generic adapter, edit its connection type and
base URL, keep its saved credential, then verify and rerun model and quality checks. Model-profile
or handler changes make old checks stale. The announced September 14, 2026, 04:00 UTC transition
of `deepseek-v4-pro` to Flash also invalidates its evidence at that time.

Saving and verification are separate by design. A saved credential alone cannot run a
model task.

## 5. Configure each model role

Configure only the tasks you intend to use: playlist planning, mood tagging, or EQ
assistance. Mood tagging needs one model and its own checks. **Mood-tag cleanup is not
required.** It remains under **Legacy tag-name maintenance (optional)** for unresolved
manual names and shares the tagging model; open it only if you need that separate helper.

1. Select a verified connection and one of its reported model IDs.
2. Keep the role disabled while saving its initial configuration.
3. Choose a thinking setting from the model's reviewed profile. **Astra requires thinking**: use
   **Low** for less reasoning; **Off** is unavailable. DeepSeek supports Off, Low,
   High, and Maximum. **Provider default** sends no override (DeepSeek defaults to high effort).
   Unknown models show an unreviewed-settings notice and need an explicit test. Older saved
   **On** settings keep their high-effort meaning for native adapters. An unsupported saved choice
   remains visible until you select a supported setting. Set a response-token allowance large
   enough for both reasoning and final JSON. The conformance test uses that configured allowance;
   task requests can impose smaller limits. Model test logs include effective settings and any
   provider-reported reasoning usage.
4. Run the role's fixed conformance test. This makes one small provider request and
   checks strict structured output for that exact connection, model, timeout, and output
   limit.
5. Enable the role only after conformance passes.
6. Run the task-specific synthetic quality check and wait for its durable job to finish.
7. Review the report. A pass certifies only that exact runtime fingerprint; changing or
   reverifying the connection, replacing/removing its key, or changing the model/runtime
   settings requires conformance and quality to run again.

To convert an existing Gemini connection, expand **Connection settings**, select
**Google Gemini API**, keep the documented address, and save. The encrypted API key is
retained, but verification and assigned task checks are deliberately cleared. Verify
again, select the newly canonicalized bare model ID, save the task, and rerun conformance.

The four checks are intentionally independent. A model that is good at playlist ordering
may be poor at conservative EQ or metadata tagging. Provider-side spending limits remain
the authoritative cost guard; Music records reported token usage but does not estimate
portable monetary cost.

The tagging report also shows separate bundled, custom, and 200-tag vocabulary
scores. Each group must pass the same 90% gate; a high overall score cannot hide
a failed group. The context-only group has nine scenarios, so its 90% threshold
requires all nine to pass. The badge counts distinct scenarios throughout the run;
safety reruns are included in each scenario, with individual-check totals in the log.
A finished score says how many scenarios passed, whereas progress says how many were
checked. New failure reports retain the model's brief evidence, including explanations
for empty tag lists. These explanations are model claims, not verified musical facts.
Playlist reports flag relevant test tracks omitted before model
ranking, so those local preparation failures can be investigated separately.

## 6. Validate model-backed workflows with real data

Use a small, representative sample before running across the whole library.

### Playlist planning

1. In Playlist Builder, run the same request once with the local planner and once with
   the configured model.
2. Read the disclosure before consenting. The model receives at most 100 locally
   eligible, path-free candidates and returns track IDs only. It also receives the
   vocabulary names and definitions linking phrases in your prompt to those
   candidates' database mood tags. Unmatched aliases, cues, and vocabulary entries
   are omitted.
3. Close or refresh the page during one run and confirm progress/result restoration.
4. Audition suggestions one at a time. Starting another song or normal playback must
   stop the previous audition through the shared canonical playback state.
5. Adjust the selection, preview Authoring import, and explicitly create the playlist.
6. Confirm a failed model request remains visibly failed and does not silently replace
   its provenance with a local result.

### Mood tagging

1. In the Library, select **Mood tags**. Choose the whole library, the current folder
   (with or without subfolders), or the currently checked tracks. Review the scoped
   counts and estimated provider requests before continuing.
2. Start with a small representative folder or selection if provider cost or output
   quality is uncertain.
3. If the selected scope contains partial, stale, failed, or unanalyzed tracks, choose explicitly
   between running them with metadata-only context or skipping every track without full context.
4. Confirm model output appears as generated `model-context-tagger/v7` suggestions,
   separate from local analysis and database mood tags.
5. Inspect the disclosure: the model receives artist, album, origin, and genre metadata, the current full canonical
   ID/name/definition/alias list, and—when available—a bounded projection of locally measured
   rounded whole-track trends and endpoints, tempo range, all ten bounded acoustic sections, repetition, coverage confidence, measurement reliability,
   and optional local voice/instrumental classifier evidence or explicit unknown/unavailable voice
   status. It does not receive a local tag hypothesis or model-owned
   signal axes. Track titles, display titles, file and folder names, library paths, audio,
   waveforms, full timelines, and spectrograms are never sent.
6. Audition several tracks inside the review dialog, then accept, reject, and reopen
   suggestions. Only explicit acceptance may add a database mood tag; the audio file and
   its embedded artist, album, year, genre, and similar tags remain unchanged.
7. Confirm automatic playlists do not react to pending or rejected model suggestions;
   they may react after an accepted suggestion becomes a manual tag.

### Bounded runs and asynchronous Batch

To find tracks from a completed pilot, choose **View saved results from this run**
in Optional model suggestions (or the equivalent Batch link). The Mood Library
opens across the whole library with **AI processed** and **AI suggestions** selected.
Keep **Review: All states** to include tracks whose AI returned no tags, or whose
suggestions you already reviewed. **Needs review** narrows this to pending tags.
Use **Show all runs** to remove the run restriction.

Use **AI returned tags** or **AI returned no tags** to distinguish outcomes independently
of review state. Select tracks and use **Reconsider AI tags** to replace only that selection
after reviewing its plan and cost; there is no need to rebuild the whole library.

Every track shows **AI processed · current**, **AI processed · no tags**, **AI processed · outdated**, or
**No saved AI result**. The inspector includes its saved date and run ID. Outdated
means the retained result fails current evidence/configuration/profile checks;
it is still identifiable but its old tags cannot be accepted. Failed attempts
without a saved result remain in job diagnostics. Run views are not permanent
history: a later run can replace a saved profile. New runs also retain bounded returned
track outcomes in job history; **Export retained run results** preserves that record for
offline comparison, including output that could not be saved because evidence changed.
Existing profiles need no new provider call just to appear in these filters.

Empty profiles now expose the model explanation, or say that no reason was recorded.
New results also retain the exact disclosed per-track input. **Track evidence sent to the
model** summarizes metadata, pulse, voice and development; exact fields are expandable.
Old results cannot recover an input snapshot that was never saved. Complete local analysis
is coverage, not proof that a music mood classifier ran. Scene/setting suggestions are
possible session uses, while mood tags describe a musical impression.

**Metadata keyword guesses** is the corrected label for the older local suggestions
previously shown as "Mood metadata". Those guesses match words in the title, album
and genre; they are neither embedded mood fields nor AI detection. Misleading names
can produce wrong guesses. Select **Suggestion source: AI suggestions** to exclude
them from review, or reject individual guesses. Existing accepted/manual tags are
preserved. The model does not receive these guesses or the track title.

The default plan selects at most **20 tracks**, allows **10 model requests** including
contract recovery, and reserves at most **1,000,000 units**. Adjust these limits before
confirming. Reservation units conservatively combine prompt/schema UTF-8 bytes and
maximum output tokens; they are not an exact token count, price estimate, or account-wide
spending limit. Provider dashboards remain authoritative for charges and account limits.
The default guard stops before another request when a completed request returns no tags.
Review its explanations first. To continue, start a new run with Rebuild off; current empty
results are skipped too. You can deliberately disable the guard. Counters distinguish
analysed tracks, tracks with/without tags, saved profiles, current and deferred work.
A cancelled or failed run retains completed results and recorded provider usage.

A later ordinary run skips valid current results. Use rebuild only when deliberately
replacing those results. API quota errors are distinguished from transient rate limits.

With the native OpenAI Responses connection, select **OpenAI Batch** after passing the
Mood tagging gates. The configured model must support Batch. With the no-tag guard on,
submit a single-request pilot (up to 20 tracks). Larger asynchronous plans require a
deliberate guard override after review; submitted work cannot be unspent. The same metadata, vocabulary
and optional local context are uploaded as JSONL; no audio, titles or paths are uploaded.
Completion can take 24 hours. There are no automatic corrective calls or resubmissions.
The server checks every five minutes, including after restart, and stores valid results
for ordinary Mood Library review. Keep the server/database and configured credential key.

Input files expire after seven days; output files may remain up to thirty days. The app
attempts to delete known input/output/error files after results are saved. Cancellation
can take time and completed work remains chargeable. A failed deletion leaves a recoverable
pending record. Do not remove provider files manually before collecting them.

If submission times out after the provider may have accepted it, the app blocks another
run. Find the provider batch whose metadata contains the displayed `music_run_id`, then
paste its batch ID into the recovery field. The server verifies ownership before collecting
or cancelling it. If the submission cannot be found, resolve it in the provider account
before explicitly abandoning the local record; abandoning cannot cancel unknown remote work.
Connection/model/credential changes remain blocked while the record is pending.

This update separates generated-result identity from model certification. Operational
recertification, timeout and credential changes do not alone invalidate saved suggestions.
Changes to the actual inference contract, model/Thinking/output allowance, vocabulary or
input evidence do. New inference always requires current conformance and quality passes.
The September 8 task change makes older AI profiles outdated and requires fresh tagging
checks before new inference. Existing `local-context/v2` analysis is reusable: this rework
does not require another algorithmic/voice pass. Accepted/manual tags remain intact and
nothing runs automatically. Start with existing explanations, then a small selected sample.
The updated quality report has an independent acoustic-context gate; a synthetic pass is
not a claim of musical accuracy. See the [offline listening pilot](docs/MOOD_PILOT.md).

### Mood-tag cleanup

This optional legacy helper is not part of the tagging pipeline. If you need it,
configure its shared model in **Mood tagging**, then run cleanup's own conformance and
quality checks. Sharing a model does not transfer a pass between tasks. Existing independent
cleanup assignments remain inactive until Music tagging is saved to link them. Cleanup is
an optional operation on unresolved manual tag names, not a second pass over tagging output.

1. Open **Assistant -> Mood library -> Mood vocabulary** and review local conservative cleanup there.
   Declared aliases, spelling, and plural rules run
   before its provider boundary and does not spend a provider request when they resolve
   every candidate.
2. Run model cleanup only after reviewing its disclosure: it receives unresolved source
   IDs/names and usage counts plus canonical ID definitions, not songs or generated
   analysis. It must return one canonical-ID-or-null decision for every source. Confirm
   each proposal labels its origin as local rule or model.
3. Select individual proposed renames. Confirm unselected items remain unchanged and a
   stale proposal is rejected rather than guessed or partially repaired.

### EQ assistance

1. Request a conservative test preset for familiar speakers or headphones. The server
   creates a deterministic baseline and narrow safety envelope before the model sees the
   goal; the model refines that baseline rather than inventing an unrestricted curve.
2. Confirm the draft contains the fixed ten frequencies and every gain stays inside
   the locally displayed envelope in 0.5 dB steps (and always inside the global
   -12 to +12 dB preset bounds).
3. Read the rationale and cautions, inspect the curve, and preview Authoring import.
4. Explicitly create the preset, audition it at a safe level, and fine-tune it in normal
   Authoring. The model does not receive audio, songs, existing presets, or library data.

## 7. Optional credential recovery checks

This is sensible before relying on saved provider credentials in production, but it is
not required for every deployment and is irrelevant when no provider key is stored.

Run the read-only credential audit inside the deployed application environment:

```console
music-cli assistant-credentials check
```

For Docker, this is normally:

```console
docker exec music music-cli assistant-credentials check
```

Follow the deployment stack's own runbook for file paths and the safe
one-off-container rotation sequence.

The command must report zero unreadable credentials. It prints only counts and a short
one-way key ID. A periodic recovery test can use an isolated restore:

1. Copy a database backup to a non-production location.
2. Point `DATABASE_URL` at that copy and provide the matching key through
   `ASSISTANT_CREDENTIAL_KEY` or an isolated `ASSISTANT_CREDENTIAL_KEY_FILE`.
3. Run the same check and require zero unreadable credentials.
4. Do not start two Music servers against the same SQLite database.

Only when deliberately rotating the master key, generate a new key and expose it
temporarily as `ASSISTANT_CREDENTIAL_KEY_NEW`. Run the dry run, stop every server
using the database, then apply:

```console
music-cli assistant-credentials rotate
music-cli assistant-credentials rotate --apply --server-stopped
```

Replace the deployment's current environment value or key-file contents with the new key
before restart. Database re-encryption is atomic, but replacing the external key is a
separate operator step and the server must remain stopped between them. Rotation
intentionally clears connection verification, role conformance, and quality gates;
repeat sections 4 and 5 afterward.

## 8. Final acceptance checklist

This project slice is operationally complete when all applicable statements are true:

- Existing playback, devices, Authoring, Library, modes, and imports still work.
- Local analysis can finish and restore progress after page refresh/reopen.
- Manual and generated tags remain visibly separate and review-controlled.
- Local playlist suggestions can be auditioned, selected, previewed, and imported.
- Automatic playlists preview before writes, refresh from allowed local evidence, keep
  normal playback rows, and retain songs when made manual.
- Every enabled provider connection verifies with a saved credential, and every enabled
  role has current conformance and quality passes.
- Live model jobs show disclosure, require explicit consent, survive browser refresh, and
  never write authored state without review.
- If provider credentials are stored, `assistant-credentials check` reports zero
  unreadable credentials and the matching master key is retained securely.
- A file-backed test connection can be deleted and secure storage reset/reinitialized
  through AI Setup without SSH; connection and role drafts remain disabled until retested.
- Provider dashboards have appropriate rate/spending limits and no unexpected requests.

If no provider models are wanted, sections 3-7 are optional; the local baseline and
automatic playlists are still a complete supported workflow.

## Deliberately deferred

- A provider adapter and consent/quality contract for sending bounded audio to specialized
  audio models.
- Additional metadata providers and specialist OCR/audio enrichment after a labeled coverage pilot.
- Provider-independent monetary cost estimates or hard budgets inside Music.
- Any new export workflow beyond the existing Authoring/import and playlist interfaces.

These items should not be enabled by merely unlocking their role in the UI. Each needs a
separate data-minimization contract, tests, failure policy, and explicit review boundary.

## Library cleanup and metadata evidence

Open **Assistant → Library cleanup → Run**. Renaming collisions propose an unchecked numbered
suffix while leaving embedded titles unchanged. Enable catalog evidence to identify recordings,
review competing local/catalog values and choose an album edition for each folder. Matching a
recording does not establish an edition or duplicate audio. **All unambiguous** leaves competing
values unticked; choosing a value unticks its alternative for the same field.

**Evidence and alternatives** shows embedded tag observations, full dates, MusicBrainz credits,
recording candidates, sources and retrieval time. **Refresh catalog results** includes previously
unmatched tracks. Catalog genres are embedded-tag proposals; Last.fm mood suggestions remain
database tags. Every file/tag/folder change still requires Apply and appears in History & rollback.

You can select a JSON metadata sidecar with this shape (the example ID is illustrative):

```json
{"tracks":[{"track_id":7,"fields":{"recording_mbid":"00000000-0000-0000-0000-000000000001","date":"2026-09-10"}}]}
```

Track IDs appear in each review evidence panel. Supported fields are `title`, `artist`,
`album_artist`, `album`, `track_no`, `disc_no`, `date`, `original_date`, `genre`, `recording_mbid`,
`release_mbid`, `release_track_mbid`, `release_group_mbid`, `isrc`, `barcode` and `catalog_number`.
Use text values, at most 512 bytes each and 500 unique tracks in the selected scope. Imported
observations support lookup; full dates/IDs are preserved as evidence rather than automatically
written into extended embedded tags. CUE splitting, executable sidecars and arbitrary URL scraping
are not part of this import.

For ambiguous catalog results, configure **Library cleanup** in **AI setup**, verify its
connection, pass conformance and all eight synthetic quality cases, and enable the role. On an
unresolved track, choose **Review ambiguous candidates with AI** and review the disclosure.
Explicit consent permits one request, which may incur provider charges. Only bounded indexed
metadata, supplied candidates and comparison facts are sent. The model can abstain or recommend
one candidate. Any resulting title/artist proposals remain unchecked until you select and apply
them. Evidence older than six hours requires a fresh catalog lookup before AI review. There is no
automatic retry after an uncertain paid request. Real-library accuracy and
additional recognizer/provider coverage require a separate labeled pilot.
