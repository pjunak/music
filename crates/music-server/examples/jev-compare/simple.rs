//! Short yes/no questions on exactly the same evidence as a frozen graded pilot.
use super::*;
use music_application::assistant::TypedQuestion;
use std::collections::{BTreeMap, BTreeSet};

const ARM: &str = "simple_noul";
const BASELINE_ARM: &str = "labels8";
const MAX_CALLS: usize = 112;
const MAX_UNITS: u64 = 4_000_000;

pub(super) struct Plan {
    pub document: Value,
    pub comparisons: Vec<comparison::Comparison>,
}

fn question(group: &str, tag: &Value) -> Result<TypedQuestion> {
    let name = tag["name"].as_str().ok_or("tag name")?;
    let text = match group {
        "mood" => format!("Is {name} a fitting description of this music?"),
        "scene" => format!("Is this music fitting accompaniment for {name}?"),
        "setting" => format!("Is {name} a fitting setting evoked by this music?"),
        "period" => format!("Is {name} a fitting musical style or period for this music?"),
        _ => return Err("unknown tag group".into()),
    };
    Ok(TypedQuestion::Noul {
        instructions: json!({"question":text,"definition":tag["definition"]}),
        criteria: BTreeMap::new(),
    })
}

pub(super) fn prepare(baseline: &Value, tracks: &[u64]) -> Result<Plan> {
    graded::authorize(
        baseline,
        &fingerprint(baseline)?,
        baseline["max_requests"]
            .as_u64()
            .ok_or("baseline requests")? as usize,
        baseline["max_input_units"]
            .as_u64()
            .ok_or("baseline units")?,
    )?;
    let selected = tracks.iter().copied().collect::<BTreeSet<_>>();
    if tracks.is_empty() || tracks.len() > 16 || selected.len() != tracks.len() {
        return Err("select 1..16 distinct baseline track ids".into());
    }
    let recordings = baseline["recordings"]
        .as_array()
        .ok_or("recordings")?
        .iter()
        .filter(|r| {
            r["input"]["track_id"]
                .as_u64()
                .is_some_and(|id| selected.contains(&id))
        })
        .map(|r| {
            json!({"track_id":r["input"]["track_id"],"display_name":r["display_name"],
            "source_path":r["source_path"],"file_sha256":r["file_sha256"]})
        })
        .collect::<Vec<_>>();
    if recordings.len() != tracks.len() {
        return Err("selected track missing from baseline".into());
    }
    let mut comparisons = Vec::new();
    let mut pairs = Vec::new();
    for (baseline_case, case) in baseline["cases"]
        .as_array()
        .ok_or("baseline cases")?
        .iter()
        .enumerate()
    {
        let Some(track) = case["track_id"].as_u64() else {
            continue;
        };
        if !selected.contains(&track)
            || case["arm"] != BASELINE_ARM
            || case["repeat_control"] != false
        {
            continue;
        }
        let group = case["group"].as_str().ok_or("group")?;
        let old = TypedDecisionRequest {
            state: case["request"]["state"].clone(),
            questions: serde_json::from_value(case["request"]["questions"].clone())?,
        };
        let mut request = TypedDecisionRequest {
            state: old.state,
            questions: BTreeMap::new(),
        };
        let mut tags = BTreeMap::new();
        for (id, original) in old.questions {
            let TypedQuestion::Score { instructions, .. } = original else {
                return Err("baseline must use Score".into());
            };
            let tag = &instructions["tag"];
            request.questions.insert(id.clone(), question(group, tag)?);
            tags.insert(
                id,
                comparison::FitTarget {
                    tag: tag["name"].as_str().ok_or("tag name")?.into(),
                    group: group.into(),
                    required: false,
                    forbidden: false,
                },
            );
        }
        request.validate()?;
        comparisons.push(comparison::Comparison {
            case_id: format!("track-{track}"),
            partition: baseline_case,
            variant: ARM,
            fit_threshold: 0.5,
            tags,
            request,
        });
        pairs.push(json!({"baseline_case":baseline_case,"track_id":track,"group":group}));
    }
    let units = comparisons
        .iter()
        .map(|c| model_request_reservation(&c.request.accounting_request(), 0))
        .sum::<u64>();
    if comparisons.len() != tracks.len() * 7 || comparisons.len() > MAX_CALLS || units > MAX_UNITS {
        return Err("simple pilot exceeds fixed coverage or budget bounds".into());
    }
    let document = json!({"schema_version":"jev-simple-noul-pilot/v1","model":MODEL,"endpoint":ENDPOINT,
        "baseline_plan_sha256":fingerprint(baseline)?,"baseline_arm":BASELINE_ARM,"arm":ARM,
        "selected_tracks":selected,"recordings":recordings,"vocabulary":baseline["vocabulary"],
        "comparisons":comparisons,"pairs":pairs,"request_count":comparisons.len(),"max_input_units":units,
        "certifies_model":false,"score_semantics":"Noul is the model probability of answering yes, not mood strength. No automatic tag cutoff or writes.",
        "comparison_limits":"Historical Score baseline, not a contemporaneous repeat. Question wording and primitive both change. Evidence, tag definitions, partitions and pinned model are held fixed.",
        "disclosure":"Audio-derived observations only; no audio, song identity, paths, owner ratings or library writes. No retries or automatic resume."});
    Ok(Plan {
        document,
        comparisons,
    })
}

fn authorize(plan: &Plan, saved: &Value, expected: &str, count: usize, units: u64) -> Result<()> {
    if &plan.document != saved
        || fingerprint(saved)? != expected
        || saved["request_count"].as_u64() != Some(count as u64)
        || saved["max_input_units"].as_u64() != Some(units)
    {
        return Err("simple pilot differs from exact reviewed plan or budget".into());
    }
    Ok(())
}

pub(super) fn load_authorized(
    baseline: &Path,
    saved: &Path,
    expected: &str,
    count: usize,
    units: u64,
) -> Result<Plan> {
    let saved = pilot::read_json(saved)?;
    let tracks: Vec<u64> = serde_json::from_value(saved["selected_tracks"].clone())?;
    let plan = prepare(&pilot::read_json(baseline)?, &tracks)?;
    authorize(&plan, &saved, expected, count, units)?;
    for recording in plan.document["recordings"].as_array().ok_or("recordings")? {
        if pilot::audio_hash(Path::new(recording["source_path"].as_str().ok_or("path")?))?
            != recording["file_sha256"]
        {
            return Err("original changed since baseline analysis".into());
        }
    }
    Ok(plan)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn baseline() -> Result<Value> {
        graded::fixture_plan()
    }

    #[test]
    fn simple_questions_keep_evidence_and_definitions_without_criteria() -> Result<()> {
        let baseline = baseline()?;
        let plan = prepare(&baseline, &[1])?;
        assert_eq!(plan.comparisons.len(), 7);
        let mut ids = BTreeSet::new();
        for item in &plan.comparisons {
            let old = &baseline["cases"][item.partition]["request"];
            assert_eq!(item.request.state, old["state"]);
            for (id, q) in &item.request.questions {
                assert!(ids.insert(id));
                let value = serde_json::to_value(q)?;
                assert_eq!(value["type"], "noul");
                assert!(value.get("criteria").is_none());
                assert_eq!(
                    value["instructions"]["definition"],
                    old["questions"][id]["instructions"]["tag"]["definition"]
                );
                assert!(!serde_json::to_string(q)?.contains("Learned labels"));
            }
        }
        assert_eq!(ids.len(), 138);
        assert!(prepare(&baseline, &[1, 1]).is_err());
        assert!(prepare(&baseline, &[99]).is_err());
        assert!(prepare(&baseline, &[]).is_err());
        Ok(())
    }

    #[test]
    fn frozen_questions_and_exact_budget_cannot_be_changed() -> Result<()> {
        let baseline = baseline()?;
        let plan = prepare(&baseline, &[1])?;
        let hash = fingerprint(&plan.document)?;
        let units = plan.document["max_input_units"].as_u64().ok_or("units")?;
        authorize(&plan, &plan.document, &hash, 7, units)?;
        assert!(authorize(&plan, &plan.document, &hash, 8, units).is_err());
        assert!(authorize(&plan, &plan.document, &hash, 7, units + 1).is_err());
        let mut changed = plan.document.clone();
        changed["comparisons"][0]["request"]["state"] = json!({"title":"injected"});
        assert!(authorize(&plan, &changed, &fingerprint(&changed)?, 7, units).is_err());
        Ok(())
    }
}
