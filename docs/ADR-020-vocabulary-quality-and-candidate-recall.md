# ADR-020: Vocabulary quality gates and playlist candidate recall

**Status:** Accepted

**Date:** 2026-09-05

**Scope:** Approved quality-audit implementation, fourth batch

## Problem

The tagging quality job always supplied the bundled vocabulary. It could certify
a model without exercising custom definitions, aliases, non-English names, or the
supported 200-tag limit. Adding cases to a single aggregate could also dilute the
existing baseline. Playlist evaluation scored final results without identifying
relevant tracks omitted during local candidate preparation.

## Decision

Tagging suite v21 contains the unchanged 50 bundled-vocabulary scenarios, five
custom-vocabulary scenarios, and one 200-tag scenario. Fixed, synthetic vocabulary
fixtures cover aliases, a familiar label with a deliberately different definition,
a Unicode name, abstention, injection resistance, and a required label at the end
of the maximum vocabulary. Operator vocabulary and library data are never loaded
for these checks.

Each case records its vocabulary identity. The application batch planner preserves
case order and partitions contiguous cases by vocabulary before using the same
20-track, exact-envelope planner as live tagging. Safety reruns and failed-case
retests use this path too. The request budget counts all resulting batches plus
the existing two shared correction attempts. Vocabulary entries are never dropped
to fit a request. Every registered adapter is tested with 20 tracks and all 200
tags in both ordinary and corrective requests; existing oversized-envelope tests
continue to reject an excessive single-track payload before execution.

The aggregate 90% threshold and blocking failures remain. Each vocabulary group
must independently meet the same threshold and have no blocking failure. With
five custom cases and one maximum-size case, those groups currently require all
their cases to pass. New easy cases cannot compensate for failures in the original
50-case baseline. All ten safety cases are repeated once. Only a complete run can
certify; diagnostic retests must match the current case and vocabulary identities.

Suite loading validates expected labels, group names, confidence choices, tag
bounds, and production input validity. A synthetic expected-output regression
round-trips all cases through the real result validator and scorer. This proves
fixture and handler agreement, not semantic quality of a provider model.

Playlist model evaluation records candidate pool size, labelled relevant tracks,
relevant tracks present, recall, and missing IDs. It derives the pool through the
same deterministic task constructor used to prepare that fixed evaluation case.
The diagnostics survive provider errors. Local-engine reports omit model-pool
metrics. The console distinguishes missing input candidates from ranking errors;
existing playlist ranking and certification thresholds are unchanged.

## Consequences and limits

- Runtime source/suite fingerprints invalidate affected saved conformance and
  quality results. Full evaluation still uses the operator's chosen model and
  Thinking setting. No live call is implied by these automated regressions.
- Reports identify vocabulary cohorts and explain independent failures. Historic
  reports remain readable, but stale results cannot be used for current retests.
- Candidate recall measures the fixed labelled fixtures, not an unlabelled live
  library. Retrieval policy is unchanged. It can now be assessed separately from
  model ranking before deciding how to improve it.
- Six additional synthetic tagging cases are boundary coverage, not a replacement
  for the 100–200-example held-out study, operator review metrics, or repeated
  measurements of the chosen provider configuration.
- There are no database, public HTTP/WebSocket DTO, or external-client changes.
  Evaluation results remain versioned documents inside the existing job envelope.

See [AI acceptance](AI_ACCEPTANCE.md) for the remaining operational and human
validation, and [the architecture map](ASSISTANT_ARCHITECTURE.md) for ownership.

## Fifth-batch amendment: cleanup vocabularies

Cleanup suite `controlled-vocabulary-cleanup-baseline-v7` retains its 15 baseline
cases and adds four custom-vocabulary cases plus a full 20-source case using all
200 canonical tags. Custom cases cover local aliases (including a Unicode target),
definitions that differ from bundled meanings, preservation of compound meanings,
and injection resistance. The existing all-cases-must-pass gate is unchanged;
cohort summaries are diagnostic. Empty, missing, reordered, or mismatched case
results cannot certify a model.

Each cleanup case constructs its task and budget from its own fixed vocabulary.
Suite loading validates required source/target pairs and production input limits.
Expected-output fixtures exercise deterministic aliases, actual model-request
decisions, strict result handling, and final scoring. They verify harness integrity,
not live semantic quality. Shared vocabulary fixtures affect both tagging and
cleanup runtime fingerprints. Operator review metrics remain separate follow-up work.
