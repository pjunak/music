//! Strict local input contract for the bounded feature experiment.
use super::*;
use std::collections::{BTreeMap, BTreeSet};

pub(super) const INPUT_SCHEMA: &str = "jev-acoustic-features/v1";
const MAX_FEATURE_BYTES: u64 = 16 * 1024 * 1024;
const MAX_PROVENANCE_DETAILS_BYTES: usize = 64 * 1024;

#[derive(Clone, Copy)]
pub(super) struct ScalarSpec {
    pub id: &'static str,
    pub minimum: f64,
    pub maximum: f64,
    pub band_width: f64,
    pub unit: &'static str,
    pub measurement: &'static str,
}

pub(super) const RHYTHM: [ScalarSpec; 3] = [
    ScalarSpec {
        id: "onsets_per_second",
        minimum: 0.0,
        maximum: 20.0,
        band_width: 0.25,
        unit: "note_onsets_per_second",
        measurement: "Detected note-onset rate; note onsets are not necessarily beat counts.",
    },
    ScalarSpec {
        id: "pulse_strength",
        minimum: 0.0,
        maximum: 1.0,
        band_width: 0.1,
        unit: "unit_interval",
        measurement: "Normalized onset-autocorrelation periodicity; not loudness, energy, or mood.",
    },
    ScalarSpec {
        id: "inter_onset_cv",
        minimum: 0.0,
        maximum: 4.0,
        band_width: 0.25,
        unit: "coefficient_of_variation",
        measurement: "Coefficient of variation of detected inter-onset intervals.",
    },
];

pub(super) const TEXTURE: [ScalarSpec; 6] = [
    ScalarSpec {
        id: "harmonic_fraction",
        minimum: 0.0,
        maximum: 1.0,
        band_width: 0.1,
        unit: "energy_share",
        measurement: "HPSS harmonic-mask assignment of spectrogram power; not separated instrument energy, harmony, or mood.",
    },
    ScalarSpec {
        id: "percussive_fraction",
        minimum: 0.0,
        maximum: 1.0,
        band_width: 0.1,
        unit: "energy_share",
        measurement: "HPSS percussive-mask assignment of spectrogram power; not separated instrument energy, loudness, or mood.",
    },
    ScalarSpec {
        id: "chroma_concentration",
        minimum: 0.0,
        maximum: 1.0,
        band_width: 0.1,
        unit: "unit_interval",
        measurement: "Concentration of energy among pitch classes; not harmonic correctness or mood.",
    },
    ScalarSpec {
        id: "chroma_change",
        minimum: 0.0,
        maximum: 1.0,
        band_width: 0.1,
        unit: "unit_interval",
        measurement: "Mean cosine distance between successive available one-second pitch-class profiles; not dissonance or emotional change.",
    },
    ScalarSpec {
        id: "spectral_flatness",
        minimum: 0.0,
        maximum: 1.0,
        band_width: 0.1,
        unit: "unit_interval",
        measurement: "Mean spectral-power geometric-to-arithmetic ratio excluding DC and near-silent frames, with a relative floor.",
    },
    ScalarSpec {
        id: "spectral_centroid_hz",
        minimum: 0.0,
        maximum: 24_000.0,
        band_width: 500.0,
        unit: "hertz",
        measurement: "Time-averaged magnitude-weighted spectral centroid excluding near-silent frames, used as a brightness proxy.",
    },
];

pub(super) const DEVELOPMENT: [ScalarSpec; 4] = [
    ScalarSpec {
        id: "rms_iqr_db",
        minimum: 0.0,
        maximum: 120.0,
        band_width: 3.0,
        unit: "decibels",
        measurement: "Interquartile range of RMS level in decibels; not perceived loudness.",
    },
    ScalarSpec {
        id: "peak_to_median_rms_db",
        minimum: 0.0,
        maximum: 120.0,
        band_width: 3.0,
        unit: "decibels",
        measurement: "Peak-to-median RMS difference in decibels; not perceived loudness.",
    },
    ScalarSpec {
        id: "early_late_rms_db_delta",
        minimum: -120.0,
        maximum: 120.0,
        band_width: 3.0,
        unit: "decibels",
        measurement: "Late-minus-early RMS-level change; not an energy or mood label.",
    },
    ScalarSpec {
        id: "early_late_spectral_centroid_hz_delta",
        minimum: -24_000.0,
        maximum: 24_000.0,
        band_width: 500.0,
        unit: "hertz",
        measurement: "Late-minus-early spectral-centroid change.",
    },
];

pub(super) const AFFECT: [ScalarSpec; 2] = [
    ScalarSpec {
        id: "arousal",
        minimum: 0.0,
        maximum: 1.0,
        band_width: 0.1,
        unit: "normalized_model_output",
        measurement: "Unweighted overlapping-window mean of normalized modeled activation; not loudness, energy, or verified mood.",
    },
    ScalarSpec {
        id: "valence",
        minimum: 0.0,
        maximum: 1.0,
        band_width: 0.1,
        unit: "normalized_model_output",
        measurement: "Unweighted overlapping-window mean of normalized modeled pleasantness; not a verified mood or owner rating.",
    },
];

pub(super) const SEGMENTS: [ScalarSpec; 3] = [
    ScalarSpec {
        id: "segment_rms_db_relative_to_median",
        minimum: -120.0,
        maximum: 120.0,
        band_width: 3.0,
        unit: "decibels",
        measurement: "Median frame RMS level minus the whole-recording median in ten equal-time bins.",
    },
    ScalarSpec {
        id: "segment_onsets_per_second",
        minimum: 0.0,
        maximum: 20.0,
        band_width: 0.25,
        unit: "note_onsets_per_second",
        measurement: "Detected note-onset rate in ten equal-time bins; not necessarily beat counts.",
    },
    ScalarSpec {
        id: "segment_spectral_centroid_hz",
        minimum: 0.0,
        maximum: 24_000.0,
        band_width: 500.0,
        unit: "hertz",
        measurement: "Time-averaged magnitude-weighted spectral centroid in ten equal-time bins.",
    },
];

fn exact_object<'a>(
    value: &'a Value,
    required: &[&str],
    optional: &[&str],
    name: &str,
) -> Result<&'a serde_json::Map<String, Value>> {
    let object = value
        .as_object()
        .ok_or_else(|| format!("{name} must be an object"))?;
    if required.iter().any(|key| !object.contains_key(*key))
        || object
            .keys()
            .any(|key| !required.contains(&key.as_str()) && !optional.contains(&key.as_str()))
    {
        return Err(format!("{name} has missing or unknown fields").into());
    }
    Ok(object)
}

fn bounded(value: &Value, spec: ScalarSpec) -> Result<f64> {
    let number = value
        .as_f64()
        .ok_or_else(|| format!("{} must be numeric", spec.id))?;
    if !(spec.minimum..=spec.maximum).contains(&number) {
        return Err(format!("{} is outside its fixed range", spec.id).into());
    }
    Ok(number)
}

pub(super) fn nullable(spec: ScalarSpec) -> bool {
    matches!(
        spec.id,
        "inter_onset_cv" | "chroma_concentration" | "chroma_change"
    )
}

fn text(value: &Value, name: &str) -> Result<()> {
    let value = value
        .as_str()
        .ok_or_else(|| format!("{name} must be text"))?;
    if value.is_empty() || value.len() > 128 || value.chars().any(char::is_control) {
        return Err(format!("{name} is invalid").into());
    }
    Ok(())
}

fn sha256(value: &Value, name: &str) -> Result<()> {
    let value = value
        .as_str()
        .ok_or_else(|| format!("{name} must be text"))?;
    if value.len() != 64
        || value
            .bytes()
            .any(|byte| !byte.is_ascii_digit() && !(b'a'..=b'f').contains(&byte))
    {
        return Err(format!("{name} must be a lowercase SHA-256 digest").into());
    }
    Ok(())
}

fn validate_provenance(value: &Value) -> Result<()> {
    let provenance = exact_object(
        value,
        &[
            "extractor",
            "extractor_version",
            "affect_model",
            "affect_model_sha256",
            "affect_source_output_order",
            "affect_source_scale",
            "affect_normalization",
        ],
        &["details"],
        "provenance",
    )?;
    for key in ["extractor", "extractor_version", "affect_model"] {
        text(&provenance[key], key)?;
    }
    sha256(&provenance["affect_model_sha256"], "affect_model_sha256")?;
    let order = provenance["affect_source_output_order"]
        .as_array()
        .ok_or("affect output order must be an array")?;
    let order_names = order
        .iter()
        .map(|item| -> Result<&str> { Ok(item.as_str().ok_or("affect output-order item")?) })
        .collect::<Result<BTreeSet<_>>>()?;
    if order.len() != 2
        || order_names.len() != 2
        || !order_names
            .iter()
            .all(|item| ["arousal", "valence"].contains(item))
    {
        return Err("affect output order must contain arousal and valence exactly once".into());
    }
    let scale = exact_object(
        &provenance["affect_source_scale"],
        &["minimum", "maximum"],
        &[],
        "affect source scale",
    )?;
    let minimum = scale["minimum"].as_f64().ok_or("affect scale minimum")?;
    let maximum = scale["maximum"].as_f64().ok_or("affect scale maximum")?;
    if minimum >= maximum {
        return Err("affect source scale must increase".into());
    }
    if provenance["affect_normalization"] != "linear_unit_interval" {
        return Err(
            "affect values must use the declared linear unit-interval normalization".into(),
        );
    }
    if let Some(details) = provenance.get("details")
        && (!details.is_object()
            || serde_json::to_vec(details)?.len() > MAX_PROVENANCE_DETAILS_BYTES)
    {
        return Err("provenance details must be a bounded object".into());
    }
    Ok(())
}

fn validate_family(value: &Value, specs: &[ScalarSpec], name: &str) -> Result<()> {
    let keys = specs.iter().map(|spec| spec.id).collect::<Vec<_>>();
    let family = exact_object(value, &keys, &[], name)?;
    for spec in specs {
        if family[spec.id].is_null() && nullable(*spec) {
            continue;
        }
        bounded(&family[spec.id], *spec)?;
    }
    Ok(())
}

fn validate_development(value: &Value) -> Result<()> {
    let mut keys = DEVELOPMENT.iter().map(|spec| spec.id).collect::<Vec<_>>();
    keys.extend(SEGMENTS.iter().map(|spec| spec.id));
    let family = exact_object(value, &keys, &[], "development")?;
    for spec in DEVELOPMENT {
        bounded(&family[spec.id], spec)?;
    }
    for spec in SEGMENTS {
        let values = family[spec.id]
            .as_array()
            .ok_or_else(|| format!("{} must be an array", spec.id))?;
        if values.len() != 10 {
            return Err(format!("{} must contain ten equal-time bins", spec.id).into());
        }
        for value in values {
            bounded(value, spec)?;
        }
    }
    Ok(())
}

fn validate_record(value: &Value) -> Result<()> {
    let record = exact_object(
        value,
        &["track_id", "file_sha256", "coverage_seconds", "families"],
        &[],
        "feature record",
    )?;
    record["track_id"].as_u64().ok_or("track_id")?;
    sha256(&record["file_sha256"], "file_sha256")?;
    let coverage = record["coverage_seconds"]
        .as_f64()
        .ok_or("coverage_seconds")?;
    if !(0.0..=36_000.0).contains(&coverage) || coverage == 0.0 {
        return Err("coverage_seconds is outside its fixed range".into());
    }
    let families = exact_object(
        &record["families"],
        &["rhythm", "texture", "development", "affect"],
        &[],
        "families",
    )?;
    validate_family(&families["rhythm"], &RHYTHM, "rhythm")?;
    validate_family(&families["texture"], &TEXTURE, "texture")?;
    validate_development(&families["development"])?;
    validate_family(&families["affect"], &AFFECT, "affect")?;
    let harmonic = families["texture"]["harmonic_fraction"]
        .as_f64()
        .ok_or("harmonic fraction")?;
    let percussive = families["texture"]["percussive_fraction"]
        .as_f64()
        .ok_or("percussive fraction")?;
    if (harmonic + percussive - 1.0).abs() > 0.02 {
        return Err(
            "HPSS harmonic and percussive energy shares must sum approximately to one".into(),
        );
    }
    Ok(())
}

pub(super) fn validate(baseline: &Value, features: &Value) -> Result<BTreeMap<u64, Value>> {
    if serde_json::to_vec(features)?.len() as u64 > MAX_FEATURE_BYTES {
        return Err("feature input exceeds its fixed file bound".into());
    }
    let input = exact_object(
        features,
        &[
            "schema_version",
            "baseline_plan_sha256",
            "provenance",
            "records",
        ],
        &[],
        "feature input",
    )?;
    if input["schema_version"] != INPUT_SCHEMA
        || input["baseline_plan_sha256"] != fingerprint(baseline)?
    {
        return Err("feature input schema or frozen baseline hash differs".into());
    }
    validate_provenance(&input["provenance"])?;
    let baseline_identities = baseline["recordings"]
        .as_array()
        .ok_or("baseline recordings")?
        .iter()
        .map(|recording| {
            Ok((
                recording["input"]["track_id"]
                    .as_u64()
                    .ok_or("baseline track id")?,
                recording["file_sha256"]
                    .as_str()
                    .ok_or("baseline file hash")?
                    .to_owned(),
                recording["input"]["length_s"]
                    .as_f64()
                    .ok_or("baseline duration")?,
            ))
        })
        .map(|identity| identity.map(|(id, hash, seconds)| (id, (hash, seconds))))
        .collect::<Result<BTreeMap<_, _>>>()?;
    let mut records = BTreeMap::new();
    for record in input["records"].as_array().ok_or("records")? {
        validate_record(record)?;
        let track_id = record["track_id"].as_u64().ok_or("track id")?;
        let (expected_hash, expected_seconds) = baseline_identities
            .get(&track_id)
            .ok_or("feature track is absent from baseline")?;
        let coverage = record["coverage_seconds"].as_f64().ok_or("coverage")?;
        if Some(expected_hash.as_str()) != record["file_sha256"].as_str()
            || (coverage - expected_seconds).abs() > 0.1
            || records.insert(track_id, record.clone()).is_some()
        {
            return Err(
                "feature record is duplicated or differs from baseline audio identity".into(),
            );
        }
    }
    if records.is_empty() {
        return Err("feature input has no records".into());
    }
    Ok(records)
}
