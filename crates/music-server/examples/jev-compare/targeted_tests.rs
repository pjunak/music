use super::*;

fn labels(state: &Value, classifier: &str) -> Result<Vec<String>> {
    state["observations"]
        .as_array()
        .ok_or("observations")?
        .iter()
        .find(|item| item["classifier"] == classifier)
        .ok_or("classifier")?["labels"]
        .as_array()
        .ok_or("labels")?
        .iter()
        .map(|item| Ok(item["label"].as_str().ok_or("label")?.to_owned()))
        .collect()
}

fn profile_labels(names: &[String], profile: usize) -> Vec<Value> {
    names
        .iter()
        .enumerate()
        .map(|(index, name)| {
            json!({"label":name,"response":0.1 + profile as f64 * 0.2 + index as f64 * 0.01})
        })
        .collect()
}

fn fixture() -> Result<(Value, Value)> {
    let baseline = graded::fixture_plan()?;
    let simple = simple::prepare(&baseline, &[1])?;
    let material = ablation::baseline_material(&baseline, &simple, 1)?;
    let instruments = labels(&material.state, "instrument")?;
    let styles = labels(&material.state, "style")?;
    let recording = &baseline["recordings"][0];
    let mut profiles = Vec::new();
    for (index, (start, end)) in [(0.0, 1.0 / 3.0), (1.0 / 3.0, 2.0 / 3.0), (2.0 / 3.0, 1.0)]
        .into_iter()
        .enumerate()
    {
        profiles.push(json!({
            "start_fraction":start,
            "end_fraction":end,
            "instrument":profile_labels(&instruments, index),
            "style":profile_labels(&styles, index),
        }));
    }
    let input = json!({
        "schema_version":input::INPUT_SCHEMA,
        "baseline_plan_sha256":fingerprint(&baseline)?,
        "provenance":{
            "physical_extractor_sha256":"a".repeat(64),
            "physical_dataset_sha256":"b".repeat(64),
            "temporal_extractor_sha256":"c".repeat(64),
            "temporal_dataset_sha256":"d".repeat(64),
            "details":{"local_diagnostic":"not sent"}
        },
        "records":[{
            "track_id":1,
            "file_sha256":recording["file_sha256"],
            "coverage_seconds":recording["input"]["length_s"],
            "beat":{"bpm_candidate":120.0,"pulse_support":0.8,
                "tempo_peak_margin":0.2,"beat_interval_cv":0.1,
                "accent_3_fit":0.2,"accent_4_fit":0.7},
            "harmony":{"roughness_mean":0.3,"roughness_p90":0.6,
                "key_template_fit":0.5,"major_minor_margin":-0.2,
                "tonal_change":0.4},
            "profiles":profiles
        }]
    });
    Ok((baseline, input))
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

fn remove_definitions(state: &mut Value) -> Result<()> {
    for family in state["targeted_numeric_cards"]
        .as_array_mut()
        .ok_or("numeric cards")?
    {
        for measurement in family["measurements"]
            .as_array_mut()
            .ok_or("measurements")?
        {
            measurement
                .as_object_mut()
                .ok_or("measurement")?
                .remove("definition");
        }
    }
    Ok(())
}

#[test]
fn arms_keep_questions_and_baseline_while_isolating_features() -> Result<()> {
    let (baseline, input) = fixture()?;
    let plan = prepare(&baseline, &input, &[1])?;
    assert_eq!(plan.comparisons.len(), 12);
    let baseline_request = request(&plan, BASELINE_ARM, false);
    assert_eq!(baseline_request.questions.len(), QUESTION_COUNT);
    for comparison in &plan.comparisons {
        assert_eq!(comparison.request.questions, baseline_request.questions);
        comparison.request.validate()?;
    }
    assert_eq!(
        baseline_request,
        request(&plan, BASELINE_ARM, true),
        "planned repeat must be byte-equivalent at the typed request level"
    );

    let all_numeric = request(&plan, "all_numeric", false);
    let mut described = request(&plan, "all_described", false).state.clone();
    remove_definitions(&mut described)?;
    assert_eq!(described, all_numeric.state);

    let mut expected_no_mood = all_numeric.state.clone();
    expected_no_mood["observations"]
        .as_array_mut()
        .ok_or("observations")?
        .retain(|item| item["classifier"] != "mood_theme");
    assert_eq!(
        request(&plan, "all_no_mood_head", false).state,
        expected_no_mood
    );

    let missing = request(&plan, "missing_all", false);
    assert_eq!(
        missing.state["observations"],
        baseline_request.state["observations"]
    );
    for family in missing.state["targeted_numeric_cards"]
        .as_array()
        .ok_or("cards")?
    {
        assert!(family["coverage_seconds"].is_null());
        for metric in family["measurements"].as_array().ok_or("metrics")? {
            assert!(metric["value"].is_null());
            assert_eq!(metric["availability"], "unavailable");
        }
    }
    for profile in missing.state["targeted_profile_card"]["profiles"]
        .as_array()
        .ok_or("profiles")?
    {
        for classifier in ["instrument", "style"] {
            for item in profile[classifier].as_array().ok_or("responses")? {
                assert!(!item["label"].as_str().ok_or("label")?.is_empty());
                assert!(item["response"].is_null());
            }
        }
    }
    assert!(missing.state["targeted_profile_card"]["coverage_seconds"].is_null());
    assert!(!serde_json::to_string(&plan.document)?.contains("local_diagnostic"));
    Ok(())
}

#[test]
fn profile_means_are_fraction_weighted_and_timeline_is_not_corroboration() -> Result<()> {
    let (baseline, input) = fixture()?;
    let plan = prepare(&baseline, &input, &[1])?;
    let means = &request(&plan, "profile_means", false).state["targeted_profile_card"];
    let response = means["instrument"][0]["response"]
        .as_f64()
        .ok_or("mean response")?;
    assert!((response - 0.3).abs() < 1e-12);
    assert!(means.get("profiles").is_none());
    let timeline = &request(&plan, "profile_timeline", false).state["targeted_profile_card"];
    assert_eq!(
        timeline["response_scale"],
        "uncalibrated_shared_encoder_response"
    );
    assert_eq!(timeline["independent_corroboration"], false);
    assert_eq!(timeline["profiles"].as_array().ok_or("profiles")?.len(), 3);
    Ok(())
}

#[test]
fn input_rejects_identity_shape_labels_gaps_and_ranges() -> Result<()> {
    let (baseline, input) = fixture()?;
    for (pointer, value) in [
        ("/records/0/file_sha256", json!("0".repeat(64))),
        (
            "/records/0/coverage_seconds",
            json!(
                input["records"][0]["coverage_seconds"]
                    .as_f64()
                    .ok_or("coverage")?
                    + 0.2
            ),
        ),
        ("/records/0/beat/bpm_candidate", json!(301.0)),
        ("/records/0/harmony/roughness_mean", json!(1.1)),
        ("/records/0/profiles/1/start_fraction", json!(0.4)),
        ("/records/0/profiles/0/instrument/0/label", json!("wrong")),
        ("/records/0/profiles/0/style/0/response", json!(-0.1)),
    ] {
        let mut malformed = input.clone();
        *malformed.pointer_mut(pointer).ok_or("fixture pointer")? = value;
        assert!(
            prepare(&baseline, &malformed, &[1]).is_err(),
            "accepted {pointer}"
        );
    }
    let mut extra = input.clone();
    extra["records"][0]["owner_comment"] = json!("private");
    assert!(prepare(&baseline, &extra, &[1]).is_err());
    let mut mood = input.clone();
    mood["records"][0]["profiles"][0]["mood"] = json!([]);
    assert!(prepare(&baseline, &mood, &[1]).is_err());
    let mut details = input.clone();
    details["provenance"]["details"] = json!({"oversized":"x".repeat(65 * 1024)});
    assert!(prepare(&baseline, &details, &[1]).is_err());
    Ok(())
}

#[test]
fn rotation_repeats_null_and_plan_authorization_are_exact() -> Result<()> {
    let (baseline, input) = fixture()?;
    let plan = prepare(&baseline, &input, &[1])?;
    for track_index in 0..13 {
        let rotated = (0..ARMS.len())
            .map(|offset| rotated_arm(track_index, offset))
            .collect::<Vec<_>>();
        let expected = ARMS
            .iter()
            .cycle()
            .skip(track_index % ARMS.len())
            .take(ARMS.len())
            .copied()
            .collect::<Vec<_>>();
        assert_eq!(rotated, expected);
    }
    assert_eq!(13 * (ARMS.len() + 1) + 1, 144);
    let mappings = plan.document["request_mappings"]
        .as_array()
        .ok_or("mappings")?;
    assert_eq!(
        mappings
            .iter()
            .filter(|item| item["repeat_control"] == true)
            .count(),
        1
    );
    assert_eq!(
        mappings
            .iter()
            .filter(|item| item["control"] == "null_evidence")
            .count(),
        1
    );
    let hash = fingerprint(&plan.document)?;
    let units = plan.document["max_input_units"].as_u64().ok_or("units")?;
    authorize(&plan, &plan.document, &hash, 12, units)?;
    assert!(authorize(&plan, &plan.document, &hash, 13, units).is_err());
    assert!(authorize(&plan, &plan.document, &hash, 12, units + 1).is_err());
    let mut changed = plan.document.clone();
    changed["comparisons"][0]["request"]["state"] = json!({"injected":true});
    assert!(authorize(&plan, &changed, &fingerprint(&changed)?, 12, units).is_err());
    Ok(())
}

fn event_line(journal: &mut String, value: Value) {
    journal.push_str(&value.to_string());
    journal.push('\n');
}

#[test]
fn report_requires_complete_nonreplayed_journal_and_preserves_unknown_usage() -> Result<()> {
    let (baseline, input) = fixture()?;
    let plan = prepare(&baseline, &input, &[1])?;
    let mut journal = format!("{}\n", json!({"event":"plan","plan":plan.document.clone()}));
    let header: Value = serde_json::from_str(journal.lines().next().ok_or("header")?)?;
    assert_eq!(header["plan"], plan.document);
    assert!(report_text(&plan, &journal).is_err());
    let mut stopped = journal.clone();
    event_line(
        &mut stopped,
        json!({"event":"attempt_started","index":0,"case_id":plan.comparisons[0].case_id,
            "variant":plan.comparisons[0].variant}),
    );
    event_line(&mut stopped, json!({"event":"stopped","index":0}));
    assert!(report_text(&plan, &stopped).is_err());
    event_line(
        &mut stopped,
        json!({"event":"attempt_started","index":0,"case_id":plan.comparisons[0].case_id,
            "variant":plan.comparisons[0].variant}),
    );
    assert!(report_text(&plan, &stopped).is_err());

    for (index, comparison) in plan.comparisons.iter().enumerate() {
        event_line(
            &mut journal,
            json!({"event":"attempt_started","index":index,"case_id":comparison.case_id,
                "variant":comparison.variant}),
        );
        let answers = comparison
            .request
            .questions
            .keys()
            .map(|id| (id.clone(), json!({"type":"noul","noul":0.5})))
            .collect::<serde_json::Map<_, _>>();
        event_line(
            &mut journal,
            json!({"event":"response","index":index,"result":{"model":MODEL,
                "answers":answers,"input_tokens":null,"output_tokens":null}}),
        );
    }
    event_line(
        &mut journal,
        json!({"event":"complete","requests":plan.comparisons.len()}),
    );
    let report = report_text(&plan, &journal)?;
    assert_eq!(report["complete"], true);
    assert_eq!(
        report["usage"]["missing_input_reports"],
        plan.comparisons.len()
    );
    assert_eq!(
        report["usage"]["missing_output_reports"],
        plan.comparisons.len()
    );
    assert!(report.get("winner").is_none() && report.get("passed").is_none());
    assert!(report["requests"][0]["raw_typed_answers"].is_object());
    Ok(())
}
