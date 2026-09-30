use super::*;

pub(super) fn fixture() -> Result<(Value, Value)> {
    let baseline = graded::fixture_plan()?;
    let recording = &baseline["recordings"][0];
    let features = json!({
        "schema_version":feature_input::INPUT_SCHEMA,
        "baseline_plan_sha256":fingerprint(&baseline)?,
        "provenance":{
            "extractor":"fixture-extractor",
            "extractor_version":"1.0",
            "affect_model":"fixture-affect",
            "affect_model_sha256":"a".repeat(64),
            "affect_source_output_order":["valence","arousal"],
            "affect_source_scale":{"minimum":-1.0,"maximum":1.0},
            "affect_normalization":"linear_unit_interval",
            "details":{"library_versions":{"fixture":"1"},"coverage":"ten equal-time bins"}
        },
        "records":[{
            "track_id":1,
            "file_sha256":recording["file_sha256"],
            "coverage_seconds":60.0,
            "families":{
                "rhythm":{"onsets_per_second":2.5,"pulse_strength":0.55,"inter_onset_cv":0.4},
                "texture":{"harmonic_fraction":0.62,"percussive_fraction":0.38,
                    "chroma_concentration":0.45,"chroma_change":0.32,
                    "spectral_flatness":0.2,"spectral_centroid_hz":2450.0},
                "development":{"rms_iqr_db":9.0,"peak_to_median_rms_db":12.0,
                    "early_late_rms_db_delta":-2.0,
                    "early_late_spectral_centroid_hz_delta":250.0,
                    "segment_rms_db_relative_to_median":[-6.0,-4.0,-2.0,0.0,1.0,2.0,3.0,2.0,0.0,-1.0],
                    "segment_onsets_per_second":[1.0,1.5,2.0,2.5,3.0,3.5,3.0,2.5,2.0,1.5],
                    "segment_spectral_centroid_hz":[1200.0,1400.0,1600.0,1800.0,2000.0,2200.0,2400.0,2600.0,2800.0,3000.0]},
                "affect":{"arousal":0.7,"valence":0.35}
            }
        }]
    });
    Ok((baseline, features))
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

fn cards(request: &TypedDecisionRequest) -> &Vec<Value> {
    request.state["feature_observations"]
        .as_array()
        .expect("feature cards")
}

fn assert_metric_equivalence(raw: &Value, banded: &Value) -> Result<usize> {
    assert_eq!(raw["unit"], banded["unit"]);
    assert_eq!(raw["measurement"], banded["measurement"]);
    if raw["available"] == false {
        assert_eq!(banded["available"], false);
        return Ok(1);
    }
    if let Some(value) = raw["value"].as_f64() {
        let lower = banded["interval"]["minimum_inclusive"]
            .as_f64()
            .ok_or("interval lower")?;
        let upper = banded["interval"]["maximum_inclusive"]
            .as_f64()
            .ok_or("interval upper")?;
        assert!(lower <= value && value <= upper);
        return Ok(1);
    }
    let values = raw["values"].as_array().ok_or("raw segment values")?;
    let intervals = banded["intervals"]
        .as_array()
        .ok_or("banded segment intervals")?;
    assert_eq!(values.len(), intervals.len());
    for (value, interval) in values.iter().zip(intervals) {
        let value = value.as_f64().ok_or("segment value")?;
        let lower = interval["minimum_inclusive"]
            .as_f64()
            .ok_or("segment lower")?;
        let upper = interval["maximum_inclusive"]
            .as_f64()
            .ok_or("segment upper")?;
        assert!(lower <= value && value <= upper);
    }
    Ok(values.len())
}

#[test]
fn arms_isolate_feature_families_and_preserve_the_frozen_baseline() -> Result<()> {
    let (baseline, features) = fixture()?;
    let plan = prepare(&baseline, &features, &[1])?;
    assert_eq!(plan.comparisons.len(), 14);
    let baseline_request = request(&plan, BASELINE_ARM, false);
    assert_eq!(baseline_request.questions.len(), QUESTION_COUNT);
    assert_eq!(
        baseline_request,
        request(&plan, BASELINE_ARM, true),
        "planned repeat must be byte-equivalent at the typed request level"
    );
    for comparison in &plan.comparisons {
        assert_eq!(comparison.request.questions.len(), QUESTION_COUNT);
        comparison.request.validate()?;
        assert!(
            serde_json::to_vec(&json!({"model":MODEL,"state":comparison.request.state,
                "questions":comparison.request.questions}))?
            .len()
                <= MAX_BODY_BYTES
        );
    }
    for (arm, expected_families) in [
        ("rhythm", vec!["rhythm"]),
        ("texture", vec!["texture"]),
        ("development", vec!["development"]),
        ("affect", vec!["affect"]),
        (
            "combined_acoustics",
            vec!["rhythm", "texture", "development"],
        ),
        (
            "combined_all",
            vec!["rhythm", "texture", "development", "affect"],
        ),
    ] {
        let candidate = request(&plan, arm, false);
        assert_eq!(candidate.questions, baseline_request.questions);
        assert_eq!(
            cards(candidate)
                .iter()
                .map(|card| card["family"].as_str().unwrap_or_default())
                .collect::<Vec<_>>(),
            expected_families
        );
        let mut state = candidate.state.clone();
        state
            .as_object_mut()
            .ok_or("state object")?
            .remove("feature_observations");
        assert_eq!(state, baseline_request.state);
    }
    let combined = request(&plan, "combined_all", false);
    let no_mood = request(&plan, "combined_all_no_mood_head", false);
    assert_eq!(no_mood.questions, combined.questions);
    assert_eq!(cards(no_mood), cards(combined));
    let mut expected_no_mood = combined.state.clone();
    expected_no_mood["observations"]
        .as_array_mut()
        .ok_or("combined observations")?
        .retain(|observation| observation["classifier"] != "mood_theme");
    let mut actual_no_mood = no_mood.state.clone();
    assert!(
        actual_no_mood["feature_scope"]
            .as_str()
            .is_some_and(|scope| scope.contains("unknown, not negative evidence"))
    );
    actual_no_mood
        .as_object_mut()
        .ok_or("no-mood state")?
        .remove("feature_scope");
    assert_eq!(actual_no_mood, expected_no_mood);

    let features_only = request(&plan, "features_only", false);
    assert_eq!(features_only.questions, combined.questions);
    assert_eq!(cards(features_only), cards(combined));
    assert_eq!(features_only.state["observations"], json!([]));
    assert_eq!(
        features_only.state["interpretation"],
        baseline_request.state["interpretation"]
    );
    assert!(
        features_only.state["feature_scope"]
            .as_str()
            .is_some_and(|scope| scope.contains("unknown, not negative evidence"))
    );
    for (arm, change) in [
        ("combined_all_no_mood_head", "remove_mood_theme"),
        ("features_only", "remove_all_learned_observations"),
    ] {
        let mapping = plan.document["request_mappings"]
            .as_array()
            .ok_or("mappings")?
            .iter()
            .find(|mapping| mapping["track_id"] == 1 && mapping["arm"] == arm)
            .ok_or("interaction mapping")?;
        assert_eq!(mapping["baseline_observation_change"], change);
    }
    let provider_bodies = serde_json::to_string(&plan.comparisons)?;
    assert!(!provider_bodies.contains("fixture-extractor"));
    assert!(!provider_bodies.contains(&"a".repeat(64)));
    let null = plan.document["request_mappings"]
        .as_array()
        .ok_or("mappings")?
        .iter()
        .find(|mapping| mapping["control"] == "null_evidence")
        .ok_or("null control")?;
    assert!(null["track_id"].is_null());
    assert_eq!(
        plan.comparisons[null["request_index"].as_u64().ok_or("null index")? as usize]
            .request
            .state,
        json!(NULL_STATE)
    );
    Ok(())
}

#[test]
fn band_arms_preserve_every_datapoint_as_a_numeric_interval() -> Result<()> {
    let (baseline, features) = fixture()?;
    let plan = prepare(&baseline, &features, &[1])?;
    let raw = cards(request(&plan, "combined_all", false));
    let banded = cards(request(&plan, "combined_all_bands", false));
    assert_eq!(raw.len(), banded.len());
    let mut datapoints = 0;
    for (raw_card, banded_card) in raw.iter().zip(banded) {
        assert_eq!(raw_card["family"], banded_card["family"]);
        assert_eq!(raw_card["representation"], "raw_numeric");
        assert_eq!(banded_card["representation"], "fixed_numeric_intervals");
        let raw_metrics = raw_card["metrics"].as_object().ok_or("raw metrics")?;
        let banded_metrics = banded_card["metrics"].as_object().ok_or("banded metrics")?;
        assert_eq!(
            raw_metrics.keys().collect::<Vec<_>>(),
            banded_metrics.keys().collect::<Vec<_>>()
        );
        for (id, metric) in raw_metrics {
            datapoints += assert_metric_equivalence(metric, &banded_metrics[id])?;
        }
    }
    assert_eq!(datapoints, 45);
    Ok(())
}

#[test]
fn unavailable_inter_onset_variability_stays_explicit_in_both_representations() -> Result<()> {
    let (baseline, mut features) = fixture()?;
    features["records"][0]["families"]["rhythm"]["inter_onset_cv"] = Value::Null;
    let plan = prepare(&baseline, &features, &[1])?;
    for arm in ["rhythm", "combined_all_bands"] {
        let rhythm = cards(request(&plan, arm, false))
            .iter()
            .find(|card| card["family"] == "rhythm")
            .ok_or("rhythm card")?;
        let metric = &rhythm["metrics"]["inter_onset_cv"];
        assert_eq!(metric["available"], false);
        assert!(metric.get("value").is_none() && metric.get("interval").is_none());
    }
    Ok(())
}

#[test]
fn unavailable_chroma_stays_explicit_for_insufficient_harmonic_content() -> Result<()> {
    let (baseline, mut features) = fixture()?;
    let texture = &mut features["records"][0]["families"]["texture"];
    texture["harmonic_fraction"] = json!(0.02);
    texture["percussive_fraction"] = json!(0.98);
    texture["chroma_concentration"] = Value::Null;
    texture["chroma_change"] = Value::Null;
    let plan = prepare(&baseline, &features, &[1])?;
    for arm in ["texture", "combined_all_bands"] {
        let texture = cards(request(&plan, arm, false))
            .iter()
            .find(|card| card["family"] == "texture")
            .ok_or("texture card")?;
        for id in ["chroma_concentration", "chroma_change"] {
            let metric = &texture["metrics"][id];
            assert_eq!(metric["available"], false);
            assert!(metric.get("value").is_none() && metric.get("interval").is_none());
        }
    }
    Ok(())
}

#[test]
fn malformed_values_missing_families_and_plan_drift_are_rejected() -> Result<()> {
    let (baseline, features) = fixture()?;
    let plan = prepare(&baseline, &features, &[1])?;
    let hash = fingerprint(&plan.document)?;
    let units = plan.document["max_input_units"].as_u64().ok_or("units")?;
    authorize(&plan, &plan.document, &hash, 14, units)?;
    assert!(authorize(&plan, &plan.document, &hash, 15, units).is_err());
    assert!(authorize(&plan, &plan.document, &hash, 14, units + 1).is_err());
    let mut changed = plan.document.clone();
    changed["comparisons"][0]["request"]["state"] = json!({"injected":true});
    assert!(authorize(&plan, &changed, &fingerprint(&changed)?, 14, units).is_err());

    for (pointer, value) in [
        ("/baseline_plan_sha256", json!("0".repeat(64))),
        ("/records/0/file_sha256", json!("0".repeat(64))),
        ("/records/0/coverage_seconds", json!(59.0)),
        ("/records/0/families/rhythm/onsets_per_second", json!(20.1)),
        ("/records/0/families/rhythm/pulse_strength", Value::Null),
        ("/records/0/families/texture/harmonic_fraction", json!(0.2)),
        ("/records/0/families/affect/arousal", json!(-0.1)),
        (
            "/records/0/families/development/segment_onsets_per_second",
            json!([1.0, 1.0, 1.0, 1.0, 1.0, 1.0, 1.0, 1.0, 1.0]),
        ),
    ] {
        let mut malformed = features.clone();
        *malformed.pointer_mut(pointer).ok_or("fixture pointer")? = value;
        assert!(
            prepare(&baseline, &malformed, &[1]).is_err(),
            "accepted {pointer}"
        );
    }
    let mut missing = features.clone();
    missing["records"][0]["families"]
        .as_object_mut()
        .ok_or("families")?
        .remove("affect");
    assert!(prepare(&baseline, &missing, &[1]).is_err());
    let mut extra = features.clone();
    extra["records"][0]["families"]["rhythm"]["mood"] = json!("calm");
    assert!(prepare(&baseline, &extra, &[1]).is_err());
    Ok(())
}

#[test]
fn complete_export_is_strict_and_keeps_raw_answers() -> Result<()> {
    let (baseline, features) = fixture()?;
    let plan = prepare(&baseline, &features, &[1])?;
    let mut journal_text = format!("{}\n", json!({"event":"plan","plan":plan.document}));
    assert!(report_text(&plan, &journal_text).is_err());
    for (index, item) in plan.comparisons.iter().enumerate() {
        journal_text.push_str(&format!(
            "{}\n",
            json!({"event":"attempt_started","index":index,"case_id":item.case_id,"variant":item.variant})
        ));
        let answers = item
            .request
            .questions
            .keys()
            .map(|id| (id.clone(), json!({"type":"noul","noul":0.5})))
            .collect::<BTreeMap<_, _>>();
        journal_text.push_str(&format!(
            "{}\n",
            json!({"event":"response","index":index,"result":{"model":MODEL,"answers":answers,
                "input_tokens":if index == 0 {None} else {Some(50)},"output_tokens":10}})
        ));
    }
    journal_text.push_str(&format!(
        "{}\n",
        json!({"event":"complete","requests":plan.comparisons.len()})
    ));
    let report = report_text(&plan, &journal_text)?;
    assert_eq!(report["complete"], true);
    assert_eq!(report["responses"], plan.comparisons.len());
    assert_eq!(report["usage"]["missing_input_reports"], 1);
    assert_eq!(
        report["requests"][0]["raw_typed_answers"]
            .as_object()
            .ok_or("answers")?
            .len(),
        QUESTION_COUNT
    );
    assert!(report.get("winner").is_none() && report.get("passed").is_none());
    Ok(())
}
