use super::*;

pub(super) fn corpus() -> Result<Value> {
    let suite = tag_quality_suite()?;
    let input = suite
        .cases
        .iter()
        .find(|v| {
            v.track["context_evidence"]["sections"]
                .as_array()
                .is_some_and(|v| !v.is_empty())
        })
        .ok_or("audio fixture")?
        .track
        .clone();
    let records = [1, 2]
        .into_iter()
        .map(|id| {
            let mut input = input.clone();
            input["track_id"] = json!(id);
            json!({"input":input,"file_sha256":format!("{id:064x}"),
            "source_path":"PRIVATE-PATH-NEVER-SEND","display_name":"PRIVATE-TITLE-NEVER-SEND",
            "collection":"PRIVATE-COLLECTION-NEVER-SEND"})
        })
        .collect::<Vec<_>>();
    Ok(
        json!({"schema_version":CORPUS_SCHEMA,"analyzer_id":LOCAL_CONTEXT_ANALYZER_ID,
        "implementation_id":LOCAL_CONTEXT_IMPLEMENTATION_ID,"recordings":records}),
    )
}

#[test]
fn pilot_discloses_all_variants_full_pipeline_bounds_and_repeat_controls() -> Result<()> {
    let plan = build_plan(&corpus()?, &[1, 2])?;
    let cases = plan["cases"].as_array().ok_or("cases")?;
    assert_eq!(cases.len(), 8);
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
        assert!(!case["assessment"].to_string().contains("PRIVATE-"));
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
    assert!(build_plan(&corpus, &[]).is_err());
    assert!(build_plan(&corpus, &[1, 1]).is_err());
    assert!(build_plan(&corpus, &[999]).is_err());
    let mut stale = corpus.clone();
    stale["implementation_id"] = json!("old");
    assert!(build_plan(&stale, &[1]).is_err());
    let mut duplicate = corpus;
    duplicate["recordings"][1]["file_sha256"] = duplicate["recordings"][0]["file_sha256"].clone();
    assert!(build_plan(&duplicate, &[1, 2]).is_err());
    Ok(())
}

#[test]
fn compact_pilot_is_lossless_and_keeps_grounding_exact() -> Result<()> {
    let corpus = corpus()?;
    let input = &corpus["recordings"][0]["input"];
    let baseline = task(input, "current")?;
    let compact = task(input, "compact")?;
    for request in baseline.assessment_requests() {
        let transformed = assessment_request(request, "compact")?;
        assert_eq!(
            serde_json::to_value(&transformed.questions)?,
            serde_json::to_value(&request.questions)?
        );
        assert_eq!(transformed.state["coverage"], request.state["coverage"]);
        for (id, original) in request.state["observations"]
            .as_object()
            .ok_or("observations")?
        {
            let mut restored = transformed.state["observations"][id].clone();
            restored["id"] = json!(id);
            if original.get("band_scale").is_some() {
                restored["band_scale"] = transformed.state["shared_physical_band_scale"].clone();
            }
            assert_eq!(&restored, original);
        }
    }
    assert_eq!(
        serde_json::to_value(baseline.grounding_requests([0, 1])?)?,
        serde_json::to_value(compact.grounding_requests([0, 1])?)?
    );
    let reduced = task(input, "compact_without_sections")?;
    for request in reduced
        .assessment_requests()
        .iter()
        .chain(reduced.grounding_requests([0, 1])?.iter())
    {
        assert!(
            !request.state["observations"]
                .as_object()
                .ok_or("observations")?
                .keys()
                .any(|id| id.starts_with("audio.sections."))
        );
        assert!(
            request.state["observations"]
                .as_object()
                .ok_or("observations")?
                .keys()
                .any(|id| id.starts_with("audio.trajectories."))
        );
        assert!(!request.state["coverage"].is_null());
    }
    assert!(
        !input["context_evidence"]["sections"]
            .as_array()
            .ok_or("sections")?
            .is_empty()
    );
    Ok(())
}
