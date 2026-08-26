# Documentation map

Start here when changing behavior or trying to find the current contract. This page is an index,
not a second description of the implementation.

## By task

- Product, deployment, configuration, and development: [root README](../README.md)
- Optional model setup and operator acceptance: [Assistant setup guide](../ASSISTANT.md)
- Assistant code, contracts, privacy boundaries, tests, and change procedure:
  [Assistant architecture and contract map](ASSISTANT_ARCHITECTURE.md)
- External output protocol: [client guide](../clients/README.md)
- Versioned Authoring import document: [authoring import v1](../clients/authoring-import-v1.md)
- Active work deliberately left for later: [backlog](../TODO.md)
- Assistant/Authoring interaction rules: [Assistant UX philosophy](assistant-ux-philosophy.md)

## Architecture decisions

ADRs explain why a durable decision was made. The living Assistant contract map above identifies
the current version strings and source files; older version strings inside an ADR describe the
decision when it was accepted unless an amendment says otherwise.

- [ADR-001: Provider connections and model roles](ADR-001-assistant-provider-connections.md)
- [ADR-002: Bounded model execution and conformance](ADR-002-assistant-model-execution.md)
- [ADR-003: Hybrid playlist evaluation](ADR-003-hybrid-model-playlist-evaluation.md)
- [ADR-004: Durable model quality gates](ADR-004-durable-model-quality-gates.md)
- [ADR-005: Consent-bound model playlist suggestions](ADR-005-consent-bound-model-playlist-suggestions.md)
- [ADR-006: Review-only model music tagging](ADR-006-review-only-model-music-tagging.md)
- [ADR-007: Algorithm-first structured model harness](ADR-007-algorithm-first-structured-model-harness.md)
- [ADR-008: Comprehensive local track context](ADR-008-comprehensive-local-track-context.md)
- [ADR-009: Opt-in local voice analysis](ADR-009-opt-in-local-voice-analysis.md)
- [ADR-010: Parallel library context analysis](ADR-010-parallel-library-context-analysis.md)
- [ADR-011: In-process provider adapter handlers](ADR-011-in-process-provider-adapter-handlers.md)
- [ADR-012: Native context analysis and workload profiling](ADR-012-native-context-analysis-and-profiling.md)
- [ADR-013: Python 3.14 runtime and concurrency](ADR-013-python-314-runtime-and-concurrency.md)
- [ADR-014: Perceptual local-context measurements](ADR-014-perceptual-context-measurements.md)

## Maintenance rules

1. Update the owning code model or protocol first, its tests second, and the linked living guide in
   the same change. Do not copy a contract into multiple overview documents.
2. Record a new durable trade-off as an ADR. Amend an existing ADR when implementation evolves
   without reversing its decision; do not rewrite history silently.
3. Keep operator steps in `ASSISTANT.md`, current code ownership and version inventory in
   `ASSISTANT_ARCHITECTURE.md`, and postponed work in `TODO.md` or `FUTURE.md`.
4. The backend documentation tests verify local Markdown links, this index, and the Assistant
   contract inventory. A contract-version change is incomplete until those tests pass.
5. Delete completed working plans from source control. Private ignored notes are not maintained
   project documentation and must not be treated as current contracts.
