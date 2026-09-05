//! Payload-free diagnostic summaries of persisted job timing, not provider latency.

use std::collections::BTreeMap;

use serde::Serialize;

use super::{JobLane, JobStatus};

pub const MAX_JOB_TIMING_SAMPLE: u16 = 10_000;
pub const DEFAULT_JOB_TIMING_SAMPLE: u16 = 1_000;

#[derive(Debug)]
pub struct JobTimingSample {
    pub kind: String,
    pub lane: JobLane,
    pub status: JobStatus,
    pub attempts: u32,
    pub created_at: Option<i64>,
    pub started_at: Option<i64>,
    pub finished_at: Option<i64>,
}

#[derive(Debug, Serialize)]
pub struct JobTimingReport {
    pub schema_version: u32,
    pub timestamp_resolution_seconds: u32,
    pub sample_limit: u16,
    pub sampled_jobs: usize,
    pub has_more: bool,
    pub groups: Vec<JobTimingGroup>,
}

#[derive(Debug, Serialize)]
pub struct JobTimingGroup {
    pub lane: JobLane,
    pub kind: String,
    pub jobs: usize,
    pub statuses: BTreeMap<String, usize>,
    pub restarted_jobs: usize,
    pub queue_wait_seconds: DurationSummary,
    pub execution_seconds: DurationSummary,
}

#[derive(Debug, Serialize)]
pub struct DurationSummary {
    pub measured: usize,
    pub unavailable: usize,
    pub p50: Option<u64>,
    pub p95: Option<u64>,
    pub max: Option<u64>,
}

impl DurationSummary {
    fn new(values: impl Iterator<Item = Option<u64>>, total: usize) -> Self {
        let mut values: Vec<_> = values.flatten().collect();
        values.sort_unstable();
        let percentile = |percent: usize| {
            let rank = (values.len() * percent).div_ceil(100);
            rank.checked_sub(1)
                .and_then(|index| values.get(index))
                .copied()
        };
        Self {
            measured: values.len(),
            unavailable: total - values.len(),
            p50: percentile(50),
            p95: percentile(95),
            max: values.last().copied(),
        }
    }
}

fn elapsed(start: Option<i64>, end: Option<i64>) -> Option<u64> {
    let start = start.filter(|value| *value >= 0)?;
    let end = end.filter(|value| *value >= 0)?;
    u64::try_from(end.checked_sub(start)?).ok()
}

/// The storage adapter supplies the bounded newest-created snapshot and its truncation flag.
pub fn summarize_job_timing(
    samples: &[JobTimingSample],
    sample_limit: u16,
    has_more: bool,
) -> JobTimingReport {
    let mut grouped = BTreeMap::<_, Vec<&JobTimingSample>>::new();
    for sample in samples {
        grouped
            .entry((sample.lane, sample.kind.as_str()))
            .or_default()
            .push(sample);
    }
    let groups = grouped
        .into_iter()
        .map(|((lane, kind), samples)| {
            let mut statuses = BTreeMap::new();
            for sample in &samples {
                *statuses
                    .entry(sample.status.as_str().to_owned())
                    .or_default() += 1;
            }
            // Recovery overwrites attempt timestamps. Do not count earlier execution
            // as queue wait, or present the latest attempt as the whole job duration.
            let queue_wait_seconds = DurationSummary::new(
                samples.iter().map(|sample| {
                    if sample.attempts == 1 && sample.status != JobStatus::Queued {
                        elapsed(sample.created_at, sample.started_at)
                    } else {
                        None
                    }
                }),
                samples.len(),
            );
            let execution_seconds = DurationSummary::new(
                samples.iter().map(|sample| {
                    if sample.attempts == 1 && !sample.status.is_active() {
                        elapsed(sample.started_at, sample.finished_at)
                    } else {
                        None
                    }
                }),
                samples.len(),
            );
            JobTimingGroup {
                lane,
                kind: kind.to_owned(),
                jobs: samples.len(),
                statuses,
                restarted_jobs: samples.iter().filter(|sample| sample.attempts > 1).count(),
                queue_wait_seconds,
                execution_seconds,
            }
        })
        .collect();
    JobTimingReport {
        schema_version: 1,
        timestamp_resolution_seconds: 1,
        sample_limit,
        sampled_jobs: samples.len(),
        has_more,
        groups,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample(wait: i64) -> JobTimingSample {
        JobTimingSample {
            kind: "test.short".to_owned(),
            lane: JobLane::Provider,
            status: JobStatus::Succeeded,
            attempts: 1,
            created_at: Some(100),
            started_at: Some(100 + wait),
            finished_at: Some(110 + wait),
        }
    }

    #[test]
    fn timing_percentiles_use_nearest_rank_and_keep_lanes_separate() {
        let mut samples: Vec<_> = (1..=20).map(sample).collect();
        samples[0].lane = JobLane::Local;
        let report = summarize_job_timing(&samples, 20, true);
        assert_eq!(report.sampled_jobs, 20);
        assert!(report.has_more);
        assert_eq!(report.groups.len(), 2);
        let provider = &report.groups[1];
        assert_eq!(provider.jobs, 19);
        assert_eq!(provider.queue_wait_seconds.p50, Some(11));
        assert_eq!(provider.queue_wait_seconds.p95, Some(20));
        assert_eq!(provider.queue_wait_seconds.max, Some(20));
        assert_eq!(provider.execution_seconds.p95, Some(10));
    }

    #[test]
    fn timing_unknown_restarted_and_unfinished_jobs_are_not_zero_measurements() {
        let mut samples: Vec<_> = (0..6).map(sample).collect();
        samples[1].started_at = None;
        samples[2].started_at = Some(99);
        samples[2].finished_at = Some(98);
        samples[3].attempts = 2;
        samples[4].status = JobStatus::Queued;
        samples[4].attempts = 0;
        samples[5].status = JobStatus::Running;
        samples[5].finished_at = None;
        let report = summarize_job_timing(&samples, 10, false);
        let group = &report.groups[0];
        assert_eq!(group.restarted_jobs, 1);
        assert_eq!(group.statuses["queued"], 1);
        assert_eq!(group.queue_wait_seconds.measured, 2);
        assert_eq!(group.queue_wait_seconds.unavailable, 4);
        assert_eq!(group.queue_wait_seconds.p50, Some(0));
        assert_eq!(group.execution_seconds.measured, 1);
        assert_eq!(group.execution_seconds.unavailable, 5);
        assert_eq!(elapsed(Some(-1), Some(i64::MAX)), None);
        assert_eq!(elapsed(Some(i64::MAX), Some(0)), None);
        assert!(summarize_job_timing(&[], 10, false).groups.is_empty());
        let unknown = DurationSummary::new([None].into_iter(), 1);
        assert_eq!(unknown.p95, None);
        assert_eq!(unknown.max, None);
    }
}
