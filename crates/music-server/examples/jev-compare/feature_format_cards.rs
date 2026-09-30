//! Strict presentation transforms for the feature-format follow-up.
use super::*;
use std::collections::BTreeSet;

#[derive(Clone, Copy)]
pub(super) struct CardStats {
    pub point_slots: usize,
    pub unavailable_points: usize,
}

#[derive(Clone, Copy)]
pub(super) struct Presentation {
    pub metric_definitions: bool,
    pub units: bool,
    pub card_interpretations: bool,
    pub card_type_labels: bool,
}

fn metric_stats(metric: &Value) -> Result<CardStats> {
    let object = metric.as_object().ok_or("feature metric")?;
    if !object.contains_key("unit") || !object.contains_key("measurement") {
        return Err("detailed feature metric lacks its unit or definition".into());
    }
    let payloads = ["value", "values", "available"]
        .iter()
        .filter(|key| object.contains_key(**key))
        .count();
    if payloads != 1 || object.len() != 3 {
        return Err("detailed feature metric has an unexpected shape".into());
    }
    if object.get("available") == Some(&json!(false)) {
        return Ok(CardStats {
            point_slots: 1,
            unavailable_points: 1,
        });
    }
    if object.get("value").is_some_and(Value::is_number) {
        return Ok(CardStats {
            point_slots: 1,
            unavailable_points: 0,
        });
    }
    let values = object
        .get("values")
        .and_then(Value::as_array)
        .ok_or("detailed feature metric value")?;
    if values.is_empty() || values.iter().any(|value| !value.is_number()) {
        return Err("detailed feature metric array is invalid".into());
    }
    Ok(CardStats {
        point_slots: values.len(),
        unavailable_points: 0,
    })
}

fn card_stats(cards: &[Value]) -> Result<CardStats> {
    let mut families = BTreeSet::new();
    let mut stats = CardStats {
        point_slots: 0,
        unavailable_points: 0,
    };
    for card in cards {
        let object = card.as_object().ok_or("feature card")?;
        for key in [
            "kind",
            "family",
            "coverage_seconds",
            "representation",
            "metrics",
            "interpretation",
        ] {
            if !object.contains_key(key) {
                return Err("detailed feature card is incomplete".into());
            }
        }
        if object.len() != 6 || object["kind"] != "bounded_numeric_feature_card" {
            return Err("detailed feature card has an unexpected shape".into());
        }
        let family = object["family"].as_str().ok_or("feature family")?;
        if !families.insert(family) {
            return Err("duplicate feature family".into());
        }
        for metric in object["metrics"]
            .as_object()
            .ok_or("feature metrics")?
            .values()
        {
            let metric = metric_stats(metric)?;
            stats.point_slots += metric.point_slots;
            stats.unavailable_points += metric.unavailable_points;
        }
    }
    Ok(stats)
}

fn detailed_cards(request: &TypedDecisionRequest) -> Result<&Vec<Value>> {
    request.state["feature_observations"]
        .as_array()
        .ok_or_else(|| "detailed feature request lacks cards".into())
}

fn compact_metric(metric: &Value) -> Result<Value> {
    metric_stats(metric)?;
    let object = metric.as_object().ok_or("feature metric")?;
    let mut compact = serde_json::Map::new();
    compact.insert("unit".into(), object["unit"].clone());
    for key in ["available", "value", "values"] {
        if let Some(value) = object.get(key) {
            compact.insert(key.into(), value.clone());
        }
    }
    Ok(Value::Object(compact))
}

fn compact_cards(detailed: &[Value], acoustics_only: bool) -> Result<Vec<Value>> {
    let mut cards = Vec::new();
    for card in detailed {
        if acoustics_only && card["family"] == "affect" {
            continue;
        }
        let metrics = card["metrics"]
            .as_object()
            .ok_or("detailed metrics")?
            .iter()
            .map(|(id, metric)| Ok((id.clone(), compact_metric(metric)?)))
            .collect::<Result<serde_json::Map<_, _>>>()?;
        cards.push(json!({
            "family":card["family"],
            "coverage_seconds":card["coverage_seconds"],
            "metrics":metrics,
        }));
    }
    Ok(cards)
}

fn definitions_only_cards(detailed: &[Value]) -> Result<Vec<Value>> {
    let mut cards = detailed.to_vec();
    for card in &mut cards {
        for metric in card["metrics"]
            .as_object_mut()
            .ok_or("detailed metrics")?
            .values_mut()
        {
            metric_stats(metric)?;
            let object = metric.as_object_mut().ok_or("feature metric")?;
            object.remove("value");
            object.remove("values");
            object.insert("available".into(), json!(false));
        }
    }
    Ok(cards)
}

pub(super) fn request_for(
    arm: &'static str,
    baseline: &TypedDecisionRequest,
    detailed: &TypedDecisionRequest,
) -> Result<(TypedDecisionRequest, CardStats, Presentation)> {
    if baseline.questions != detailed.questions {
        return Err("source baseline and detailed questions differ".into());
    }
    let detailed_cards = detailed_cards(detailed)?;
    let detailed_stats = card_stats(detailed_cards)?;
    if detailed_stats.point_slots != 45 {
        return Err("detailed all-family request must contain 45 feature slots".into());
    }
    let (request, expected_slots, presentation) = match arm {
        BASELINE => (
            baseline.clone(),
            0,
            Presentation {
                metric_definitions: false,
                units: false,
                card_interpretations: false,
                card_type_labels: false,
            },
        ),
        DETAILED_ALL => (
            detailed.clone(),
            45,
            Presentation {
                metric_definitions: true,
                units: true,
                card_interpretations: true,
                card_type_labels: true,
            },
        ),
        COMPACT_ALL | COMPACT_ACOUSTICS | DEFINITIONS_ONLY => {
            let mut request = detailed.clone();
            let cards = match arm {
                COMPACT_ALL => compact_cards(detailed_cards, false)?,
                COMPACT_ACOUSTICS => compact_cards(detailed_cards, true)?,
                DEFINITIONS_ONLY => definitions_only_cards(detailed_cards)?,
                _ => unreachable!(),
            };
            request.state["feature_observations"] = json!(cards);
            let presentation = match arm {
                COMPACT_ALL | COMPACT_ACOUSTICS => Presentation {
                    metric_definitions: false,
                    units: true,
                    card_interpretations: false,
                    card_type_labels: false,
                },
                DEFINITIONS_ONLY => Presentation {
                    metric_definitions: true,
                    units: true,
                    card_interpretations: true,
                    card_type_labels: true,
                },
                _ => unreachable!(),
            };
            (
                request,
                if arm == COMPACT_ACOUSTICS { 43 } else { 45 },
                presentation,
            )
        }
        _ => return Err("unknown feature-format arm".into()),
    };
    request.validate()?;
    if request.questions.len() != QUESTION_COUNT {
        return Err("every feature-format request must contain all 138 questions".into());
    }
    let stats = if expected_slots == 0 {
        CardStats {
            point_slots: 0,
            unavailable_points: 0,
        }
    } else {
        let stats = if arm == DEFINITIONS_ONLY {
            CardStats {
                point_slots: 45,
                unavailable_points: 45,
            }
        } else if arm == DETAILED_ALL {
            detailed_stats
        } else {
            let cards = request.state["feature_observations"]
                .as_array()
                .ok_or("formatted cards")?;
            let mut point_slots = 0;
            let mut unavailable_points = 0;
            for card in cards {
                for metric in card["metrics"]
                    .as_object()
                    .ok_or("formatted metrics")?
                    .values()
                {
                    if metric["available"] == false {
                        point_slots += 1;
                        unavailable_points += 1;
                    } else if metric.get("value").is_some() {
                        point_slots += 1;
                    } else {
                        point_slots += metric["values"]
                            .as_array()
                            .ok_or("formatted metric values")?
                            .len();
                    }
                }
            }
            CardStats {
                point_slots,
                unavailable_points,
            }
        };
        if stats.point_slots != expected_slots {
            return Err("feature-format arm changed numeric slot coverage".into());
        }
        stats
    };
    Ok((request, stats, presentation))
}
