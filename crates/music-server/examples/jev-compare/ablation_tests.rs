use super::*;

fn fixture() -> Result<(Value, Value)> {
    let baseline = graded::fixture_plan()?;
    let recording = &baseline["recordings"][0];
    let style_source = recording["learned"]["sources"]
        .as_array()
        .ok_or("sources")?
        .iter()
        .find(|source| source["kind"] == "style")
        .ok_or("style source")?;
    let mut styles = style_source["labels"]
        .as_array()
        .ok_or("styles")?
        .iter()
        .map(|label| {
            Ok((
                label["label"].as_str().ok_or("label")?.to_owned(),
                label["mean_score"].as_f64().ok_or("mean")?,
            ))
        })
        .collect::<Result<Vec<_>>>()?;
    styles.sort_by(|(a_label, a), (b_label, b)| b.total_cmp(a).then_with(|| a_label.cmp(b_label)));
    let extra = &styles[8];
    let auxiliary = json!({
        "schema_version":"jev-audio-auxiliary/v1",
        "recordings":[{
            "track_id":1,
            "file_sha256":recording["file_sha256"],
            "danceability":{"mean_response":0.4,"low_response":0.1,
                "high_response":0.8,"coverage_seconds":60.0},
            "contrasts":[{"dimension":"instrumental_vs_sung","preference":"instrumental",
                "agreement":3,"margin":0.25}],
            "section_styles":[{"label":extra.0,"mean_response":auxiliary::rounded3(extra.1),
                "top3_section_count":2,"section_count":10}]
        }],
        "provenance":{"source_sha256":"a".repeat(64),"model_sha256":"b".repeat(64)}
    });
    Ok((baseline, auxiliary))
}

fn request<'a>(plan: &'a Plan, arm: &str, repeat: bool) -> &'a TypedDecisionRequest {
    let index = plan.document["request_mappings"]
        .as_array()
        .expect("mappings")
        .iter()
        .position(|mapping| {
            mapping["track_id"] == 1 && mapping["arm"] == arm && mapping["repeat_control"] == repeat
        })
        .expect("request mapping");
    &plan.comparisons[index].request
}

#[test]
fn arms_cover_all_tags_and_isolate_the_declared_factor() -> Result<()> {
    let (baseline, auxiliary) = fixture()?;
    let plan = prepare(&baseline, &auxiliary, &[1])?;
    let baseline_request = request(&plan, BASELINE_ARM, false);
    assert_eq!(baseline_request.questions.len(), QUESTION_COUNT);
    for comparison in &plan.comparisons {
        assert_eq!(comparison.request.questions.len(), QUESTION_COUNT);
        comparison.request.validate()?;
    }

    let no_mood = request(&plan, "no_mood_head", false);
    assert_eq!(no_mood.questions, baseline_request.questions);
    let mut expected = baseline_request.state.clone();
    expected["observations"]
        .as_array_mut()
        .ok_or("observations")?
        .retain(|observation| observation["classifier"] != "mood_theme");
    assert_eq!(no_mood.state, expected);

    let rank_only = request(&plan, "rank_only", false);
    assert_eq!(rank_only.questions, baseline_request.questions);
    for observation in rank_only.state["observations"]
        .as_array()
        .ok_or("observations")?
    {
        for label in observation["labels"].as_array().ok_or("labels")? {
            assert_eq!(label.as_object().ok_or("label")?.len(), 2);
            assert!(label.get("label").is_some() && label.get("rank").is_some());
        }
    }

    let compact = request(&plan, "compact_score", false);
    assert_eq!(compact.state, baseline_request.state);
    for (id, question) in &compact.questions {
        let TypedQuestion::Score {
            instructions,
            criteria,
        } = question
        else {
            return Err("compact arm must use Score".into());
        };
        assert_eq!(criteria.len(), 3);
        let TypedQuestion::Noul {
            instructions: old, ..
        } = &baseline_request.questions[id]
        else {
            return Err("baseline must use Noul".into());
        };
        assert_eq!(instructions["definition"], old["definition"]);
    }

    let specific = request(&plan, "specific_context", false);
    assert_eq!(specific.state, baseline_request.state);
    for (id, question) in &specific.questions {
        let group = plan.comparisons[0].tags[id].group.as_str();
        if ["mood", "period"].contains(&group) {
            assert_eq!(question, &baseline_request.questions[id]);
        } else {
            let (
                TypedQuestion::Noul { instructions, .. },
                TypedQuestion::Noul {
                    instructions: old, ..
                },
            ) = (question, &baseline_request.questions[id])
            else {
                return Err("specific questions must be Noul".into());
            };
            assert_ne!(instructions["question"], old["question"]);
            assert_eq!(instructions["definition"], old["definition"]);
        }
    }

    let contrast_body = serde_json::to_string(request(&plan, "plus_contrasts", false))?;
    assert!(!contrast_body.contains("three model heads"));
    assert!(contrast_body.contains("one independent audio/text model"));
    assert!(contrast_body.contains("correlated prompt variants, not three models"));
    for arm in ["plus_danceability", "plus_contrasts", "section_styles"] {
        let augmented = request(&plan, arm, false);
        assert_eq!(augmented.questions, baseline_request.questions);
        let mut state = augmented.state.clone();
        assert!(
            state
                .as_object_mut()
                .ok_or("state")?
                .remove("auxiliary_observations")
                .is_some()
        );
        assert_eq!(state, baseline_request.state);
    }
    Ok(())
}

#[test]
fn repeats_rotation_and_null_controls_are_explicit() -> Result<()> {
    let (baseline, auxiliary) = fixture()?;
    let plan = prepare(&baseline, &auxiliary, &[1])?;
    assert_eq!(plan.comparisons.len(), 11);
    assert_eq!(
        request(&plan, BASELINE_ARM, false),
        request(&plan, BASELINE_ARM, true)
    );
    let mappings = plan.document["request_mappings"]
        .as_array()
        .ok_or("mappings")?;
    assert_eq!(
        mappings
            .iter()
            .filter(|v| v["repeat_control"] == true)
            .count(),
        1
    );
    let nulls = mappings
        .iter()
        .filter(|v| v["control"] == "null_evidence")
        .collect::<Vec<_>>();
    assert_eq!(nulls.len(), 2);
    for mapping in nulls {
        assert!(mapping["track_id"].is_null());
        assert_eq!(
            plan.comparisons[mapping["request_index"].as_u64().ok_or("index")? as usize]
                .request
                .state,
            json!(NULL_STATE)
        );
    }
    assert_eq!(
        plan.document["request_count"],
        json!(1usize * ARMS.len() + 1 + 2)
    );

    let mut empty_auxiliary = auxiliary.clone();
    empty_auxiliary["recordings"][0]["section_styles"] = json!([]);
    let empty_plan = prepare(&baseline, &empty_auxiliary, &[1])?;
    assert_eq!(
        request(&empty_plan, "section_styles", false),
        request(&empty_plan, BASELINE_ARM, false)
    );
    let empty_mapping = empty_plan.document["request_mappings"]
        .as_array()
        .ok_or("mappings")?
        .iter()
        .find(|mapping| mapping["track_id"] == 1 && mapping["arm"] == "section_styles")
        .ok_or("section mapping")?;
    assert_eq!(empty_mapping["evidence_changed"], false);
    assert_eq!(empty_mapping["no_op"], true);
    Ok(())
}

#[test]
fn malformed_auxiliary_frozen_drift_and_caps_are_rejected() -> Result<()> {
    let (baseline, auxiliary) = fixture()?;
    let plan = prepare(&baseline, &auxiliary, &[1])?;
    let hash = fingerprint(&plan.document)?;
    let units = plan.document["max_input_units"].as_u64().ok_or("units")?;
    authorize(&plan, &plan.document, &hash, 11, units)?;
    assert!(authorize(&plan, &plan.document, &hash, 12, units).is_err());
    assert!(authorize(&plan, &plan.document, &hash, 11, units + 1).is_err());
    let mut changed = plan.document.clone();
    changed["comparisons"][0]["request"]["state"] = json!({"injected":true});
    assert!(authorize(&plan, &changed, &fingerprint(&changed)?, 11, units).is_err());

    for (pointer, value) in [
        ("/recordings/0/file_sha256", json!("0".repeat(64))),
        ("/recordings/0/danceability/mean_response", json!(1.1)),
        ("/recordings/0/contrasts/0/agreement", json!(2)),
        ("/recordings/0/contrasts/0/preference", json!("ambiguous")),
        ("/recordings/0/section_styles/0/section_count", json!(9)),
    ] {
        let mut malformed = auxiliary.clone();
        *malformed.pointer_mut(pointer).ok_or("fixture pointer")? = value;
        assert!(
            prepare(&baseline, &malformed, &[1]).is_err(),
            "accepted {pointer}"
        );
    }
    let mut extra = auxiliary.clone();
    extra["recordings"][0]["danceability"]["comment"] = json!("free form");
    assert!(prepare(&baseline, &extra, &[1]).is_err());
    Ok(())
}

#[test]
fn report_rejects_malformed_or_incomplete_journals() -> Result<()> {
    let (baseline, auxiliary) = fixture()?;
    let plan = prepare(&baseline, &auxiliary, &[1])?;
    let header = serde_json::to_string(&json!({"event":"plan","plan":plan.document}))?;
    assert!(report_text(&plan, &header).is_err());
    let wrong = serde_json::to_string(&json!({"event":"plan","plan":{"changed":true}}))?;
    assert!(report_text(&plan, &wrong).is_err());
    Ok(())
}
