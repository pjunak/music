use super::*;

pub(super) fn recordings() -> Result<Vec<Value>> {
    let mut recording = pilot::tests::recording()?;
    recording["cohort"] = json!("development");
    let context = &mut recording["input"]["context_evidence"];
    context["coverage"] = json!({"scope":"whole_track","decoded_seconds":60.0});
    *context = music_application::assistant::compact_context_evidence(context);
    Ok(vec![recording])
}

#[test]
fn incomplete_or_malformed_physical_evidence_cannot_build_any_arm() -> Result<()> {
    let valid = recordings()?;
    build(&valid)?;
    let pointers = [
        "/input/context_evidence",
        "/input/context_evidence/trajectories/brightness",
        "/input/context_evidence/trajectories/brightness/end",
        "/input/context_evidence/measurement_reliability/spectral_flux",
        "/input/context_evidence/structure",
        "/input/context_evidence/structure/development",
        "/input/context_evidence/coverage",
    ];
    for pointer in pointers {
        let mut missing = valid.clone();
        *missing[0].pointer_mut(pointer).ok_or("fixture field")? = Value::Null;
        assert!(build(&missing).is_err(), "accepted absent {pointer}");
    }
    for (pointer, value) in [
        ("/input/context_evidence/completeness", json!("partial")),
        (
            "/input/context_evidence/coverage/decoded_seconds",
            json!(59.0),
        ),
        (
            "/input/context_evidence/trajectories/brightness/end",
            json!(1.1),
        ),
        (
            "/input/context_evidence/trajectories/brightness/shape",
            json!("unmeasured"),
        ),
        (
            "/input/context_evidence/measurement_reliability/density",
            json!("unavailable"),
        ),
        ("/input/context_evidence/structure/section_count", json!(0)),
    ] {
        let mut malformed = valid.clone();
        *malformed[0].pointer_mut(pointer).ok_or("fixture field")? = value;
        assert!(build(&malformed).is_err(), "accepted malformed {pointer}");
    }
    let mut injected = valid.clone();
    injected[0]["input"]["context_evidence"]["trajectories"]["brightness"]["source_path"] =
        json!("PRIVATE");
    assert!(build(&injected).is_err());
    Ok(())
}

#[test]
fn complete_vocabulary_and_controlled_recipes_exclude_private_and_owner_data() -> Result<()> {
    let mut recordings = recordings()?;
    for key in ["artist", "album", "genre", "origin", "owner_rating"] {
        recordings[0]["input"][key] = json!("PRIVATE-NEVER-SEND");
    }
    let plan = build(&recordings)?;
    for arm in ARMS {
        let cases = plan["cases"]
            .as_array()
            .ok_or("cases")?
            .iter()
            .filter(|c| c["arm"] == arm && c["repeat_control"] == false)
            .collect::<Vec<_>>();
        let mut ids = BTreeSet::new();
        for case in cases {
            let request = request(case)?;
            assert!(!serde_json::to_string(&request)?.contains("PRIVATE"));
            for id in request.questions.keys() {
                assert!(ids.insert(id.clone()));
            }
        }
        assert_eq!(ids.len(), 138);
        assert!(ids.contains("setting.medieval"));
    }
    let a = evidence(&recordings[0], "labels3")?;
    let b = evidence(&recordings[0], "labels8")?;
    for (small, large) in a["observations"]
        .as_array()
        .ok_or("small")?
        .iter()
        .zip(b["observations"].as_array().ok_or("large")?)
    {
        assert_eq!(
            small["labels"].as_array().ok_or("labels")?,
            &large["labels"].as_array().ok_or("labels")?[..3]
        );
    }
    let mut physical = evidence(&recordings[0], "labels8_physical")?;
    assert!(
        physical
            .as_object_mut()
            .ok_or("physical")?
            .remove("physical")
            .is_some()
    );
    assert_eq!(physical, b);
    let mut temporal = evidence(&recordings[0], "labels8_temporal")?;
    temporal
        .as_object_mut()
        .ok_or("temporal")?
        .remove("temporal_interpretation");
    for source in temporal["observations"].as_array_mut().ok_or("sources")? {
        for label in source["labels"].as_array_mut().ok_or("labels")? {
            for field in [
                "peak_response",
                "opening_response",
                "ending_response",
                "fraction_of_time_in_head_top3",
            ] {
                label.as_object_mut().ok_or("label")?.remove(field);
            }
        }
    }
    assert_eq!(temporal, b);
    Ok(())
}

#[test]
fn immutable_authorization_rejects_changed_budget_recipe_or_source_before_secret_access()
-> Result<()> {
    let plan = build(&recordings()?)?;
    let hash = fingerprint(&plan)?;
    let calls = plan["max_requests"].as_u64().ok_or("calls")? as usize;
    let units = plan["max_input_units"].as_u64().ok_or("units")?;
    authorize(&plan, &hash, calls, units)?;
    for (c, u) in [
        (calls + 1, units),
        (calls - 1, units),
        (calls, units + 1),
        (calls, units - 1),
    ] {
        assert!(authorize(&plan, &hash, c, u).is_err());
    }
    for (pointer, value) in [
        ("/rubric", json!("changed")),
        ("/display_cutoff", json!(0.7)),
        ("/cases/2/request/state", json!({"injected":"observation"})),
    ] {
        let mut changed = plan.clone();
        *changed.pointer_mut(pointer).ok_or("pointer")? = value;
        assert!(authorize(&changed, &fingerprint(&changed)?, calls, units).is_err());
    }
    Ok(())
}

#[test]
fn score_normalization_preserves_uncertainty_without_binary_gate_or_top_eight() -> Result<()> {
    let plan = build(&recordings()?)?;
    let case = &plan["cases"][2];
    let request = request(case)?;
    let answers = request
        .questions
        .keys()
        .map(|id| {
            (
                id.clone(),
                json!({"type":"score","score":1.2,
        "confidence":0.1,"probabilities":{"0":0.4,"1":0.0,"2":0.6,"3":0.0,"4":0.0}}),
            )
        })
        .collect::<serde_json::Map<_, _>>();
    let values = scores(case, json!(answers))?;
    assert_eq!(values.as_object().ok_or("values")?.len(), 25);
    for value in values.as_object().ok_or("values")?.values() {
        assert_eq!(value["relevance"], 0.3);
        assert_eq!(value["score_confidence"], 0.1);
        assert_eq!(value["level_probabilities"]["2"], 0.6);
    }
    Ok(())
}
