//! Paired source/amount diagnostics, with explicit coverage and no automatic winner.
use super::*;
use music_application::assistant::{TagQualityVocabulary, TypedAnswer};
use std::collections::BTreeSet;

#[derive(Default, Serialize)]
struct Counts {
    required: usize,
    required_baseline_retained: usize,
    required_variant_retained: usize,
    required_gains: usize,
    required_losses: usize,
    forbidden: usize,
    forbidden_baseline: usize,
    forbidden_variant: usize,
    new_forbidden_candidates: usize,
    resolved_forbidden_candidates: usize,
    unscored_crossings: usize,
    required_score_delta_sum: f64,
}

impl Counts {
    fn add(&mut self, tag: &FitTarget, old: f64, new: f64, threshold: f64) {
        let before = old >= threshold;
        let after = new >= threshold;
        if tag.required {
            self.required += 1;
            self.required_baseline_retained += usize::from(before);
            self.required_variant_retained += usize::from(after);
            self.required_gains += usize::from(!before && after);
            self.required_losses += usize::from(before && !after);
            self.required_score_delta_sum += new - old;
        }
        if tag.forbidden {
            self.forbidden += 1;
            self.forbidden_baseline += usize::from(before);
            self.forbidden_variant += usize::from(after);
            self.new_forbidden_candidates += usize::from(!before && after);
            self.resolved_forbidden_candidates += usize::from(before && !after);
        }
        if !tag.required && !tag.forbidden && before != after {
            self.unscored_crossings += 1;
        }
    }
}

#[derive(Default, Serialize)]
struct ArmSummary {
    planned_partitions: usize,
    paired_partitions: usize,
    paired_cases: BTreeSet<String>,
    distinct_baseline_requests: BTreeSet<usize>,
    distinct_variant_requests: BTreeSet<usize>,
    distinct_request_pairs: BTreeSet<(usize, usize)>,
    counts: Counts,
    by_cohort_and_group: BTreeMap<String, Counts>,
    baseline_state_bytes: usize,
    variant_state_bytes: usize,
    questions_bytes: usize,
    paired_baseline_reported_input_tokens: u64,
    paired_variant_reported_input_tokens: u64,
    pairs_reporting_both_input_counts: usize,
    period_pairs: usize,
    changed_period_winners: usize,
}

fn period_options(plan: &Plan) -> Result<BTreeMap<String, BTreeMap<String, FitTarget>>> {
    let mut result = BTreeMap::new();
    for case in plan.cases.as_array().ok_or("case manifest")? {
        let vocabulary: TagQualityVocabulary = serde_json::from_value(case["vocabulary"].clone())?;
        let snapshot = vocabulary.snapshot()?;
        let contains = |field: &str, value: &str| {
            case[field]
                .as_array()
                .is_some_and(|v| v.iter().any(|v| v == value))
        };
        let tags = snapshot
            .document
            .groups
            .iter()
            .flat_map(|g| g.tags.iter().map(move |tag| (&g.key, tag)))
            .enumerate()
            .filter(|(_, (group, _))| group.as_str() == "period")
            .map(|(index, (group, tag))| {
                (
                    format!("tag_{index}"),
                    FitTarget {
                        tag: tag.name.clone(),
                        group: group.clone(),
                        required: contains("required_tags", &tag.name),
                        forbidden: case["maximum_tags"] == 0
                            || contains("forbidden_tags", &tag.name)
                            || contains("forbidden_groups", group),
                    },
                )
            })
            .collect();
        result.insert(case["id"].as_str().ok_or("case id")?.to_owned(), tags);
    }
    Ok(result)
}

fn period_snapshot(answer: &TypedAnswer, options: &BTreeMap<String, FitTarget>) -> Result<Value> {
    let TypedAnswer::Choice {
        choice,
        probabilities,
        confidence,
    } = answer
    else {
        return Err("period answer must be Choice".into());
    };
    let tag = options.get(choice);
    if tag.is_none() && choice != "no_supported_period" {
        return Err("unknown period option".into());
    }
    let score = probabilities
        .get(choice)
        .ok_or("missing period winner score")?;
    Ok(
        json!({"option":choice,"tag":tag.map(|v|&v.tag),"winner_score":score,"confidence":confidence,
        "required":tag.is_some_and(|v|v.required),"forbidden":tag.is_some_and(|v|v.forbidden),
        "interpretation":"Relative Choice only; the independent applicability Noul and grounding have not run."}),
    )
}

pub fn read(plan: &Plan, path: &Path) -> Result<Value> {
    const MAX_BYTES: u64 = 256 * 1024 * 1024;
    let mut bytes = Vec::new();
    File::open(path)?
        .take(MAX_BYTES + 1)
        .read_to_end(&mut bytes)?;
    if bytes.len() as u64 > MAX_BYTES {
        return Err("evidence journal exceeds report bound".into());
    }
    analyze(plan, std::str::from_utf8(&bytes)?)
}

pub(super) fn analyze(plan: &Plan, text: &str) -> Result<Value> {
    let document = plan.document();
    let observed = journal::parse(&document, &plan.comparisons, text)?;
    let options = period_options(plan)?;
    let mut arms = BTreeMap::<String, ArmSummary>::new();
    let mut rows = Vec::new();
    let mut controls = Vec::new();
    let mut period_choices = Vec::new();
    let mut changed_controls = 0;
    let mut control_max_abs_delta = 0.0_f64;
    for pair in &plan.pairs {
        let context = plan
            .contexts
            .get(pair.context)
            .ok_or("unknown scoring context")?;
        let baseline = plan
            .comparisons
            .get(context.baseline)
            .ok_or("unknown baseline")?;
        let candidate = plan
            .comparisons
            .get(pair.candidate)
            .ok_or("unknown candidate")?;
        if baseline.request.questions != candidate.request.questions {
            return Err("evidence pair does not preserve question identity".into());
        }
        let summary = arms.entry(pair.variant.to_owned()).or_default();
        summary.planned_partitions += 1;
        let (Some(before), Some(after)) = (
            observed.answers.get(&context.baseline),
            observed.answers.get(&pair.candidate),
        ) else {
            continue;
        };
        summary.paired_partitions += 1;
        summary.distinct_baseline_requests.insert(context.baseline);
        summary.distinct_variant_requests.insert(pair.candidate);
        summary
            .distinct_request_pairs
            .insert((context.baseline, pair.candidate));
        if let (Some(old), Some(new)) = (before.get("period"), after.get("period")) {
            let old = period_snapshot(old, &options[&context.case_id])?;
            let new = period_snapshot(new, &options[&context.case_id])?;
            summary.period_pairs += 1;
            summary.changed_period_winners += usize::from(old["option"] != new["option"]);
            period_choices.push(json!({"case_id":context.case_id,"variant":pair.variant,
                "baseline_request_index":context.baseline,"candidate_request_index":pair.candidate,
                "cohort":context.cohort,"baseline":old,"candidate":new}));
        }
        summary.paired_cases.insert(context.case_id.clone());
        summary.baseline_state_bytes += serde_json::to_vec(&baseline.request.state)?.len();
        summary.variant_state_bytes += serde_json::to_vec(&candidate.request.state)?.len();
        summary.questions_bytes += serde_json::to_vec(&baseline.request.questions)?.len();
        if let (Some(old), Some(new)) = (
            observed.usage_by_request[&context.baseline][0],
            observed.usage_by_request[&pair.candidate][0],
        ) {
            summary.pairs_reporting_both_input_counts += 1;
            summary.paired_baseline_reported_input_tokens += old;
            summary.paired_variant_reported_input_tokens += new;
        }
        for (id, tag) in &context.tags {
            let (Some(TypedAnswer::Noul { noul: old }), Some(TypedAnswer::Noul { noul: new })) =
                (before.get(id), after.get(id))
            else {
                return Err("missing Noul comparison answer".into());
            };
            let threshold = context.fit_threshold;
            summary.counts.add(tag, *old, *new, threshold);
            summary
                .by_cohort_and_group
                .entry(format!("{}/{}", context.cohort, tag.group))
                .or_default()
                .add(tag, *old, *new, threshold);
            let row = json!({"case_id":context.case_id,"cohort":context.cohort,"variant":pair.variant,
                "partition":context.partition,"baseline_request_index":context.baseline,"candidate_request_index":pair.candidate,
                "question_id":id,"tag":tag.tag,"group":tag.group,
                "required":tag.required,"forbidden":tag.forbidden,"baseline":old,"candidate":new,
                "delta":new-old,"threshold":threshold,"baseline_above_threshold":old>=&threshold,
                "candidate_above_threshold":new>=&threshold});
            if pair.variant == REPEAT {
                changed_controls += usize::from((*old >= threshold) != (*new >= threshold));
                control_max_abs_delta = control_max_abs_delta.max((new - old).abs());
                controls.push(row);
            } else {
                rows.push(row);
            }
        }
    }
    let paired = arms.values().map(|v| v.paired_partitions).sum::<usize>();
    Ok(
        json!({"schema_version":"jev-evidence-report/v1","certifies_model":false,
        "plan_sha256":fingerprint(&document)?,"complete":observed.complete,"planned_requests":plan.comparisons.len(),
        "responses":observed.answers.len(),"unresolved_attempt":observed.pending,"paired_partitions":paired,
        "logical_request_uses":plan.contexts.len()+plan.pairs.len(),"shared_requests":plan.shared_requests(),
        "unpaired_or_unrun_partitions":plan.pairs.len()-paired,"skipped":plan.skipped,"arms":arms,
        "usage":{"reported_input_tokens":observed.tokens[0],"reported_output_tokens":observed.tokens[1],
            "responses_reporting_input":observed.token_reports[0],"responses_reporting_output":observed.token_reports[1]},
        "repeat_controls":{"scores":controls,"threshold_crossings":changed_controls,
            "max_observed_absolute_score_delta":if controls.is_empty(){None}else{Some(control_max_abs_delta)}},
        "scores":rows,"period_choices":period_choices,
        "interpretation":"Exploratory first-pass sensitivity only. Compare each arm to its own paired baseline, within source cohort and tag group; arm denominators differ. Counts, partitions and paired usage are logical case comparisons. Shared request indices expose reused observations and differing expectations; they are not independent samples. Distinct request sets show physical coverage. Required retention after data removal measures available support, not intrinsic model error. Unlisted tags are unscored; forbidden candidates are not final false tags. Empty/no-op arms make no request and are not successes. Missing responses and usage remain unknown. The repeated baseline is a small variability control, not a confidence interval or statistical significance test. Source relocation uses copied synthetic text, not independently retrieved catalog facts. Byte counts are not token estimates. Paired baseline and variant usage may reuse requests and must not be summed as total spend; only root usage counts each paid response once. Period Choice snapshots are separate from Nouls and omit the winner's independent applicability check. Grounding, candidate limits, conformance and final suite scoring are not evaluated. No automatic best-arm selection, production adoption or music accuracy claim. Any promising result needs held-out independent listening and full native validation."}),
    )
}
