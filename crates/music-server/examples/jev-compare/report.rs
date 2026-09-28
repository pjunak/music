//! Read-only interpretation of a current journal. Missing answers are never zero scores.
use super::*;
use comparison::{BASELINE, CANDIDATE, Plan};
use music_application::assistant::TypedAnswer;
#[cfg(test)]
use std::collections::BTreeMap;

const MAX_JOURNAL_BYTES: u64 = 64 * 1024 * 1024;

pub fn read(plan: &Plan, path: &Path) -> Result<Value> {
    let mut bytes = Vec::new();
    File::open(path)?
        .take(MAX_JOURNAL_BYTES + 1)
        .read_to_end(&mut bytes)?;
    if bytes.len() as u64 > MAX_JOURNAL_BYTES {
        return Err("comparison journal exceeds report bound".into());
    }
    analyze(plan, std::str::from_utf8(&bytes)?)
}

fn analyze(plan: &Plan, journal: &str) -> Result<Value> {
    let observed = super::journal::parse(&plan.document(), &plan.comparisons, journal)?;
    let answers = &observed.answers;
    let next = answers.len();
    let pending = observed.pending;
    let complete = observed.complete;
    let tokens = observed.tokens;
    let token_reports = observed.token_reports;
    let mut rows = Vec::new();
    let mut gains = Vec::new();
    let mut losses = Vec::new();
    let mut new_forbidden = Vec::new();
    let mut resolved_forbidden = Vec::new();
    let mut unscored_crossings = Vec::new();
    let mut paired = 0;
    let mut required_pairs = 0;
    for (pair_index, pair) in plan.comparisons.chunks_exact(2).enumerate() {
        let [baseline, candidate] = pair else {
            return Err("invalid comparison pair".into());
        };
        if baseline.variant != BASELINE
            || candidate.variant != CANDIDATE
            || baseline.case_id != candidate.case_id
            || baseline.partition != candidate.partition
        {
            return Err("invalid comparison pair identity".into());
        }
        let (Some(before), Some(after)) = (
            answers.get(&(2 * pair_index)),
            answers.get(&(2 * pair_index + 1)),
        ) else {
            continue;
        };
        paired += 1;
        for (id, tag) in &baseline.tags {
            let (TypedAnswer::Noul { noul: old }, TypedAnswer::Noul { noul: new }) =
                (&before[id], &after[id])
            else {
                return Err("fit answer is not a Noul".into());
            };
            let old_above = *old >= baseline.fit_threshold;
            let new_above = *new >= baseline.fit_threshold;
            let row = json!({"case_id":baseline.case_id,"question_id":id,"tag":tag.tag,"group":tag.group,
                "required":tag.required,"forbidden":tag.forbidden,
                "baseline":old,"candidate":new,"threshold":baseline.fit_threshold,
                "baseline_above_threshold":old_above,"candidate_above_threshold":new_above});
            if tag.required {
                required_pairs += 1;
                if !old_above && new_above {
                    gains.push(row.clone());
                }
                if old_above && !new_above {
                    losses.push(row.clone());
                }
            }
            if tag.forbidden {
                if !old_above && new_above {
                    new_forbidden.push(row.clone());
                }
                if old_above && !new_above {
                    resolved_forbidden.push(row.clone());
                }
            }
            if !tag.required && !tag.forbidden && old_above != new_above {
                unscored_crossings.push(row.clone());
            }
            rows.push(row);
        }
    }
    Ok(json!({
        "schema_version":"jev-first-pass-report/v1","certifies_model":false,
        "plan_sha256":fingerprint(&plan.document())?,"complete":complete,
        "planned_requests":plan.comparisons.len(),"responses":next,
        "unresolved_attempt":pending,"paired_partitions":paired,
        "unpaired_or_unrun_partitions":plan.comparisons.len()/2-paired,
        "required_noul_pairs":required_pairs,
        "usage":{"reported_input_tokens":tokens[0],"reported_output_tokens":tokens[1],
            "responses_reporting_input":token_reports[0],"responses_reporting_output":token_reports[1]},
        "required_gains":gains,"required_losses":losses,
        "new_forbidden_candidates":new_forbidden,"resolved_forbidden_candidates":resolved_forbidden,
        "unscored_threshold_crossings":unscored_crossings,"scores":rows,
        "interpretation":"Only paired initial Noul scores are compared, before candidate limits and grounding. Choice/period results are not scored. Forbidden crossings are investigation flags, not final false-positive tags; unlisted tags are unscored, not negatives. Missing or unpaired responses are not abstentions. No full-suite pass rate, safety certification or production adoption can be inferred. A candidate needs full native validation and independent listening."
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use music_application::assistant::TypedQuestion;

    fn header(plan: &Plan) -> Result<String> {
        Ok(format!(
            "{}\n",
            serde_json::to_string(&json!({"event":"plan","plan":plan.document()}))?
        ))
    }

    fn response(plan: &Plan, index: usize, required: f64, forbidden: f64) -> Value {
        let item = &plan.comparisons[index];
        let answers = item.request.questions.iter().map(|(id, question)| {
            let answer = match question {
                TypedQuestion::Noul { .. } => json!({"type":"noul","noul":if item.tags[id].required { required } else if item.tags[id].forbidden { forbidden } else { 0.1 }}),
                TypedQuestion::Choice { criteria, .. } => {
                    let choice = criteria.keys().next().cloned().unwrap_or_default();
                    json!({"type":"choice","choice":choice,"confidence":1.0,
                        "probabilities":criteria.keys().map(|key| (key.clone(),f64::from(key == &choice))).collect::<BTreeMap<_,_>>()})
                }
            };
            (id.clone(), answer)
        }).collect::<BTreeMap<_,_>>();
        json!({"event":"response","index":index,"result":{"model":MODEL,"answers":answers,"input_tokens":50,"output_tokens":10}})
    }

    fn append(journal: &mut String, event: Value) {
        journal.push_str(&event.to_string());
        journal.push('\n');
    }

    fn attempt(plan: &Plan, index: usize) -> Value {
        json!({"event":"attempt_started","index":index,"case_id":plan.comparisons[index].case_id,"variant":plan.comparisons[index].variant})
    }

    #[test]
    fn report_separates_gains_losses_negatives_and_incomplete_pairs() -> Result<()> {
        let plan = comparison::build_plan()?;
        let mut journal = header(&plan)?;
        append(&mut journal, attempt(&plan, 0));
        append(&mut journal, response(&plan, 0, 0.69, 0.69));
        let unpaired = analyze(&plan, &journal)?;
        assert_eq!(unpaired["paired_partitions"], 0);
        assert_eq!(unpaired["complete"], false);
        let count = plan
            .comparisons
            .iter()
            .take_while(|item| item.case_id == plan.comparisons[0].case_id)
            .count();
        for index in 1..count {
            let score = if index % 2 == 0 { 0.69 } else { 0.70 };
            append(&mut journal, attempt(&plan, index));
            append(&mut journal, response(&plan, index, score, score));
        }
        let report = analyze(&plan, &journal)?;
        assert!(
            !report["required_gains"]
                .as_array()
                .ok_or("gains")?
                .is_empty()
        );
        assert!(
            !report["new_forbidden_candidates"]
                .as_array()
                .ok_or("negatives")?
                .is_empty()
        );
        assert!(
            report["required_losses"]
                .as_array()
                .ok_or("losses")?
                .is_empty()
        );
        assert_eq!(
            report["usage"]["reported_input_tokens"],
            (count * 50) as u64
        );
        let mut regression = header(&plan)?;
        for index in 0..count {
            let score = if index % 2 == 0 { 0.70 } else { 0.69 };
            append(&mut regression, attempt(&plan, index));
            append(&mut regression, response(&plan, index, score, score));
        }
        let report = analyze(&plan, &regression)?;
        assert!(
            !report["required_losses"]
                .as_array()
                .ok_or("losses")?
                .is_empty()
        );
        assert!(
            !report["resolved_forbidden_candidates"]
                .as_array()
                .ok_or("resolved")?
                .is_empty()
        );
        Ok(())
    }

    #[test]
    fn complete_report_requires_every_response_and_never_certifies() -> Result<()> {
        let plan = comparison::build_plan()?;
        let mut journal = header(&plan)?;
        for index in 0..plan.comparisons.len() {
            append(&mut journal, attempt(&plan, index));
            append(&mut journal, response(&plan, index, 0.8, 0.1));
        }
        append(
            &mut journal,
            json!({"event":"complete","requests":plan.comparisons.len()}),
        );
        let report = analyze(&plan, &journal)?;
        assert_eq!(report["complete"], true);
        assert_eq!(report["certifies_model"], false);
        assert_eq!(report["unpaired_or_unrun_partitions"], 0);
        assert!(
            report["required_losses"]
                .as_array()
                .ok_or("losses")?
                .is_empty()
        );
        assert!(report.get("passed").is_none());
        append(
            &mut journal,
            json!({"event":"complete","requests":plan.comparisons.len()}),
        );
        assert!(analyze(&plan, &journal).is_err());
        Ok(())
    }

    #[test]
    fn report_rejects_drift_duplicates_uncheckpointed_and_false_completion() -> Result<()> {
        let plan = comparison::build_plan()?;
        let initial = header(&plan)?;
        for event in [
            response(&plan, 0, 0.8, 0.1),
            json!({"event":"complete","requests":0}),
            json!({"event":"stopped","index":0}),
        ] {
            let mut journal = initial.clone();
            append(&mut journal, event);
            assert!(analyze(&plan, &journal).is_err());
        }
        let mut journal = initial.clone();
        append(&mut journal, attempt(&plan, 0));
        let pending = analyze(&plan, &journal)?;
        assert_eq!(pending["unresolved_attempt"], 0);
        assert_eq!(pending["responses"], 0);
        append(&mut journal, response(&plan, 0, 0.8, 0.1));
        append(&mut journal, response(&plan, 0, 0.8, 0.1));
        assert!(analyze(&plan, &journal).is_err());
        assert!(
            analyze(
                &plan,
                &initial.replace("jev-framing-comparison/v4", "unknown")
            )
            .is_err()
        );
        Ok(())
    }
}
