//! Application-owned traces for synthetic quality checks, never model instructions.
use super::*;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct JevTaggingDiagnostics {
    pub schema_version: String,
    pub engine_id: String,
    /// Bounded request states without local track IDs. Statuses identify unfinished checks.
    pub input_snapshot: Value,
    pub fit_threshold: f64,
    pub grounding_threshold: f64,
    pub period_threshold: f64,
    pub period_choice: Option<String>,
    pub tags: Vec<JevTagDiagnostic>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct JevTagDiagnostic {
    pub tag_id: String,
    pub tag: String,
    pub group: String,
    pub fit: Option<f64>,
    pub period_probability: Option<f64>,
    pub candidate: bool,
    pub status: JevTagStatus,
    pub grounding: Vec<JevObservationDiagnostic>,
}

#[derive(Debug, Clone, Copy, Eq, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum JevTagStatus {
    NoEligibleEvidence,
    NotEvaluated,
    AssessmentPending,
    BelowFitThreshold,
    CandidateLimit,
    PeriodNotSelected,
    BelowPeriodThreshold,
    GroundingPending,
    NoUnambiguousSupport,
    Accepted,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct JevObservationDiagnostic {
    pub evidence_id: String,
    pub support: f64,
    pub conflict: f64,
}

impl JevTaggingDiagnostics {
    pub(super) fn new(task: &JevTaggerTask) -> Self {
        let mut trace = Self {
            schema_version: "jev-tagging-diagnostics/v1".to_owned(),
            engine_id: JEV_TAGGER_CONTRACT.to_owned(),
            input_snapshot: json!(task.states),
            fit_threshold: FIT_THRESHOLD,
            grounding_threshold: GROUNDING_THRESHOLD,
            period_threshold: PERIOD_CHOICE_THRESHOLD,
            period_choice: None,
            tags: Vec::new(),
        };
        trace.record(task, &BTreeMap::new(), &BTreeMap::new(), false);
        trace
    }

    pub(super) fn record(
        &mut self,
        task: &JevTaggerTask,
        assessment: &BTreeMap<String, TypedAnswer>,
        evidence: &BTreeMap<String, TypedAnswer>,
        assessment_complete: bool,
    ) {
        let period = match assessment.get(PERIOD_QUESTION) {
            Some(TypedAnswer::Choice {
                choice,
                probabilities,
                ..
            }) => Some((choice, probabilities)),
            _ => None,
        };
        self.period_choice = period.map(|(choice, _)| choice.clone());
        let candidates = task.candidates(assessment);
        self.tags = task
            .tags
            .iter()
            .enumerate()
            .map(|(index, (group, tag))| {
                let fit = answer_noul(
                    if group == "period" {
                        evidence
                    } else {
                        assessment
                    },
                    &format!("fit_{index}"),
                )
                .ok();
                let period_probability = (group == "period")
                    .then(|| {
                        period.and_then(|(_, probabilities)| {
                            probabilities.get(&format!("tag_{index}")).copied()
                        })
                    })
                    .flatten();
                let candidate = candidates.iter().find(|candidate| candidate.index == index);
                let mut eligible_count = 0;
                let grounding = task
                    .observations
                    .iter()
                    .enumerate()
                    .filter_map(|(observation_index, (id, _))| {
                        if !eligible_observation(group, id) {
                            return None;
                        }
                        eligible_count += 1;
                        Some(JevObservationDiagnostic {
                            evidence_id: id.clone(),
                            support: answer_noul(
                                evidence,
                                &format!("support_{index}_{observation_index}"),
                            )
                            .ok()?,
                            conflict: answer_noul(
                                evidence,
                                &format!("conflict_{index}_{observation_index}"),
                            )
                            .ok()?,
                        })
                    })
                    .collect::<Vec<_>>();
                let status = if eligible_count == 0 {
                    JevTagStatus::NoEligibleEvidence
                } else if !assessment_complete {
                    if fit.is_some() || (group == "period" && period.is_some()) {
                        JevTagStatus::AssessmentPending
                    } else {
                        JevTagStatus::NotEvaluated
                    }
                } else if group == "period"
                    && period.is_none_or(|(choice, _)| choice != &format!("tag_{index}"))
                {
                    JevTagStatus::PeriodNotSelected
                } else if group == "period"
                    && period_probability.is_some_and(|value| value < PERIOD_CHOICE_THRESHOLD)
                {
                    JevTagStatus::BelowPeriodThreshold
                } else if fit.is_some_and(|value| value < FIT_THRESHOLD) {
                    JevTagStatus::BelowFitThreshold
                } else if let Some(candidate) = candidate {
                    if grounding.len() != eligible_count || fit.is_none() {
                        JevTagStatus::GroundingPending
                    } else if task.decision(*candidate, evidence).ok().flatten().is_some() {
                        JevTagStatus::Accepted
                    } else {
                        JevTagStatus::NoUnambiguousSupport
                    }
                } else if fit.is_some() {
                    JevTagStatus::CandidateLimit
                } else {
                    JevTagStatus::NotEvaluated
                };
                JevTagDiagnostic {
                    tag_id: tag.id.clone(),
                    tag: tag.name.clone(),
                    group: group.clone(),
                    fit,
                    period_probability,
                    candidate: candidate.is_some(),
                    status,
                    grounding,
                }
            })
            .collect();
    }
}
