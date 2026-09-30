//! Provider-facing numeric cards and fixed-interval reconstruction.
use super::{
    feature_input::{AFFECT, DEVELOPMENT, RHYTHM, SEGMENTS, ScalarSpec, TEXTURE, nullable},
    *,
};

fn rounded(value: f64) -> f64 {
    (value * 1_000_000.0).round() / 1_000_000.0
}

fn interval(value: f64, spec: ScalarSpec) -> Value {
    let bins = ((spec.maximum - spec.minimum) / spec.band_width).ceil();
    let mut index = ((value - spec.minimum) / spec.band_width).floor();
    if index >= bins {
        index = bins - 1.0;
    }
    let lower = rounded(spec.minimum + index * spec.band_width);
    let upper = rounded((lower + spec.band_width).min(spec.maximum));
    json!({"minimum_inclusive":lower,"maximum_inclusive":upper})
}

fn scalar_metrics(
    family: &serde_json::Map<String, Value>,
    specs: &[ScalarSpec],
    bands: bool,
) -> Result<Value> {
    let mut metrics = serde_json::Map::new();
    for spec in specs {
        if family[spec.id].is_null() && nullable(*spec) {
            metrics.insert(
                spec.id.to_owned(),
                json!({"available":false,"unit":spec.unit,"measurement":spec.measurement}),
            );
            continue;
        }
        let value = family[spec.id]
            .as_f64()
            .ok_or("validated numeric feature missing")?;
        metrics.insert(
            spec.id.to_owned(),
            if bands {
                json!({"interval":interval(value,*spec),"unit":spec.unit,"measurement":spec.measurement})
            } else {
                json!({"value":value,"unit":spec.unit,"measurement":spec.measurement})
            },
        );
    }
    Ok(Value::Object(metrics))
}

pub(super) fn family_card(record: &Value, family_name: &str, bands: bool) -> Result<Value> {
    let family = record["families"][family_name]
        .as_object()
        .ok_or("validated family")?;
    let specs: &[ScalarSpec] = match family_name {
        "rhythm" => &RHYTHM,
        "texture" => &TEXTURE,
        "development" => &DEVELOPMENT,
        "affect" => &AFFECT,
        _ => return Err("unknown feature family".into()),
    };
    let mut metrics = scalar_metrics(family, specs, bands)?;
    if family_name == "development" {
        let object = metrics.as_object_mut().ok_or("metrics")?;
        for spec in SEGMENTS {
            let values = family[spec.id].as_array().ok_or("segment values")?;
            let representation = if bands {
                let intervals = values
                    .iter()
                    .map(|value| -> Result<Value> {
                        Ok(interval(
                            value.as_f64().ok_or("validated segment missing")?,
                            spec,
                        ))
                    })
                    .collect::<Result<Vec<_>>>()?;
                json!({"intervals":intervals,
                    "unit":spec.unit,"measurement":spec.measurement})
            } else {
                json!({"values":values,"unit":spec.unit,"measurement":spec.measurement})
            };
            object.insert(spec.id.to_owned(), representation);
        }
    }
    Ok(json!({
        "kind":"bounded_numeric_feature_card",
        "family":family_name,
        "coverage_seconds":record["coverage_seconds"],
        "representation":if bands {"fixed_numeric_intervals"} else {"raw_numeric"},
        "metrics":metrics,
        "interpretation":match family_name {
            "rhythm"=>"Measured onset timing and onset-periodicity descriptors. Note onsets are not necessarily beats; no tempo, loudness, energy, or mood is asserted.",
            "texture"=>"Measured HPSS energy shares, pitch-class profiles, and spectrum descriptors. Harmonic does not mean harmony, and these values do not establish instruments, dissonance, or mood.",
            "development"=>"Measured level and spectral change across the recording and ten equal-time bins. RMS level is not perceived loudness; bins are not detected musical sections.",
            "affect"=>"Locally normalized affect-model estimates. Arousal means modeled activation and valence means modeled pleasantness; neither is a verified mood, listener judgment, or acoustic fact.",
            _=>unreachable!(),
        }
    }))
}
