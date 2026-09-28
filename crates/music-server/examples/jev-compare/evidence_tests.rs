use super::*;

fn state() -> Value {
    json!({"coverage":{"ending_included":true},"completeness":"full","observations":{
        "metadata.album":{"id":"metadata.album","meaning":"Supplied album title or description.","value":"Escape"},
        "metadata.genre":{"id":"metadata.genre","meaning":"Supplied genre or musical-style description.","value":"urgent orchestral"},
        "audio.sections.s2":{"id":"audio.sections.s2","meaning":"Ending section","measurement_reliability":{"structure":"low"},"band_scale":"Physical thirds","physical_bands":{"rhythmic_drive":"high"},"value":{"id":"s2","start_fraction":0.8,"end_fraction":1.0,"rhythmic_drive":0.9,"brightness":null}},
        "audio.trajectories.rhythmic_drive":{"id":"audio.trajectories.rhythmic_drive","meaning":"Onset activity","measurement_reliability":"medium","band_scale":"Physical thirds","physical_bands":{"end":"high"},"value":{"end":0.9,"reliability":null}},
        "audio.voice":{"id":"audio.voice","meaning":"Uncalibrated voice estimate","value":{"score":0.4}}
    }})
}

#[test]
fn evidence_transformations_preserve_ending_reliability_and_source_meaning() -> Result<()> {
    let original = state();
    let compact = variants::transform("compact_cards", &original, &json!({}))?;
    let mut restored = compact.clone();
    let scale = restored
        .as_object_mut()
        .ok_or("object")?
        .remove("shared_physical_band_scale")
        .ok_or("scale")?;
    for (id, card) in restored["observations"].as_object_mut().ok_or("cards")? {
        card["id"] = json!(id);
        if card.get("physical_bands").is_some() {
            card["band_scale"] = scale.clone();
        }
    }
    assert_eq!(restored, original);
    let bands = variants::transform("bands_only", &original, &json!({}))?;
    let ending = &bands["observations"]["audio.sections.s2"];
    assert_eq!(ending["value"]["rhythmic_drive"], "high");
    assert_eq!(ending["value"]["start_fraction"], 0.8);
    assert_eq!(ending["value"]["end_fraction"], 1.0);
    assert_eq!(ending["value"]["brightness"], Value::Null);
    assert_eq!(ending["measurement_reliability"]["structure"], "low");
    let numeric = variants::transform("numeric_only", &original, &json!({}))?;
    assert!(
        numeric["observations"]["audio.sections.s2"]
            .get("physical_bands")
            .is_none()
    );
    assert_eq!(
        numeric["observations"]["audio.sections.s2"]["value"],
        original["observations"]["audio.sections.s2"]["value"]
    );
    let repeated = variants::transform("repeat_cards_x3", &original, &json!({}))?;
    assert_eq!(
        repeated["duplicate_context"]["copies"][0],
        original["observations"]
    );
    assert_eq!(
        repeated["duplicate_context"]["copies"][1],
        original["observations"]
    );
    assert_eq!(repeated["observations"], original["observations"]);
    let catalog = variants::transform("genre_as_catalog", &original, &json!({}))?;
    assert!(catalog["observations"].get("metadata.genre").is_none());
    assert_eq!(
        catalog["observations"]["catalog.musicbrainz.genres"]["value"]["value"][0],
        original["observations"]["metadata.genre"]["value"]
    );
    let community = variants::transform("genre_as_community", &original, &json!({}))?;
    assert_eq!(
        community["observations"]["catalog.lastfm.community_tags"]["value"]["kind"],
        "weak_community_labels"
    );
    assert_eq!(
        community["observations"]["catalog.lastfm.community_tags"]["value"]["value"][0]["count"],
        1
    );
    assert!(community.to_string().find("verified").is_none());
    let identity = variants::transform(
        "add_identity_context",
        &original,
        &json!({"artist":"Temple","origin":"Castle","length_s":180,"bpm":90,"filename":"secret.mp3","required_tags":["heroic"]}),
    )?;
    assert_eq!(
        identity["observations"]["metadata.artist"]["value"],
        "Temple"
    );
    assert!(
        !identity.to_string().contains("secret.mp3") && !identity.to_string().contains("heroic")
    );
    assert_eq!(identity["coverage"], original["coverage"]);
    Ok(())
}

#[test]
fn evidence_plan_preserves_all_questions_and_requires_exact_approval() -> Result<()> {
    let plan = build_plan()?;
    let original = comparison::build_plan()?;
    assert_eq!(plan.cases.as_array().ok_or("cases")?.len(), 66);
    for case in tag_quality_suite()?.cases {
        let expected = original
            .comparisons
            .iter()
            .filter(|v| v.case_id == case.id && v.variant == CURRENT)
            .flat_map(|v| v.request.questions.clone())
            .collect::<BTreeMap<_, _>>();
        let actual = plan
            .contexts
            .iter()
            .filter(|v| v.case_id == case.id)
            .flat_map(|v| plan.comparisons[v.baseline].request.questions.clone())
            .collect::<BTreeMap<_, _>>();
        assert_eq!(expected, actual);
    }
    let mut unique = std::collections::BTreeSet::new();
    for request in plan.comparisons.iter().filter(|v| v.variant != REPEAT) {
        assert!(unique.insert(serde_json::to_string(&request.request)?));
    }
    assert_eq!(
        plan.comparisons
            .iter()
            .filter(|v| v.variant == REPEAT)
            .count(),
        18
    );
    assert!(plan.contexts.len() + plan.pairs.len() > plan.comparisons.len());
    assert!(
        plan.shared_requests()
            .iter()
            .any(|v| v["distinct_expectation_sets"].as_u64().unwrap_or(0) > 1)
    );
    for pair in &plan.pairs {
        let context = &plan.contexts[pair.context];
        let old = &plan.comparisons[context.baseline];
        let new = &plan.comparisons[pair.candidate];
        assert_eq!(old.request.questions, new.request.questions);
        assert_eq!(old.fit_threshold, new.fit_threshold);
        if pair.variant == REPEAT {
            assert_eq!(old.request, new.request);
            assert_ne!(context.baseline, pair.candidate);
        } else {
            assert_ne!(old.request.state, new.request.state);
        }
        new.request.validate()?;
        let body = serde_json::to_string(&new.request)?;
        assert!(!body.contains("required_tags") && !body.contains("forbidden_tags"));
    }
    assert!(
        plan.skipped
            .iter()
            .any(|v| v["reason"] == "all_observations_removed_no_inference")
    );
    assert!(
        plan.skipped
            .iter()
            .any(|v| v["case_id"] == "insufficient-evidence")
    );
    let sha = fingerprint(&plan.document())?;
    let n = plan.comparisons.len();
    let units = plan.reservation();
    plan.check_authorization(&sha, n, units)?;
    for (hash, count, budget) in [
        ("bad", n, units),
        (&sha, n + 1, units),
        (&sha, n - 1, units),
        (&sha, n, units + 1),
        (&sha, n, units - 1),
    ] {
        assert!(plan.check_authorization(hash, count, budget).is_err());
    }
    Ok(())
}

#[test]
fn evidence_report_keeps_partial_pairs_and_missing_usage_unknown() -> Result<()> {
    let mut plan = build_plan()?;
    let first = plan.comparisons.first().ok_or("case")?.case_id.clone();
    let count = plan
        .comparisons
        .iter()
        .take_while(|v| v.case_id == first)
        .count();
    plan.comparisons.truncate(count);
    plan.contexts.retain(|v| v.baseline < count);
    plan.pairs
        .retain(|v| v.context < plan.contexts.len() && v.candidate < count);
    let mut text = format!("{}\n", json!({"event":"plan","plan":plan.document()}));
    let empty = report::analyze(&plan, &text)?;
    assert_eq!(empty["paired_partitions"], 0);
    assert_eq!(empty["complete"], false);
    for (index, item) in plan.comparisons.iter().enumerate() {
        text.push_str(&format!("{}\n",json!({"event":"attempt_started","index":index,"case_id":item.case_id,"variant":item.variant})));
        let pending = if index == 0 {
            Some(report::analyze(&plan, &text)?)
        } else {
            None
        };
        if let Some(pending) = pending {
            assert_eq!(pending["unresolved_attempt"], 0);
            assert_eq!(pending["responses"], 0);
        }
        let answers=item.request.questions.iter().map(|(id,q)|{
            let answer=match q {
                TypedQuestion::Score {..}=>unreachable!("binary evidence fixture"),
                TypedQuestion::Noul {..}=>json!({"type":"noul","noul":if item.tags[id].required {if item.variant==CURRENT {0.8}else{0.6}}else{0.1}}),
                TypedQuestion::Choice {criteria,..}=>{
                    let choice=criteria.keys().next().cloned().unwrap_or_default();
                    json!({"type":"choice","choice":choice,"confidence":1.0,"probabilities":criteria.keys().map(|key|(key.clone(),f64::from(key==&choice))).collect::<BTreeMap<_,_>>()})
                }
            };(id.clone(),answer)
        }).collect::<BTreeMap<_,_>>();
        text.push_str(&format!("{}\n",json!({"event":"response","index":index,"result":{"model":MODEL,"answers":answers,"input_tokens":if index==1 {None}else{Some(50)},"output_tokens":10}})));
    }
    text.push_str(&format!(
        "{}\n",
        json!({"event":"complete","requests":count})
    ));
    let result = report::analyze(&plan, &text)?;
    assert_eq!(result["complete"], true);
    assert_eq!(result["certifies_model"], false);
    assert_eq!(result["unpaired_or_unrun_partitions"], 0);
    assert_eq!(result["usage"]["responses_reporting_input"], count - 1);
    assert_eq!(result["usage"]["reported_input_tokens"], (count - 1) * 50);
    assert!(
        result["arms"]
            .as_object()
            .ok_or("arms")?
            .values()
            .any(|v| v["counts"]["required_losses"].as_u64().unwrap_or(0) > 0)
    );
    assert!(result.get("best_arm").is_none() && result.get("passed").is_none());
    assert!(
        !result["period_choices"]
            .as_array()
            .ok_or("period choices")?
            .is_empty()
    );
    let period = &result["period_choices"][0]["baseline"];
    assert_eq!(period["option"], "no_supported_period");
    assert_eq!(period["winner_score"], 1.0);
    assert_eq!(period["tag"], Value::Null);
    text.push_str("{\"event\":\"complete\",\"requests\":0}\n");
    assert!(report::analyze(&plan, &text).is_err());
    Ok(())
}

#[test]
fn shared_answers_preserve_each_expectation_and_count_paid_usage_once() -> Result<()> {
    let mut plan = Plan {
        comparisons: Vec::new(),
        contexts: Vec::new(),
        pairs: Vec::new(),
        skipped: Vec::new(),
        cases: json!([]),
        suite_id: "shared-answer-regression".to_owned(),
    };
    let mut known = BTreeMap::new();
    let questions = BTreeMap::from([(
        "fit_0".to_owned(),
        TypedQuestion::Noul {
            instructions: json!("Does the description support heroism?"),
            criteria: BTreeMap::new(),
        },
    )]);
    for (context, required) in [true, false].into_iter().enumerate() {
        let case_id = format!("case_{context}");
        let tags = BTreeMap::from([(
            "fit_0".to_owned(),
            FitTarget {
                tag: "heroic".to_owned(),
                group: "mood".to_owned(),
                required,
                forbidden: !required,
            },
        )]);
        for (variant, state) in [
            (CURRENT, json!({"album":"Castle","genre":"procession"})),
            ("album_only", json!({"album":"Castle"})),
        ] {
            let index = plan.register(
                Comparison {
                    case_id: case_id.clone(),
                    partition: 0,
                    variant,
                    fit_threshold: 0.7,
                    tags: tags.clone(),
                    request: TypedDecisionRequest {
                        state,
                        questions: questions.clone(),
                    },
                },
                &mut known,
            )?;
            if variant == CURRENT {
                plan.contexts.push(ScoringContext {
                    case_id: case_id.clone(),
                    partition: 0,
                    baseline: index,
                    fit_threshold: 0.7,
                    tags: tags.clone(),
                    cohort: "metadata_only",
                });
            } else {
                plan.pairs.push(Pair {
                    context,
                    candidate: index,
                    variant,
                });
            }
        }
    }
    assert_eq!(plan.comparisons.len(), 2);
    assert_eq!(plan.shared_requests().len(), 2);
    assert!(
        plan.shared_requests()
            .iter()
            .all(|v| v["distinct_expectation_sets"] == 2)
    );
    let mut text = format!("{}\n", json!({"event":"plan","plan":plan.document()}));
    for (index, item) in plan.comparisons.iter().enumerate() {
        text.push_str(&format!("{}\n",json!({"event":"attempt_started","index":index,"case_id":item.case_id,"variant":item.variant})));
        let pending = report::analyze(&plan, &text)?;
        assert_eq!(pending["paired_partitions"], 0);
        assert_eq!(pending["unpaired_or_unrun_partitions"], 2);
        text.push_str(&format!("{}\n",json!({"event":"response","index":index,"result":{"model":MODEL,
            "answers":{"fit_0":{"type":"noul","noul":if index==0 {0.6}else{0.8}}},"input_tokens":50,"output_tokens":10}})));
    }
    text.push_str(&format!("{}\n", json!({"event":"complete","requests":2})));
    let result = report::analyze(&plan, &text)?;
    assert_eq!(result["usage"]["reported_input_tokens"], 100);
    assert_eq!(result["paired_partitions"], 2);
    let arm = &result["arms"]["album_only"];
    assert_eq!(arm["counts"]["required_gains"], 1);
    assert_eq!(arm["counts"]["new_forbidden_candidates"], 1);
    assert_eq!(arm["distinct_request_pairs"], json!([[0, 1]]));
    assert_eq!(arm["paired_variant_reported_input_tokens"], 100);
    assert_eq!(result["scores"][0]["case_id"], "case_0");
    assert_eq!(result["scores"][1]["case_id"], "case_1");
    for expected_index in [2, 3] {
        let original = &plan.comparisons[0];
        let repeat = Comparison {
            case_id: original.case_id.clone(),
            partition: 0,
            variant: REPEAT,
            fit_threshold: original.fit_threshold,
            tags: original.tags.clone(),
            request: original.request.clone(),
        };
        assert_eq!(plan.register(repeat, &mut known)?, expected_index);
    }
    assert_eq!(known.len(), 2);
    Ok(())
}
