//! Single-factor first-pass experiment; expectations never enter provider requests.
use super::*;
use music_application::assistant::{TagQualityCase, TypedQuestion};
use serde::Serialize;
use std::collections::BTreeMap;

const MAX_REQUESTS: usize = 400;
const MAX_INPUT_UNITS: u64 = 20_000_000;
pub const BASELINE: &str = "current";
pub const CANDIDATE: &str = "fit_without_criteria";

#[derive(Clone, Serialize)]
pub struct FitTarget {
    pub tag: String,
    pub group: String,
    pub required: bool,
    pub forbidden: bool,
}

#[derive(Serialize)]
pub struct Comparison {
    pub case_id: String,
    pub partition: usize,
    pub variant: &'static str,
    pub fit_threshold: f64,
    pub tags: BTreeMap<String, FitTarget>,
    pub request: TypedDecisionRequest,
}

pub struct Plan {
    pub comparisons: Vec<Comparison>,
    cases: Vec<Value>,
    suite_id: String,
}

fn expectations(case: &TagQualityCase, partitions: usize) -> Value {
    json!({"id":case.id,"gate":case.gate,"vocabulary":case.vocabulary,
        "required_tags":case.required_tags,"forbidden_tags":case.forbidden_tags,
        "forbidden_groups":case.forbidden_groups,"maximum_tags":case.maximum_tags,
        "assessment_partitions":partitions,
        "status":if partitions == 0 {"no_eligible_evidence_no_requests"} else {"planned"}})
}

pub fn build_plan() -> Result<Plan> {
    let suite = tag_quality_suite()?;
    let mut plan = Plan {
        comparisons: Vec::new(),
        cases: Vec::new(),
        suite_id: suite.id,
    };
    for case in &suite.cases {
        let vocabulary = case.vocabulary.snapshot()?;
        let entries = vocabulary
            .document
            .groups
            .iter()
            .flat_map(|group| group.tags.iter().map(|tag| (&group.key, tag)))
            .collect::<Vec<_>>();
        let tasks = plan_jev_tagging(std::slice::from_ref(&case.track), &vocabulary)?;
        let task = tasks.first().ok_or("missing synthetic task")?;
        plan.cases
            .push(expectations(case, task.assessment_requests().len()));
        for (partition, baseline) in task.assessment_requests().iter().enumerate() {
            let mut candidate = baseline.clone();
            let mut tags = BTreeMap::new();
            for (id, question) in &mut candidate.questions {
                if let Some(index) = id.strip_prefix("fit_") {
                    let (group, tag) = entries
                        .get(index.parse::<usize>()?)
                        .ok_or("unknown fit tag")?;
                    let TypedQuestion::Noul { criteria, .. } = question else {
                        return Err("fit must be a Noul".into());
                    };
                    // Only remove the optional answer criteria. Every instruction,
                    // definition, evidence card, Choice and partition stays intact.
                    criteria.clear();
                    tags.insert(
                        id.clone(),
                        FitTarget {
                            tag: tag.name.clone(),
                            group: (*group).clone(),
                            required: case.required_tags.contains(&tag.name),
                            forbidden: case.maximum_tags == 0
                                || case.forbidden_tags.contains(&tag.name)
                                || case.forbidden_groups.contains(group),
                        },
                    );
                }
            }
            candidate.validate()?;
            if tags.is_empty() {
                return Err("assessment partition has no comparison targets".into());
            }
            for (variant, request) in [(BASELINE, baseline.clone()), (CANDIDATE, candidate)] {
                plan.comparisons.push(Comparison {
                    case_id: case.id.clone(),
                    partition,
                    variant,
                    fit_threshold: task.diagnostics().fit_threshold,
                    tags: tags.clone(),
                    request,
                });
            }
        }
    }
    Ok(plan)
}

impl Plan {
    pub fn reservation(&self) -> u64 {
        self.comparisons
            .iter()
            .map(|item| model_request_reservation(&item.request.accounting_request(), 0))
            .sum()
    }

    pub fn document(&self) -> Value {
        json!({
            "schema_version":"jev-framing-comparison/v4", "engine_id":JEV_TAGGER_CONTRACT,
            "suite_id":self.suite_id,"inference_identity":jev_inference_identity(),
            "model":MODEL,"endpoint":ENDPOINT,"certifies_model":false,
            "request_count":self.comparisons.len(),"max_input_units":self.reservation(),
            "purpose":"Compare current first-pass Nouls against exactly the same questions without optional yes/no criteria. Every synthetic suite case is accounted for, including all prior passing positives, failures, custom definitions and safety controls. Evidence, instructions, thresholds, Choice questions and production partitioning remain identical. No grounding, safety repeats, conformance, library data or application acceptance writes. First-pass results are diagnostic, not tag predictions or certification. No automatic retries.",
            "cases":self.cases,"comparisons":self.comparisons,
        })
    }

    pub fn check_authorization(&self, expected: &str, count: usize, units: u64) -> Result<()> {
        if fingerprint(&self.document())? != expected {
            return Err("plan changed; review a fresh offline plan before executing".into());
        }
        if count != self.comparisons.len()
            || units != self.reservation()
            || count > MAX_REQUESTS
            || units > MAX_INPUT_UNITS
        {
            return Err("comparison requires the exact reviewed request and input bounds".into());
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn comparison_covers_whole_suite_and_changes_only_optional_fit_criteria() -> Result<()> {
        let plan = build_plan()?;
        let suite = tag_quality_suite()?;
        assert_eq!(plan.cases.len(), suite.cases.len());
        let mut expected_requests = 0;
        for case in &suite.cases {
            let tasks = plan_jev_tagging(
                std::slice::from_ref(&case.track),
                &case.vocabulary.snapshot()?,
            )?;
            let task = tasks.first().ok_or("task")?;
            let comparisons = plan
                .comparisons
                .iter()
                .filter(|item| item.case_id == case.id)
                .collect::<Vec<_>>();
            assert_eq!(comparisons.len(), 2 * task.assessment_requests().len());
            expected_requests += comparisons.len();
            for (partition, pair) in comparisons.chunks_exact(2).enumerate() {
                assert_eq!(pair[0].request, task.assessment_requests()[partition]);
                assert_eq!(pair[0].request.state, pair[1].request.state);
                assert_eq!(
                    pair[0].request.questions.keys().collect::<Vec<_>>(),
                    pair[1].request.questions.keys().collect::<Vec<_>>()
                );
                for (id, question) in &pair[0].request.questions {
                    if let TypedQuestion::Noul {
                        instructions,
                        criteria,
                    } = question
                    {
                        assert!(id.starts_with("fit_"));
                        assert!(!criteria.is_empty());
                        assert_eq!(
                            pair[1].request.questions[id],
                            TypedQuestion::Noul {
                                instructions: instructions.clone(),
                                criteria: BTreeMap::new(),
                            }
                        );
                        assert!(
                            serde_json::to_value(&pair[1].request.questions[id])?
                                .get("criteria")
                                .is_none()
                        );
                    } else {
                        assert_eq!(question, &pair[1].request.questions[id]);
                    }
                }
                pair[0].request.validate()?;
                pair[1].request.validate()?;
            }
        }
        assert_eq!(expected_requests, plan.comparisons.len());
        let sha = fingerprint(&plan.document())?;
        plan.check_authorization(&sha, expected_requests, plan.reservation())?;
        Ok(())
    }

    #[test]
    fn comparisons_include_regressed_positives_and_explicit_negative_controls() -> Result<()> {
        let plan = build_plan()?;
        for (case, tag) in [
            ("forest-hunt", "hunting"),
            ("arctic-escape", "escape"),
            ("infernal-dark-ritual", "infernal realm"),
            ("curious-puzzle", "puzzle"),
            ("temple-band-name-ambiguity", "city"),
            ("quiet-intro-urgent-escape", "escape"),
            ("quiet-intro-urgent-escape", "chase"),
            ("slow-tempo-high-intensity-siege", "combat"),
        ] {
            assert!(plan.comparisons.iter().any(|item| {
                item.case_id == case
                    && item
                        .tags
                        .values()
                        .any(|target| target.tag == tag && target.required)
            }));
        }
        assert!(plan.comparisons.iter().any(|item| {
            item.case_id == "metadata-prompt-injection"
                && item
                    .tags
                    .values()
                    .any(|target| target.tag == "combat" && target.forbidden)
        }));
        assert!(plan.comparisons.iter().any(|item| {
            item.case_id == "signal-evidence-does-not-invent-context"
                && item
                    .tags
                    .values()
                    .any(|target| target.group == "setting" && target.forbidden)
        }));
        assert!(
            plan.comparisons
                .iter()
                .any(|item| item.case_id == "custom-vocabulary-redefined-label")
        );
        assert!(plan.cases.iter().any(
            |case| case["id"] == "insufficient-evidence" && case["assessment_partitions"] == 0
        ));
        for item in &plan.comparisons {
            let body = serde_json::to_string(&item.request)?;
            assert!(!body.contains("required_tags") && !body.contains("forbidden_tags"));
        }
        Ok(())
    }

    #[test]
    fn comparison_authorization_rejects_drift_and_inexact_caps() -> Result<()> {
        let plan = build_plan()?;
        let sha = fingerprint(&plan.document())?;
        let count = plan.comparisons.len();
        let units = plan.reservation();
        for (hash, calls, tokens) in [
            ("different", count, units),
            (&sha, count - 1, units),
            (&sha, count + 1, units),
            (&sha, count, units - 1),
            (&sha, count, units + 1),
        ] {
            assert!(plan.check_authorization(hash, calls, tokens).is_err());
        }
        Ok(())
    }
}
