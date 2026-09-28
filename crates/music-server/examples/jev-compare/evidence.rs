//! Controlled evidence ablations. Questions and expectations never depend on answers.
use super::*;
use comparison::{Comparison, FitTarget};
use music_application::assistant::TypedQuestion;
use serde::Serialize;
use std::collections::BTreeMap;

#[path = "evidence_report.rs"]
pub mod report;
#[cfg(test)]
#[path = "evidence_tests.rs"]
mod tests;
#[path = "evidence_variants.rs"]
pub(super) mod variants;

const MAX_REQUESTS: usize = 2_500;
const MAX_INPUT_UNITS: u64 = 140_000_000;
pub const CURRENT: &str = "current";
pub const REPEAT: &str = "repeat_current";
const REPEAT_CASES: [&str; 6] = [
    "arctic-escape",
    "modern-temple-service",
    "battle-of-bards-ambiguity",
    "custom-vocabulary-alias",
    "quiet-intro-urgent-escape",
    "acoustic-context-volatile-development",
];
pub const ARMS: [(&str, &str); 14] = [
    (
        "metadata_only",
        "Retain eligible metadata cards; remove audio and catalog cards.",
    ),
    ("album_only", "Retain only the supplied album card."),
    ("genre_only", "Retain only the supplied genre card."),
    (
        "audio_only",
        "Retain only audio cards, with original coverage and reliability.",
    ),
    (
        "without_sections",
        "Remove section cards, retaining trajectories including their ending.",
    ),
    (
        "without_trajectories",
        "Remove trajectories, retaining sections including the ending.",
    ),
    ("without_voice", "Remove only the optional voice card."),
    (
        "numeric_only",
        "Remove derived physical bands; retain original numbers and meanings.",
    ),
    (
        "bands_only",
        "Replace banded physical magnitudes with their existing bins; retain timing, reliability and missingness.",
    ),
    (
        "compact_cards",
        "Remove duplicate card IDs and hoist identical band-scale explanations; preserve all facts.",
    ),
    (
        "repeat_cards_x3",
        "Add two explicitly identified copies of the same cards, without independent corroboration.",
    ),
    (
        "genre_as_catalog",
        "Move the same genre text into a synthetic MusicBrainz recording-genre card; source-presentation probe only.",
    ),
    (
        "genre_as_community",
        "Move the same genre text into one synthetic weak Last.fm label; source-presentation probe only.",
    ),
    (
        "add_identity_context",
        "Add supplied artist, origin, duration and unverified BPM with their limitations; never add paths or invented descriptions.",
    ),
];

#[derive(Serialize)]
pub struct Pair {
    pub context: usize,
    pub candidate: usize,
    pub variant: &'static str,
}

#[derive(Serialize)]
pub struct ScoringContext {
    pub case_id: String,
    pub partition: usize,
    pub baseline: usize,
    pub fit_threshold: f64,
    pub tags: BTreeMap<String, FitTarget>,
    pub cohort: &'static str,
}

pub struct Plan {
    // One entry per paid request. Its labels identify the first logical use;
    // scoring always uses the separate contexts, including when answers are shared.
    pub comparisons: Vec<Comparison>,
    pub contexts: Vec<ScoringContext>,
    pub pairs: Vec<Pair>,
    pub skipped: Vec<Value>,
    cases: Value,
    suite_id: String,
}

fn cohort(state: &Value) -> &'static str {
    let observations = state["observations"].as_object();
    let has = |prefix: &str| observations.is_some_and(|v| v.keys().any(|k| k.starts_with(prefix)));
    match (has("audio."), has("metadata.")) {
        (true, true) => "mixed",
        (true, false) => "audio_only",
        _ => "metadata_only",
    }
}

// Use the same question partitions for every arm, sized for the largest state.
// Question content and vocabulary coverage stay exact even when a larger state
// needs more requests than the production-only baseline.
fn partitions(
    questions: BTreeMap<String, TypedQuestion>,
    states: &[(&'static str, Value)],
) -> Result<Vec<BTreeMap<String, TypedQuestion>>> {
    let state = &states
        .iter()
        .max_by_key(|(_, state)| state.to_string().len())
        .ok_or("missing evidence states")?
        .1;
    let fits = |questions: &BTreeMap<String, TypedQuestion>| {
        TypedDecisionRequest {
            state: state.clone(),
            questions: questions.clone(),
        }
        .validate()
        .is_ok()
    };
    let mut result = Vec::new();
    let mut batch = BTreeMap::new();
    for (id, question) in questions {
        batch.insert(id.clone(), question.clone());
        if !fits(&batch) {
            batch.remove(&id);
            if batch.is_empty() {
                return Err("a data arm exceeds the per-question context bound".into());
            }
            result.push(std::mem::take(&mut batch));
            batch.insert(id, question);
            if !fits(&batch) {
                return Err("a data arm exceeds the per-question context bound".into());
            }
        }
    }
    if !batch.is_empty() {
        result.push(batch);
    }
    Ok(result)
}

pub fn build_plan() -> Result<Plan> {
    let source = comparison::build_plan()?;
    let cases = source.document()["cases"].clone();
    let suite = tag_quality_suite()?;
    let mut plan = Plan {
        comparisons: Vec::new(),
        contexts: Vec::new(),
        pairs: Vec::new(),
        skipped: Vec::new(),
        cases,
        suite_id: suite.id,
    };
    let mut known_requests = BTreeMap::new();
    for (case_index, case) in suite.cases.iter().enumerate() {
        let original = source
            .comparisons
            .iter()
            .filter(|v| v.case_id == case.id && v.variant == comparison::BASELINE)
            .collect::<Vec<_>>();
        let Some(first) = original.first() else {
            plan.skipped.push(json!({"case_id":case.id,"variant":"all","reason":"production_has_no_eligible_evidence"}));
            continue;
        };
        if original
            .iter()
            .any(|v| v.request.state != first.request.state)
        {
            return Err(
                "data experiment requires one production evidence view per synthetic case".into(),
            );
        }
        let mut states = vec![(CURRENT, first.request.state.clone())];
        for (name, _) in ARMS {
            let state = variants::transform(name, &first.request.state, &case.track)?;
            let reason = if state == first.request.state {
                Some("unchanged_or_not_applicable")
            } else if state["observations"]
                .as_object()
                .is_none_or(|v| v.is_empty())
            {
                Some("all_observations_removed_no_inference")
            } else {
                None
            };
            if let Some(reason) = reason {
                plan.skipped
                    .push(json!({"case_id":case.id,"variant":name,"reason":reason}));
            } else {
                states.push((name, state));
            }
        }
        let questions = original
            .iter()
            .flat_map(|v| v.request.questions.clone())
            .collect();
        let tags: BTreeMap<String, FitTarget> =
            original.iter().flat_map(|v| v.tags.clone()).collect();
        let batches = partitions(questions, &states)?;
        let cohort = cohort(&first.request.state);
        for (partition, questions) in batches.into_iter().enumerate() {
            let tags = tags
                .iter()
                .filter(|(id, _)| questions.contains_key(*id))
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect::<BTreeMap<_, _>>();
            // Rotate the order independently of outcomes. A repeat is always a
            // new, explicitly budgeted call; it is never an error retry.
            let mut ordered = states.clone();
            let offset = (case_index + partition) % ordered.len();
            ordered.rotate_left(offset);
            if REPEAT_CASES.contains(&case.id.as_str()) {
                ordered.push((REPEAT, first.request.state.clone()));
            }
            let mut indices = BTreeMap::new();
            for (variant, state) in ordered {
                let request = TypedDecisionRequest {
                    state,
                    questions: questions.clone(),
                };
                request.validate()?;
                let index = plan.register(
                    Comparison {
                        case_id: case.id.clone(),
                        partition,
                        variant,
                        fit_threshold: first.fit_threshold,
                        tags: tags.clone(),
                        request,
                    },
                    &mut known_requests,
                )?;
                indices.insert(variant, index);
            }
            let baseline = indices[CURRENT];
            let context = plan.contexts.len();
            plan.contexts.push(ScoringContext {
                case_id: case.id.clone(),
                partition,
                baseline,
                fit_threshold: first.fit_threshold,
                tags,
                cohort,
            });
            for (variant, candidate) in indices {
                if variant != CURRENT {
                    plan.pairs.push(Pair {
                        context,
                        candidate,
                        variant,
                    });
                }
            }
        }
    }
    plan.check_limits()?;
    Ok(plan)
}

impl Plan {
    fn register(&mut self, item: Comparison, known: &mut BTreeMap<String, usize>) -> Result<usize> {
        let key = fingerprint(
            &json!({"model":MODEL,"state":item.request.state,"questions":item.request.questions}),
        )?;
        if item.variant != REPEAT {
            if let Some(&index) = known.get(&key) {
                if self.comparisons[index].request != item.request {
                    return Err("request fingerprint collision".into());
                }
                return Ok(index);
            }
            known.insert(key, self.comparisons.len());
        }
        let index = self.comparisons.len();
        self.comparisons.push(item);
        Ok(index)
    }

    pub fn shared_requests(&self) -> Vec<Value> {
        let mut uses = BTreeMap::<usize, Vec<(usize, &str)>>::new();
        for (context, item) in self.contexts.iter().enumerate() {
            uses.entry(item.baseline)
                .or_default()
                .push((context, CURRENT));
        }
        for pair in &self.pairs {
            uses.entry(pair.candidate)
                .or_default()
                .push((pair.context, pair.variant));
        }
        uses.into_iter().filter(|(_, uses)| uses.len() > 1).map(|(request, uses)| {
            let expectations = uses.iter().map(|(index, _)| {
                let context = &self.contexts[*index];
                let case = self.cases.as_array().and_then(|cases| cases.iter().find(|case| case["id"] == context.case_id));
                json!({"tags":context.tags,"threshold":context.fit_threshold,
                    "case_expectations":case.map(|case|json!({"required_tags":case["required_tags"],"forbidden_tags":case["forbidden_tags"],
                        "forbidden_groups":case["forbidden_groups"],"maximum_tags":case["maximum_tags"]}))}).to_string()
            }).collect::<std::collections::BTreeSet<_>>();
            json!({"request_index":request,"distinct_expectation_sets":expectations.len(),
                "uses":uses.iter().map(|(index, variant)| {
                    let context=&self.contexts[*index];
                    json!({"context":index,"case_id":context.case_id,"partition":context.partition,"variant":variant})
                }).collect::<Vec<_>>()})
        }).collect()
    }

    pub fn reservation(&self) -> u64 {
        self.comparisons
            .iter()
            .map(|v| model_request_reservation(&v.request.accounting_request(), 0))
            .sum()
    }

    fn check_limits(&self) -> Result<()> {
        if self.comparisons.is_empty()
            || self.comparisons.len() > MAX_REQUESTS
            || self.reservation() > MAX_INPUT_UNITS
        {
            return Err("evidence experiment exceeds its hard request/input bounds".into());
        }
        Ok(())
    }

    pub fn check_authorization(&self, sha: &str, requests: usize, units: u64) -> Result<()> {
        self.check_limits()?;
        if requests != self.comparisons.len()
            || units != self.reservation()
            || fingerprint(&self.document())? != sha
        {
            return Err(
                "evidence experiment requires its exact reviewed plan and request/input bounds"
                    .into(),
            );
        }
        Ok(())
    }

    pub fn document(&self) -> Value {
        json!({"schema_version":"jev-evidence-experiment/v1","engine_id":JEV_TAGGER_CONTRACT,
            "suite_id":self.suite_id,"inference_identity":jev_inference_identity(),"model":MODEL,"endpoint":ENDPOINT,
            "certifies_model":false,"request_count":self.comparisons.len(),"max_input_units":self.reservation(),
            "arms":ARMS,"repeat_cases":REPEAT_CASES,"cases":self.cases,"skipped":self.skipped,
            "logical_request_uses":self.contexts.len()+self.pairs.len(),
            "scoring_contexts":self.contexts,"pairs":self.pairs,"comparisons":self.comparisons,"shared_requests":self.shared_requests(),
            "purpose":"Exploratory synthetic evidence sensitivity, not music accuracy. Hold all production question bodies, criteria, definitions, vocabulary and thresholds fixed. Align partitions across arms using the largest state; rotate order deterministically. Skip unchanged or empty-data arms. Deduplicate exact provider bodies across cases and arms, sharing one answer but preserving each logical context's expectations. Shared uses are not independent observations; request labels identify only the first use. Six cases have one separately budgeted identical-input repeat, never an automatic retry. Source relocation probes reuse the same text and do not represent independently retrieved or verified facts. Report paired retention, explicit negatives, source cohorts, physical byte sizes, actual usage and repeat variation separately. No grounding, final candidate selection, safety repeats, conformance, private audio or application acceptance writes. No full-suite pass rate or automatic best-arm selection."})
    }
}
