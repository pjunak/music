use super::*;

fn fixture() -> Result<(Value, Value)> {
    super::super::tests::fixture()
}

fn mapped_request<'a>(plan: &'a Plan, arm: &str, repeat: bool) -> &'a TypedDecisionRequest {
    let index = plan.document["request_mappings"]
        .as_array()
        .expect("mappings")
        .iter()
        .position(|mapping| {
            mapping["track_id"] == 1 && mapping["arm"] == arm && mapping["repeat_control"] == repeat
        })
        .expect("format mapping");
    &plan.comparisons[index].request
}

fn source_request<'a>(source: &'a super::super::Plan, arm: &str) -> &'a TypedDecisionRequest {
    let index = source.document["request_mappings"]
        .as_array()
        .expect("source mappings")
        .iter()
        .position(|mapping| {
            mapping["track_id"] == 1 && mapping["arm"] == arm && mapping["repeat_control"] == false
        })
        .expect("source mapping");
    &source.comparisons[index].request
}

fn cards(request: &TypedDecisionRequest) -> &Vec<Value> {
    request.state["feature_observations"]
        .as_array()
        .expect("feature cards")
}

fn point_slots(cards: &[Value]) -> Result<usize> {
    let mut count = 0;
    for metric in cards.iter().flat_map(|card| {
        card["metrics"]
            .as_object()
            .into_iter()
            .flat_map(|map| map.values())
    }) {
        count += if let Some(values) = metric["values"].as_array() {
            values.len()
        } else {
            1
        };
    }
    Ok(count)
}

#[test]
fn baseline_and_detailed_requests_are_exact_source_requests() -> Result<()> {
    let (baseline, features) = fixture()?;
    let source = super::super::prepare(&baseline, &features, &[1])?;
    let source_document = source.document.clone();
    let plan = prepare(&baseline, &features, &[1])?;
    assert_eq!(source.document, source_document);
    assert_eq!(
        mapped_request(&plan, BASELINE, false),
        source_request(&source, "baseline")
    );
    assert_eq!(
        mapped_request(&plan, DETAILED_ALL, false),
        source_request(&source, "combined_all")
    );
    assert_eq!(
        plan.document["source_feature_plan_sha256"],
        fingerprint(&source.document)?
    );
    Ok(())
}

#[test]
fn compact_arms_preserve_raw_values_units_nulls_and_point_coverage() -> Result<()> {
    let (baseline, mut features) = fixture()?;
    features["records"][0]["families"]["rhythm"]["inter_onset_cv"] = Value::Null;
    let plan = prepare(&baseline, &features, &[1])?;
    let detailed = cards(mapped_request(&plan, DETAILED_ALL, false));
    let compact = cards(mapped_request(&plan, COMPACT_ALL, false));
    assert_eq!(point_slots(detailed)?, 45);
    assert_eq!(point_slots(compact)?, 45);
    assert_eq!(detailed.len(), compact.len());
    for (detailed_card, compact_card) in detailed.iter().zip(compact) {
        assert_eq!(detailed_card["family"], compact_card["family"]);
        assert_eq!(
            detailed_card["coverage_seconds"],
            compact_card["coverage_seconds"]
        );
        assert!(compact_card.get("kind").is_none());
        assert!(compact_card.get("representation").is_none());
        assert!(compact_card.get("interpretation").is_none());
        let detailed_metrics = detailed_card["metrics"].as_object().ok_or("metrics")?;
        let compact_metrics = compact_card["metrics"].as_object().ok_or("metrics")?;
        assert_eq!(
            detailed_metrics.keys().collect::<Vec<_>>(),
            compact_metrics.keys().collect::<Vec<_>>()
        );
        for (id, detailed_metric) in detailed_metrics {
            let compact_metric = &compact_metrics[id];
            assert_eq!(detailed_metric["unit"], compact_metric["unit"]);
            assert!(compact_metric.get("measurement").is_none());
            for key in ["available", "value", "values"] {
                assert_eq!(detailed_metric.get(key), compact_metric.get(key));
            }
        }
    }
    let acoustics = cards(mapped_request(&plan, COMPACT_ACOUSTICS, false));
    assert_eq!(point_slots(acoustics)?, 43);
    assert!(acoustics.iter().all(|card| card["family"] != "affect"));
    Ok(())
}

#[test]
fn definitions_only_preserves_definitions_but_withholds_every_value() -> Result<()> {
    let (baseline, features) = fixture()?;
    let plan = prepare(&baseline, &features, &[1])?;
    let baseline_request = mapped_request(&plan, BASELINE, false);
    let detailed = mapped_request(&plan, DETAILED_ALL, false);
    let definitions = mapped_request(&plan, DEFINITIONS_ONLY, false);
    assert_eq!(definitions.questions, detailed.questions);
    assert_eq!(
        definitions.state["observations"],
        baseline_request.state["observations"]
    );
    assert_eq!(
        definitions.state["interpretation"],
        baseline_request.state["interpretation"]
    );
    let detailed_cards = cards(detailed);
    let definition_cards = cards(definitions);
    assert_eq!(point_slots(detailed_cards)?, 45);
    for (detailed_card, definition_card) in detailed_cards.iter().zip(definition_cards) {
        for key in [
            "kind",
            "family",
            "coverage_seconds",
            "representation",
            "interpretation",
        ] {
            assert_eq!(detailed_card[key], definition_card[key]);
        }
        for (id, detailed_metric) in detailed_card["metrics"].as_object().ok_or("metrics")? {
            let definition_metric = &definition_card["metrics"][id];
            assert_eq!(detailed_metric["unit"], definition_metric["unit"]);
            assert_eq!(
                detailed_metric["measurement"],
                definition_metric["measurement"]
            );
            assert_eq!(definition_metric["available"], false);
            assert!(
                definition_metric.get("value").is_none()
                    && definition_metric.get("values").is_none()
            );
        }
    }
    let mapping = plan.document["request_mappings"]
        .as_array()
        .ok_or("mappings")?
        .iter()
        .find(|mapping| mapping["track_id"] == 1 && mapping["arm"] == DEFINITIONS_ONLY)
        .ok_or("definitions mapping")?;
    assert_eq!(mapping["numeric_values_disclosed"], 0);
    assert_eq!(mapping["feature_point_slots"], 45);
    assert_eq!(mapping["unavailable_points"], 45);
    assert_eq!(mapping["metric_definitions_included"], true);
    assert_eq!(mapping["units_included"], true);
    assert_eq!(mapping["baseline_observations_changed"], false);
    Ok(())
}

#[test]
fn arms_rotation_repeats_null_and_authorization_are_exact() -> Result<()> {
    let (baseline, features) = fixture()?;
    let plan = prepare(&baseline, &features, &[1])?;
    assert_eq!(plan.comparisons.len(), 7);
    assert_eq!(plan.document["request_count"], 7);
    assert_eq!(
        plan.document["request_mappings"]
            .as_array()
            .ok_or("mappings")?
            .iter()
            .take(ARMS.len())
            .map(|mapping| mapping["arm"].as_str().unwrap_or_default())
            .collect::<Vec<_>>(),
        ARMS
    );
    assert_eq!(
        mapped_request(&plan, BASELINE, false),
        mapped_request(&plan, BASELINE, true)
    );
    let null = plan.document["request_mappings"]
        .as_array()
        .ok_or("mappings")?
        .iter()
        .find(|mapping| mapping["control"] == "null_evidence")
        .ok_or("null")?;
    assert!(null["track_id"].is_null());
    assert_eq!(
        plan.comparisons[null["request_index"].as_u64().ok_or("null index")? as usize]
            .request
            .state,
        json!(NULL_STATE)
    );
    let hash = fingerprint(&plan.document)?;
    let units = plan.document["max_input_units"].as_u64().ok_or("units")?;
    authorize(&plan, &plan.document, &hash, 7, units)?;
    assert!(authorize(&plan, &plan.document, &hash, 8, units).is_err());
    assert!(authorize(&plan, &plan.document, &hash, 7, units + 1).is_err());
    let mut changed = plan.document.clone();
    changed["comparisons"][0]["request"]["state"] = json!({"changed":true});
    assert!(authorize(&plan, &changed, &fingerprint(&changed)?, 7, units).is_err());
    Ok(())
}

#[test]
fn complete_export_is_strict_and_retains_raw_answers_and_null_usage() -> Result<()> {
    let (baseline, features) = fixture()?;
    let plan = prepare(&baseline, &features, &[1])?;
    let mut text = format!("{}\n", json!({"event":"plan","plan":plan.document}));
    assert!(report_text(&plan, &text).is_err());
    for (index, item) in plan.comparisons.iter().enumerate() {
        text.push_str(&format!(
            "{}\n",
            json!({"event":"attempt_started","index":index,"case_id":item.case_id,"variant":item.variant})
        ));
        let answers = item
            .request
            .questions
            .keys()
            .map(|id| (id.clone(), json!({"type":"noul","noul":0.5})))
            .collect::<std::collections::BTreeMap<_, _>>();
        text.push_str(&format!(
            "{}\n",
            json!({"event":"response","index":index,"result":{"model":MODEL,"answers":answers,
                "input_tokens":if index == 0 {None} else {Some(50)},"output_tokens":10}})
        ));
    }
    text.push_str(&format!(
        "{}\n",
        json!({"event":"complete","requests":plan.comparisons.len()})
    ));
    let report = report_text(&plan, &text)?;
    assert_eq!(report["complete"], true);
    assert_eq!(report["responses"], 7);
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
