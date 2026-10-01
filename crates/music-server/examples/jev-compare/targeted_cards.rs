//! Provider-state cards for validated targeted features. No semantic labels are derived here.
use super::*;

#[derive(Clone, Copy)]
struct Metric {
    id: &'static str,
    unit: &'static str,
    definition: &'static str,
}

const BEAT: [Metric; 6] = [
    Metric {
        id: "bpm_candidate",
        unit: "beats_per_minute",
        definition: "Candidate pulse rate from the dynamic-programming seed at the strongest valid lag.",
    },
    Metric {
        id: "pulse_support",
        unit: "unit_interval",
        definition: "Positive-onset autocorrelation at the candidate period, normalized by zero-lag energy.",
    },
    Metric {
        id: "tempo_peak_margin",
        unit: "unit_interval",
        definition: "Relative gap between candidate autocorrelation and strongest competing peak outside its two-lag neighborhood; half/double-rate peaks remain eligible.",
    },
    Metric {
        id: "beat_interval_cv",
        unit: "coefficient_of_variation",
        definition: "Variation of detected beat intervals divided by their mean.",
    },
    Metric {
        id: "accent_3_fit",
        unit: "unit_interval",
        definition: "Fit of beat-centered RMS accents to a repeating three-beat pattern.",
    },
    Metric {
        id: "accent_4_fit",
        unit: "unit_interval",
        definition: "Fit of beat-centered RMS accents to a repeating four-beat pattern.",
    },
];

const HARMONY: [Metric; 5] = [
    Metric {
        id: "roughness_mean",
        unit: "unit_interval",
        definition: "Mean normalized spectral-peak roughness proxy over valid frames.",
    },
    Metric {
        id: "roughness_p90",
        unit: "unit_interval",
        definition: "Ninetieth percentile of the normalized spectral-peak roughness proxy.",
    },
    Metric {
        id: "key_template_fit",
        unit: "normalized_template_fit",
        definition: "Best correlation with the fixed major and minor key templates.",
    },
    Metric {
        id: "major_minor_margin",
        unit: "normalized_template_fit_difference",
        definition: "Best major-template fit minus best minor-template fit.",
    },
    Metric {
        id: "tonal_change",
        unit: "l1_distance",
        definition: "Mean L1 distance between adjacent valid one-second normalized harmonic pitch-class profiles; missing seconds are not bridged.",
    },
];

fn family_card(record: &Value, family: &str, described: bool, missing: bool) -> Result<Value> {
    let metrics = match family {
        "beat" => &BEAT[..],
        "harmony" => &HARMONY[..],
        _ => return Err("unknown targeted numeric family".into()),
    };
    let measurements = metrics
        .iter()
        .map(|metric| {
            let value = if missing {
                Value::Null
            } else {
                record[family][metric.id].clone()
            };
            let mut card = json!({
                "id":metric.id,
                "unit":metric.unit,
                "availability":if value.is_null() {"unavailable"} else {"available"},
                "value":value,
            });
            if described {
                card["definition"] = json!(metric.definition);
            }
            card
        })
        .collect::<Vec<_>>();
    Ok(json!({
        "family":family,
        "coverage_seconds":if missing {Value::Null} else {record["coverage_seconds"].clone()},
        "measurements":measurements,
    }))
}

pub(super) fn numeric_cards(
    record: &Value,
    families: &[&str],
    described: bool,
    missing: bool,
) -> Result<Value> {
    Ok(Value::Array(
        families
            .iter()
            .map(|family| family_card(record, family, described, missing))
            .collect::<Result<Vec<_>>>()?,
    ))
}

fn response_list(profile: &Value, classifier: &str, missing: bool) -> Result<Vec<Value>> {
    Ok(profile[classifier]
        .as_array()
        .ok_or("profile responses")?
        .iter()
        .map(|item| {
            json!({
                "label":item["label"],
                "response":if missing {Value::Null} else {item["response"].clone()},
            })
        })
        .collect())
}

pub(super) fn timeline_card(record: &Value, missing: bool) -> Result<Value> {
    let profiles = record["profiles"]
        .as_array()
        .ok_or("profiles")?
        .iter()
        .map(|profile| {
            Ok(json!({
                "start_fraction":profile["start_fraction"],
                "end_fraction":profile["end_fraction"],
                "instrument":response_list(profile, "instrument", missing)?,
                "style":response_list(profile, "style", missing)?,
            }))
        })
        .collect::<Result<Vec<_>>>()?;
    Ok(json!({
        "coverage_seconds":if missing {Value::Null} else {record["coverage_seconds"].clone()},
        "representation":"three_equal_time_profiles",
        "response_scale":"uncalibrated_shared_encoder_response",
        "independent_corroboration":false,
        "availability":if missing {"unavailable"} else {"available"},
        "profiles":profiles,
    }))
}

fn pooled_classifier(record: &Value, classifier: &str) -> Result<Vec<Value>> {
    let profiles = record["profiles"].as_array().ok_or("profiles")?;
    let first = profiles.first().ok_or("profiles")?[classifier]
        .as_array()
        .ok_or("profile responses")?;
    let mut pooled = Vec::with_capacity(first.len());
    for index in 0..first.len() {
        let label = first[index]["label"].clone();
        let mut weighted = 0.0;
        let mut total = 0.0;
        for profile in profiles {
            let width = profile["end_fraction"].as_f64().ok_or("end fraction")?
                - profile["start_fraction"].as_f64().ok_or("start fraction")?;
            weighted += width
                * profile[classifier][index]["response"]
                    .as_f64()
                    .ok_or("profile response")?;
            total += width;
        }
        pooled.push(json!({"label":label,"response":weighted/total}));
    }
    Ok(pooled)
}

pub(super) fn means_card(record: &Value) -> Result<Value> {
    Ok(json!({
        "coverage_seconds":record["coverage_seconds"],
        "representation":"time_weighted_profile_means",
        "response_scale":"uncalibrated_shared_encoder_response",
        "independent_corroboration":false,
        "availability":"available",
        "instrument":pooled_classifier(record, "instrument")?,
        "style":pooled_classifier(record, "style")?,
    }))
}
