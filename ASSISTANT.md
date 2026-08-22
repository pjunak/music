# Assistant setup and acceptance guide

This is the operator guide for finishing and validating the local-first Assistant,
optional model connections, review-first authoring, and automatic playlists. The
local playlist and library tools work without any provider key. Model-backed tools
are optional and fail closed when their setup or quality checks are incomplete.
Only section 1 is relevant to deploying the code; the remaining sections are
first-time setup and functional checks, not release prerequisites.

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

1. Make a copy of `app.db`. This is the only application data changed by the
   additive schema update in this release.
2. If provider credentials are already saved, confirm that the matching Assistant
   credential master key is still available through `ASSISTANT_CREDENTIAL_KEY` or the
   dedicated key-file mount. A database backup and this key are one restore set.
3. Confirm the deployment still mounts the existing music, SFX, modes, and device
   paths in their usual locations. These features do not migrate or rewrite those
   files, so a new full media backup is not required for this release.
4. Build and deploy the current `main` revision through the normal CI and
   infrastructure workflow. Do not copy a development database over production.
5. After startup, sign in and confirm that normal playback, output-device selection,
   Authoring, and the Library still work before enabling optional models.

The schema changes are additive and applied at startup. Music and SFX should still be
covered by the server's normal long-term backup policy because they are valuable source
data, not because this release introduces a special risk to them.

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
python -c "import base64,secrets; print(base64.urlsafe_b64encode(secrets.token_bytes(32)).decode())"
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

1. Choose `openai-compatible/v1` for the widest provider compatibility. If the
   provider explicitly documents OpenAI-style `response_format` with
   `type: json_schema`, you may instead choose
   `openai-compatible-json-schema/v1` for API-enforced strict output. Do not choose
   the strict adapter merely because the endpoint is otherwise OpenAI-compatible;
   the role conformance test is the authoritative check.
2. Enter a clear local name, the provider's documented API base URL, and its API key.
3. Leave private-network access off for public providers. Enable it only for a service
   you intentionally run on a trusted private address.
4. Save the connection. Confirm the UI says a credential is saved and shows only a
   masked hint.
5. A saved API key is write-once. To use another key, explicitly delete the current one
   from the connection and then enter the replacement.
6. Click **Verify connection**. Verification lists models and confirms the adapter's
   structured-text capability; it does not send library data.
7. If verification fails, correct the base URL, credential, TLS, or provider access.
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
3. Confirm model output appears as generated `model-evidence-tagger/v3` suggestions,
   separate from local analysis and manual tags.
4. Inspect the disclosure: the model receives a path-free deterministic metadata
   hypothesis with each candidate tag's matched field and term. A non-empty display
   title is used as the canonical title for this matching. When available, the model
   also receives bounded local signal axes/activity/dynamics/rhythm.
   These remain evidence, not automatic tags.
5. Accept, reject, and reopen several suggestions. Only acceptance may add a manual tag.
6. Confirm automatic playlists do not react to pending or rejected model suggestions;
   they may react after an accepted suggestion becomes a manual tag.

### Manual-tag cleanup

1. Review local conservative cleanup first. The combined harness also runs these rules
   before its provider boundary and does not spend a provider request when they resolve
   every candidate.
2. Run model cleanup only after reviewing its disclosure: it receives unresolved source
   tags, allowed target tags and usage counts, not songs or generated analysis. Confirm
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
- Model-assisted file/library cleanup on top of the existing review-first local cleanup.
- Provider-independent monetary cost estimates or hard budgets inside Music.
- Any new export workflow beyond the existing Authoring/import and playlist interfaces.

These items should not be enabled by merely unlocking their role in the UI. Each needs a
separate data-minimization contract, tests, failure policy, and explicit review boundary.
