# Job scheduling and timing diagnostics

Run this against an explicit existing application database or an isolated copy:

```sh
music-cli jobs timing --database /data/app.db --limit 1000 --json
```

Omit `--json` for a readable summary. The command opens one read-only SQLite
connection and reads one consistent snapshot. It can run while the server owns the
database. It does not initialize or migrate storage, acquire the server's writer
lock, load deployment configuration, start workers, or contact any provider. Normal
SQLite access to the database and any active WAL sidecars is still required.
Missing databases and incompatible job tables fail without creating or upgrading
them. Do not copy a live SQLite file without its WAL; use the application's backup
workflow when an isolated consistent copy is needed.

## What the report measures

The version 1 JSON report groups jobs by lane and registered job kind. It reads no
parameters, results, prompts, credentials, job IDs, errors, or progress prose.

| Field | Meaning |
|---|---|
| `sample_limit` / `sampled_jobs` | Requested bound and actual sample size; default 1,000, allowed 1–10,000 across both lanes |
| `has_more` | Older jobs exist outside this newest-created sample |
| `timestamp_resolution_seconds` | Persisted timestamps currently resolve to one second |
| `statuses` | Job counts for each observed state, including queued and running jobs |
| `restarted_jobs` | Jobs claimed more than once; their timing is unavailable because attempt history is incomplete |
| `queue_wait_seconds` | Creation to first claim for jobs with exactly one attempt |
| `execution_seconds` | Claim to terminal completion for jobs with exactly one attempt |
| `measured` / `unavailable` | Number of usable and excluded observations for that duration |
| `p50` / `p95` / `max` | Nearest-rank percentiles and maximum of usable observations, in seconds; null when none exist |

Queue wait is unavailable for jobs that have not started. Execution time is
unavailable for unfinished jobs. Missing, unparseable, pre-epoch or backwards
timestamps produce unavailable durations, not zeros. A valid same-second interval
can be zero at this precision. Clock jumps that still produce ordered timestamps
cannot be distinguished from elapsed time.

Execution duration covers the **whole job**, including local preparation,
checkpointing, and possibly many requests. Completed provider-request durations
remain in model usage v2; they are a different measurement. Restarted local or
catalog jobs retain only their latest attempt timestamps, so the report excludes
both durations for those jobs. An explicit retry creates a new job and has its own
queue wait.

This is a bounded diagnostic sample, not a monitoring system or a model-quality
score. Newest-created sampling can omit old waiting jobs. Completed-duration
percentiles exclude unfinished work and therefore can look good while a queue is
stalled. Inspect state counts and unavailable counts, wait for the workload to
drain, and use an adequate sample before drawing conclusions. Repeated reports
overlap; do not sum their counts as independent observations.

## Current scheduling contract

The application owns one local worker and one provider worker. Each worker awaits
the entire claimed job before claiming another. Model tasks, model evaluations,
and catalog enrichment share the provider lane. A long evaluation or catalog run
therefore delays short drafts. A checkpoint preserves recovery/cancellation state;
it does not yield that lane to another job. The transport's separate four-request
admission limit also covers connection checks and conformance, and does not create
four provider job workers.

Within a lane, queued jobs sort by creation time and then SQLite insertion order.
This repairs random UUID ordering when several jobs arrive in one second. History
and active-job lookup use the corresponding newest-first order. Recovery preserves
the original row's place; an explicit retry inserts a new row. Cancellation skips
queued work without reserving a turn.

Insertion order is a local tie-breaker, not an externally stable job identity.
SQLite can renumber implicit rowids during offline rebuilds or `VACUUM`; no original
tie-order guarantee is made across those operations or clock rollback. Job IDs and
leases remain explicit UUIDs. If future scheduling requires a durable sequence
across arbitrary imports/rebuilds, add a migrated explicit sequence column rather
than exposing rowids. See SQLite's [rowid documentation](https://www.sqlite.org/rowidtable.html).

## Evidence and next scheduling decision

The SQLite-backed scheduling regression holds a provider job at three successive
checkpoints. A local job completes while a short provider job remains queued at
every checkpoint; releasing the final step lets the short job finish once. The
test uses gates instead of machine-speed timing thresholds and makes no external
requests. Separate tests cover equal creation timestamps with deliberately reversed
IDs, cancellation, database reopen, and read-only inspection during writer ownership.

For a representative study, record workload sizes and arrivals, take bounded timing
reports before and after the workload drains, and compare short drafts with bulk
tagging, catalog work, and quality evaluations. Include per-request usage durations,
unavailable observations, error/cancellation counts, and sample coverage. Provider
and live-library runs still require their existing disclosure and consent.

If short-job queue delay exceeds the operator's agreed target, prefer cooperative
scheduling at completed-request boundaries: retain each job's lease and request
budget, admit a bounded number of jobs, rotate ready requests, and include aging so
bulk work progresses. Keep in-flight calls bounded and record each attempt before
dispatch. Test cancellation, role changes, shutdown, uncertain completion, and
non-restartable paid work before rollout. Simply spawning more whole-job workers
would change cost/concurrency behavior without establishing fairness. No such
scheduling policy change or production latency improvement is claimed here.
