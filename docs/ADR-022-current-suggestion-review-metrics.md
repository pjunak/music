# ADR-022: Current suggestion review metrics

**Status:** Accepted

**Date:** 2026-09-05

## Decision

Report pending, accepted, and rejected current tag suggestions by analyzer in the
Mood Library. Reuse the application review projection and its freshness, shape,
normalization, and source-signature checks. Manual tags do not imply acceptance;
reopening returns a suggestion to pending while preserving authored tags.

The unit is a unique `(track, analyzer, current source signature, normalized tag)`
suggestion. The same tag proposed by two analyzers counts once under each source.
Rejected suggestions stay in the denominator. Stale or malformed proposals do not.
An analyzer with no current suggestions contributes no row.

`AssistantService::tag_page` computes the summary after folder/track scope, search,
manual-tag and analyzer filters, but before review-state filtering and pagination.
`matching_tracks` includes tracks without suggestions. The page's existing `total`
still counts tracks after the review-state filter. The summary uses the same loaded
evidence and projection as the page; it adds no provider calls or database writes.

Authenticated GET `/api/assistant/library-tags` and POST
`/api/assistant/library-tags/query` return an additive `review_summary` field:

```json
{
  "matching_tracks": 200,
  "sources": [
    {"analyzer_id": "local-metadata/v1", "pending": 10, "accepted": 4, "rejected": 2}
  ]
}
```

The field is optional in OpenAPI for compatibility with older responses. The new
server always supplies it. The browser omits the summary for absent or invalid
counts and hides old counts while loading or after a failed refresh. Single and
bulk reviews request a fresh server summary; the browser does not infer global
changes from one page. Scope/filter changes and manual-tag edits that change
filtered membership use the existing reload path. Counts refresh on requests and
decisions, with no background polling.

This administrative HTTP endpoint is not consumed by Baton core-model/core-network,
the headless output, or the compatibility player. Their registration, authentication,
reconnect, playback, device, and output contracts do not change. Generated OpenAPI
and its explicit compatibility review record the extension; no unused Baton DTO is
introduced. No database migration or independent metrics store is needed.

## Evidence boundaries

These are current operator decisions, not model accuracy, lifetime history, or
verified training labels. Reviewed count is accepted plus rejected; total also
includes pending. No percentage is needed to describe an empty denominator.
Regeneration, configuration changes, vocabulary changes, and evidence invalidation
may remove prior proposals and their decisions from this view. Analyzer IDs identify
sources, not a longitudinal comparison across model configurations.

Strict synthetic certification and the independently labelled held-out study remain
separate. A future historical study would need retained immutable proposals, review
events and model/run provenance, explicit sampling, and an independently labelled
evaluation design. Current summary counts cannot reconstruct those data.

## Verification

- Application regression: source separation, normalization/deduplication, manual
  tags versus decisions, rejected inclusion, stale and malformed exclusion.
- SQLite/service regression: pagination and review-filter invariance, scope/search/
  manual-tag/analyzer filters, empty results, acceptance/rejection/reopening, and
  changed evidence.
- Real-runtime HTTP regression: model decisions, serialization, vocabulary
  invalidation, out-of-range pages, and authentication.
- Browser regressions: global source counts, zero/invalid/legacy responses, single
  and bulk decision refreshes, failed refresh and retry.
