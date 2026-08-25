# ADR-004: Durable, fingerprint-bound model quality gates

**Status:** Accepted

**Date:** 2026-08-19
**Decider:** Project owner

**Implementation follow-up (2026-08-20):** The same fingerprint-bound quality
framework now certifies playlist planning, music tagging, manual-tag cleanup, and
EQ drafting. Passing results are consumed only by each feature's own consent and
review contract.

## Context

The configured playlist model can already be tested for basic structured-output
conformance and evaluated from the command line. Conformance proves that the
provider endpoint can follow one small protocol challenge; it does not prove
that the selected model can rank and sequence representative playlists safely.
Running the larger suite in an HTTP request would also leave the browser waiting
and lose visible progress when the page closes or refreshes.

This quality result must belong to the exact connection, model, timeout, and
response-limit configuration that was tested. A result from an older runtime
must never authorize a changed model.

## Decision

- Run each task-specific model quality suite through the existing durable
  background-job runner. The job row owns lifecycle, progress, cancellation,
  history, and the complete safe synthetic report.
- Store the current certification separately in
  `assistant_model_evaluations`, keyed by model role and evaluation ID. This
  table stores only the runtime fingerprint, pass/fail state, case counts,
  suite and engine IDs, source job ID, and evaluation time.
- Enqueue only role and evaluation IDs plus the non-secret runtime fingerprint.
  Resolve the encrypted credential inside the worker and reject the run if the
  role no longer matches before execution or before saving the result.
- Clear current certifications when a connection is reverified or role runtime
  settings change. Historical jobs remain available for diagnosis.
- Use the checked-in `playlist-local-v1.json` suite for
  `playlist-quality-v1`. The suite is synthetic. The model still receives only
  the locally filtered, path-free candidate contract and returns IDs only.
- Make model evaluation jobs non-restartable. A server restart marks an
  interrupted evaluation failed; the operator can deliberately retry it.
  Browser refresh and reopening remain supported because job progress is
  server-owned and persistent.
- Treat a completed but failing quality report as a successful job with a
  failed certification. Job failure is reserved for infrastructure,
  configuration, cancellation, or execution-lifecycle failure.
- Let a task-specific suite distinguish blocking safety/contract failures from
  scored semantic quality where that distinction is meaningful. Mood tagging
  repeats every safety scenario once and rejects any forbidden false positive or
  provider/contract failure from either attempt. A safety label does not make a
  required semantic-tag miss blocking: all scenarios contribute to the suite's
  explicit minimum recall pass rate. This catches unstable dangerous output without
  turning a larger nondeterministic semantic suite into an accidental all-or-nothing
  gate.
- Permit a bounded mood-tagging failed-scenario recheck only against the exact
  current complete result. Run only its failed case IDs, merge replacements with
  the saved complete case set, and show the recomputed report for diagnosis.
  Never update certification from a selective rerun: only a new complete suite
  can change the gate. This prevents repeated sampling from gradually replacing
  one coherent evaluation with only favorable attempts.
- Do not treat a passing result as standing authorization for a live request.
  ADR-005 adds the playlist-specific consent, disclosure, bounded data, fallback,
  and review-to-import integration; other roles apply equivalent task-specific
  contracts.

## Options considered

### Keep quality fields directly on each role

This is smaller initially but mixes task configuration with one particular
suite and does not extend cleanly to multiple evaluations for tagging, cleanup,
EQ, or audio roles. Rejected.

### Store only background-job results

This preserves history but makes every readiness check search and interpret
arbitrary job JSON. It also weakens the exact-runtime gate. Rejected.

### Restart interrupted evaluations automatically

This improves unattended completion but can repeat provider calls and cost
after an uncertain interruption. Rejected until model calls have explicit
checkpoint or idempotency semantics.

### Durable job history plus a small current-certification table

Separates lifecycle from readiness, supports refresh-safe progress, and leaves
room for multiple role-specific suites. Selected.

## Consequences

- The UI can restore an active or completed run after navigation, refresh, or
  reopening without holding the original HTTP request open.
- Changing a model's runtime configuration requires conformance and quality to
  be run again. This is deliberate and visible.
- An interrupted server-side run may have incurred provider cost even though it
  cannot certify the model; retry remains an explicit operator action.
- A passed synthetic suite is necessary but not sufficient evidence for live
  library use. It does not prove subjective quality for every private library.
