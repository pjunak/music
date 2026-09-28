use super::*;

pub(crate) fn recording() -> Result<Value> {
    let suite = tag_quality_suite()?;
    let mut input = suite
        .cases
        .iter()
        .find(|v| {
            v.track["context_evidence"]["sections"]
                .as_array()
                .is_some_and(|v| !v.is_empty())
        })
        .ok_or("fixture with sections")?
        .track
        .clone();
    input["track_id"] = json!(1);
    input["length_s"] = json!(60.0);
    let classes: Value = serde_json::from_str(include_str!("fixtures/audio-taxonomies.json"))?;
    let sources = predictions::MODELS.iter().map(|(kind,model,graph,taxonomy,_,_)| {
        let labels = classes[kind].as_array().ok_or("fixture labels")?.iter().map(|label| json!({"label":label,
            "mean_score":0.2,"max_score":0.5,"top_rank_fraction":0.1,"opening_score":0.15,"ending_score":0.25}))
            .collect::<Vec<_>>();
        Ok(json!({"kind":kind,"model_id":model,"model_sha256":graph,"taxonomy_sha256":taxonomy,
            "score_kind":"uncalibrated_sigmoid","duration_seconds":60.0,"covered_seconds":60.0,
            "top_rank_limit":3,"labels":labels}))
    }).collect::<Result<Vec<_>>>()?;
    let hash = format!("{:064x}", 1);
    Ok(
        json!({"input":input,"file_sha256":hash,"source_path":"PRIVATE-PATH-NEVER-SEND",
        "display_name":"PRIVATE-TITLE-NEVER-SEND","collection":"PRIVATE-COLLECTION-NEVER-SEND",
        "learned":{"file_sha256":hash,"pcm_sha256":"a".repeat(64),"duration_seconds":60.0,"covered_seconds":60.0,"sources":sources}}),
    )
}

pub(super) fn corpus() -> Result<Value> {
    let first = recording()?;
    let mut second = first.clone();
    second["input"]["track_id"] = json!(2);
    second["file_sha256"] = json!(format!("{:064x}", 2));
    second["learned"]["file_sha256"] = second["file_sha256"].clone();
    Ok(
        json!({"schema_version":CORPUS_SCHEMA,"analyzer_id":LOCAL_CONTEXT_ANALYZER_ID,
        "implementation_id":LOCAL_CONTEXT_IMPLEMENTATION_ID,"recordings":[first,second]}),
    )
}

#[test]
fn pilot_discloses_all_variants_full_pipeline_bounds_and_repeat_controls() -> Result<()> {
    let plan = build_plan(&corpus()?, &[1, 2])?;
    let cases = plan["cases"].as_array().ok_or("cases")?;
    assert_eq!(cases.len(), 18);
    assert_eq!(
        cases.iter().filter(|v| v["repeat_control"] == true).count(),
        2
    );
    let calls = 1 + cases
        .iter()
        .map(|c| c["max_requests"].as_u64().unwrap_or(0))
        .sum::<u64>();
    let units = model_request_reservation(
        &typed_conformance_request("private-pilot").accounting_request(),
        0,
    ) + cases
        .iter()
        .map(|c| c["max_input_units"].as_u64().unwrap_or(0))
        .sum::<u64>();
    assert_eq!(plan["max_requests"], calls);
    assert_eq!(plan["max_input_units"], units);
    assert_eq!(plan["certifies_model"], false);
    for case in cases {
        let body = case["assessment"].to_string();
        assert!(!body.contains("PRIVATE-"));
        assert!(!body.contains("file_sha256"));
        assert!(!body.contains("model_sha256"));
        assert!(
            case["max_requests"].as_u64().unwrap_or(0)
                > case["assessment"].as_array().ok_or("assessment")?.len() as u64
        );
    }
    Ok(())
}

#[test]
fn pilot_requires_exact_budget_identity_membership_and_current_analysis() -> Result<()> {
    let corpus = corpus()?;
    let plan = build_plan(&corpus, &[1, 2])?;
    let hash = fingerprint(&plan)?;
    let calls = plan["max_requests"].as_u64().ok_or("calls")? as usize;
    let units = plan["max_input_units"].as_u64().ok_or("units")?;
    authorize(&plan, &hash, calls, units)?;
    assert!(authorize(&plan, &hash, calls + 1, units).is_err());
    assert!(authorize(&plan, &hash, calls, units + 1).is_err());
    assert!(authorize(&plan, "different", calls, units).is_err());
    let mut altered = plan.clone();
    altered["inference_identity"] = json!("different");
    assert!(authorize(&altered, &fingerprint(&altered)?, calls, units).is_err());
    for tracks in [vec![], vec![1, 1], vec![999]] {
        assert!(build_plan(&corpus, &tracks).is_err());
    }
    let mut stale = corpus.clone();
    stale["implementation_id"] = json!("old");
    assert!(build_plan(&stale, &[1]).is_err());
    Ok(())
}

#[test]
fn predictions_require_exact_hash_coverage_models_labels_and_all_scores() -> Result<()> {
    let valid = recording()?;
    predictions::validate_recording(&valid)?;
    for (pointer, value) in [
        ("/learned/file_sha256", json!("different")),
        ("/learned/pcm_sha256", json!(null)),
        ("/learned/covered_seconds", json!(59.0)),
        ("/learned/duration_seconds", json!(61.0)),
        ("/learned/sources/0/model_sha256", json!("different")),
        ("/learned/sources/0/taxonomy_sha256", json!("different")),
        ("/learned/sources/0/labels/39/mean_score", json!(-0.1)),
        ("/learned/sources/0/labels/39/mean_score", json!(null)),
        ("/learned/sources/0/labels/39/ending_score", json!(null)),
        ("/learned/sources/0/labels/39/label", json!("renamed")),
    ] {
        let mut changed = valid.clone();
        *changed.pointer_mut(pointer).ok_or("test pointer")? = value;
        assert!(
            predictions::validate_recording(&changed).is_err(),
            "{pointer}"
        );
    }
    let mut reordered = valid;
    reordered["learned"]["sources"][0]["labels"]
        .as_array_mut()
        .ok_or("fixture labels")?
        .swap(0, 1);
    assert!(predictions::validate_recording(&reordered).is_err());
    Ok(())
}
