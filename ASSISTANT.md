# Assistant setup and acceptance runbook

This is the operator guide for finishing and validating the local-first Assistant,
optional model connections, review-first authoring, and automatic playlists. The
local playlist and library tools work without any provider key. Model-backed tools
are optional and fail closed when their setup or quality checks are incomplete.

## What is ready

- Local metadata and server-side audio-signal analysis run as durable jobs.
- Manual D&D-oriented tags remain separate from generated suggestions.
- The local playlist planner creates a reviewable draft and is the default.
- Optional provider models can assist playlist planning, music tagging, manual-tag
  cleanup, and ten-band EQ drafting.
- Every model task has a separate role. Roles may share one connection and key or
  use different connections, providers, models, and keys.
- Playlist and EQ drafts go through Authoring import preview and explicit commit.
- Model tag suggestions go through explicit generated-tag review.
- Automatic playlists use only manual/accepted tags and optional current local
  metadata analysis. They never consume unreviewed provider suggestions.

Specialized models that receive audio and model-assisted file cleanup are not ready.
Their role names are visible in AI Setup but deliberately locked until they have a
provider adapter, data limit, disclosure, quality suite, and review contract.

## 1. Prepare the deployment

1. Back up `app.db`, `devices.json`, the modes directory, and the music/SFX
   directories before deploying a new build.
2. If provider credentials already exist, also back up the deployment secret named
   `ASSISTANT_CREDENTIAL_KEY`. The database and this key are one restore set.
3. Build and deploy the current `main` revision through the normal CI and
   infrastructure workflow. Do not copy a development database over production.
4. After startup, sign in and confirm that normal playback, output-device selection,
   Authoring, and the Library still work before enabling optional models.

The schema changes in this feature are additive and are applied at startup. Keep the
pre-deployment database backup until the acceptance checks below pass.

## 2. Establish the local baseline first

1. Open **Assistant -> Library Analysis**.
2. Run local metadata analysis. The job is server-owned, so the page may be closed
   and reopened without losing progress.
3. Optionally run local audio analysis. It decodes audio on the server and stores
   bounded numeric evidence; it does not send audio anywhere or invent semantic tags.
4. Review generated tags and accept only the useful ones. Add or edit manual tags such
   as `medieval`, `tavern`, `dancing`, `combat`, `travel`, and custom campaign terms.
5. Open **Assistant -> Playlist Builder**, create at least one local suggestion, audition
   several songs, adjust the final selection, preview the Authoring import, and create
   a test playlist.
6. Create or choose a normal playlist, configure an automatic local rule, review its
   exact matching songs, and enable it. Change a relevant accepted tag and confirm the
   playlist refreshes when opened or played. Switch it back to manual and confirm its
   current songs remain.

Do not continue to provider setup until this local path is satisfactory. It remains the
privacy-preserving fallback and provides the tags and bounded candidates used by model
planning.

## 3. Enable encrypted provider credentials

The server needs one deployment-owned 32-byte master key before it can save provider
API keys. Generate a URL-safe base64 key outside the repository:

```powershell
python -c "import base64,secrets; print(base64.urlsafe_b64encode(secrets.token_bytes(32)).decode())"
```

1. Save the result as `ASSISTANT_CREDENTIAL_KEY` in the deployment's secret store.
2. Restart the Music server.
3. Open **Assistant -> AI Setup** and confirm credential storage is ready.
4. Keep the master key out of `.env` files that are copied, logs, screenshots, source
   control, and provider-connection names.

The key encrypts provider credentials in `app.db`; it is not a provider API key. Losing
it does not expose a provider key, but it makes every saved credential unreadable and
requires those credentials to be entered again.

## 4. Add and verify provider connections

Create one connection for each credential or trust boundary you want. Reusing one key
is allowed, but separate connections make rotation, revocation, provider limits, and
task ownership clearer.

For each connection:

1. Choose the `openai-compatible/v1` adapter.
2. Enter a clear local name, the provider's documented API base URL, and its API key.
3. Leave private-network access off for public providers. Enable it only for a service
   you intentionally run on a trusted private address.
4. Save the connection. Confirm the UI says a credential is saved and shows only a
   masked hint.
5. Click **Verify connection**. Verification lists models and confirms the adapter's
   structured-text capability; it does not send library data.
6. If verification fails, correct the base URL, credential, TLS, or provider access.
   Do not work around a failure by enabling private-network access for a public host.

Saving and verification are separate by design. A saved credential alone cannot run a
model task.

## 5. Configure each model role

Repeat this sequence for playlist planning, music tagging, song-tag cleanup, and EQ
assistance. The same connection/model may be selected for all four, or each role may use
a specialized model and separate key.

1. Select a verified connection and one of its reported model IDs.
2. Keep the role disabled while saving its initial configuration.
3. Run the role's fixed conformance test. This makes one small provider request and
   checks strict structured output for that exact connection, model, timeout, and output
   limit.
4. Enable the role only after conformance passes.
5. Run the task-specific synthetic quality check and wait for its durable job to finish.
6. Review the report. A pass certifies only that exact runtime fingerprint; changing or
   reverifying the connection, replacing/removing its key, or changing the model/runtime
   settings requires conformance and quality to run again.

The four checks are intentionally independent. A model that is good at playlist ordering
may be poor at conservative EQ or metadata tagging. Provider-side spending limits remain
the authoritative cost guard; Music records reported token usage but does not estimate
portable monetary cost.

## 6. Validate model-backed workflows with real data

Use a small, representative sample before running across the whole library.

### Playlist planning

1. In Playlist Builder, run the same request once with the local planner and once with
   the configured model.
2. Read the disclosure before consenting. The model receives at most 100 locally
   eligible, path-free candidates and returns track IDs only.
3. Close or refresh the page during one run and confirm progress/result restoration.
4. Audition suggestions one at a time. Starting another song or normal playback must
   stop the previous audition through the shared canonical playback state.
5. Adjust the selection, preview Authoring import, and explicitly create the playlist.
6. Confirm a failed model request remains visibly failed and does not silently replace
   its provenance with a local result.

### Music tagging

1. Review the disclosed counts and estimated provider requests in Library Analysis.
2. Start with a small library/sample if provider cost or output quality is uncertain.
3. Confirm model output appears as generated `model-evidence-tagger/v2` suggestions,
   separate from local analysis and manual tags.
4. Accept, reject, and reopen several suggestions. Only acceptance may add a manual tag.
5. Confirm automatic playlists do not react to pending or rejected model suggestions;
   they may react after an accepted suggestion becomes a manual tag.

### Manual-tag cleanup

1. Run local conservative cleanup first.
2. Run model cleanup only after reviewing its disclosure: it receives normalized manual
   tag names and usage counts, not songs or generated analysis.
3. Select individual proposed renames. Confirm unselected items remain unchanged and a
   stale proposal is rejected rather than guessed or partially repaired.

### EQ assistance

1. Request a conservative test preset for familiar speakers or headphones.
2. Confirm the draft contains the fixed ten frequencies and gains only from -12 to
   +12 dB in 0.5 dB steps.
3. Read the rationale and cautions, inspect the curve, and preview Authoring import.
4. Explicitly create the preset, audition it at a safe level, and fine-tune it in normal
   Authoring. The model does not receive audio, songs, existing presets, or library data.

## 7. Prove backup and credential recovery

Run the read-only credential audit in the deployed environment:

```powershell
music-cli assistant-credentials check
```

The command must report zero unreadable credentials. It prints only counts and a short
one-way key ID. Then test an isolated restore:

1. Copy a database backup to a non-production location.
2. Point `DATABASE_URL` at that copy and set the matching
   `ASSISTANT_CREDENTIAL_KEY` only in the isolated process.
3. Run the same check and require zero unreadable credentials.
4. Do not start two Music servers against the same SQLite database.

For planned master-key rotation, generate a new key, expose it temporarily as
`ASSISTANT_CREDENTIAL_KEY_NEW`, run the dry run, stop every server using the database,
then apply:

```powershell
music-cli assistant-credentials rotate
music-cli assistant-credentials rotate --apply --server-stopped
```

Replace the deployment's current key with the new key before restart. Rotation is atomic
but intentionally clears connection verification, role conformance, and quality gates;
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
- A restored database/key pair passes `assistant-credentials check` with zero unreadable
  credentials.
- Provider dashboards have appropriate rate/spending limits and no unexpected requests.
- The deployment backup, rollback route, and matching credential master key are documented
  outside the repository.

If no provider models are wanted, sections 3-7 are optional; the local baseline and
automatic playlists are still a complete supported workflow.

## Deliberately deferred

- A provider adapter and consent/quality contract for sending bounded audio to specialized
  audio models.
- Model-assisted file/library cleanup on top of the existing review-first local cleanup.
- Provider-independent monetary cost estimates or hard budgets inside Music.
- Any new export workflow beyond the existing Authoring/import and playlist interfaces.

These items should not be enabled by merely unlocking their role in the UI. Each needs a
separate data-minimization contract, tests, failure policy, and explicit review boundary.
