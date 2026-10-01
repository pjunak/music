//! Strict local input validation for the targeted Jev feature comparison.
use super::*;
use std::collections::{BTreeMap, BTreeSet};

pub(super) const INPUT_SCHEMA: &str = "jev-targeted-features/v1";
const MAX_INPUT_BYTES: u64 = 128 * 1024 * 1024;
const MAX_DETAILS_BYTES: usize = 64 * 1024;
const FRACTION_TOLERANCE: f64 = 1e-8;

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

fn number(value: &Value, minimum: f64, maximum: f64, name: &str) -> Result<f64> {
    let value = value
        .as_f64()
        .ok_or_else(|| format!("{name} must be numeric"))?;
    if !value.is_finite() || !(minimum..=maximum).contains(&value) {
        return Err(format!("{name} is outside its fixed range").into());
    }
    Ok(value)
}

fn nullable(value: &Value, minimum: f64, maximum: f64, name: &str) -> Result<()> {
    if !value.is_null() {
        number(value, minimum, maximum, name)?;
    }
    Ok(())
}

fn digest(value: &Value, name: &str) -> Result<()> {
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

fn label(value: &Value) -> Result<()> {
    let value = value.as_str().ok_or("profile label must be text")?;
    if value.is_empty() || value.len() > 128 || value.chars().any(char::is_control) {
        return Err("profile label is invalid".into());
    }
    Ok(())
}

fn validate_provenance(value: &Value) -> Result<()> {
    let provenance = exact_object(
        value,
        &[
            "physical_extractor_sha256",
            "physical_dataset_sha256",
            "temporal_extractor_sha256",
            "temporal_dataset_sha256",
        ],
        &["details"],
        "provenance",
    )?;
    for field in [
        "physical_extractor_sha256",
        "physical_dataset_sha256",
        "temporal_extractor_sha256",
        "temporal_dataset_sha256",
    ] {
        digest(&provenance[field], field)?;
    }
    if let Some(details) = provenance.get("details")
        && (!details.is_object() || serde_json::to_vec(details)?.len() > MAX_DETAILS_BYTES)
    {
        return Err("provenance details must be a bounded local object".into());
    }
    Ok(())
}

fn validate_responses(value: &Value, name: &str) -> Result<()> {
    let values = value
        .as_array()
        .ok_or_else(|| format!("{name} must be an array"))?;
    if values.len() != 8 {
        return Err(format!("{name} must contain exactly eight labels").into());
    }
    for item in values {
        let item = exact_object(item, &["label", "response"], &[], "profile response")?;
        label(&item["label"])?;
        number(&item["response"], 0.0, 1.0, "profile response")?;
    }
    Ok(())
}

fn validate_profiles(value: &Value) -> Result<()> {
    let profiles = value.as_array().ok_or("profiles must be an array")?;
    if profiles.len() != 3 {
        return Err("profiles must contain exactly three intervals".into());
    }
    let expected = [(0.0, 1.0 / 3.0), (1.0 / 3.0, 2.0 / 3.0), (2.0 / 3.0, 1.0)];
    for (profile, (expected_start, expected_end)) in profiles.iter().zip(expected) {
        let profile = exact_object(
            profile,
            &["start_fraction", "end_fraction", "instrument", "style"],
            &[],
            "profile",
        )?;
        let start = number(&profile["start_fraction"], 0.0, 1.0, "start_fraction")?;
        let end = number(&profile["end_fraction"], 0.0, 1.0, "end_fraction")?;
        if (start - expected_start).abs() > FRACTION_TOLERANCE
            || (end - expected_end).abs() > FRACTION_TOLERANCE
            || end <= start
        {
            return Err("profiles must exactly cover the three contiguous thirds".into());
        }
        validate_responses(&profile["instrument"], "instrument profile")?;
        validate_responses(&profile["style"], "style profile")?;
    }
    Ok(())
}

fn validate_record(value: &Value) -> Result<()> {
    let record = exact_object(
        value,
        &[
            "track_id",
            "file_sha256",
            "coverage_seconds",
            "beat",
            "harmony",
            "profiles",
        ],
        &[],
        "targeted record",
    )?;
    record["track_id"].as_u64().ok_or("track_id")?;
    digest(&record["file_sha256"], "file_sha256")?;
    number(
        &record["coverage_seconds"],
        f64::MIN_POSITIVE,
        36_000.0,
        "coverage_seconds",
    )?;
    let beat = exact_object(
        &record["beat"],
        &[
            "bpm_candidate",
            "pulse_support",
            "tempo_peak_margin",
            "beat_interval_cv",
            "accent_3_fit",
            "accent_4_fit",
        ],
        &[],
        "beat",
    )?;
    nullable(&beat["bpm_candidate"], 30.0, 300.0, "bpm_candidate")?;
    number(&beat["pulse_support"], 0.0, 1.0, "pulse_support")?;
    number(&beat["tempo_peak_margin"], 0.0, 1.0, "tempo_peak_margin")?;
    nullable(&beat["beat_interval_cv"], 0.0, 10.0, "beat_interval_cv")?;
    nullable(&beat["accent_3_fit"], 0.0, 1.0, "accent_3_fit")?;
    nullable(&beat["accent_4_fit"], 0.0, 1.0, "accent_4_fit")?;
    let harmony = exact_object(
        &record["harmony"],
        &[
            "roughness_mean",
            "roughness_p90",
            "key_template_fit",
            "major_minor_margin",
            "tonal_change",
        ],
        &[],
        "harmony",
    )?;
    nullable(&harmony["roughness_mean"], 0.0, 1.0, "roughness_mean")?;
    nullable(&harmony["roughness_p90"], 0.0, 1.0, "roughness_p90")?;
    nullable(&harmony["key_template_fit"], -1.0, 1.0, "key_template_fit")?;
    nullable(
        &harmony["major_minor_margin"],
        -2.0,
        2.0,
        "major_minor_margin",
    )?;
    nullable(&harmony["tonal_change"], 0.0, 2.0, "tonal_change")?;
    validate_profiles(&record["profiles"])?;
    Ok(())
}

pub(crate) fn read(path: &Path) -> Result<Value> {
    if path.metadata()?.len() > MAX_INPUT_BYTES {
        return Err("targeted feature input exceeds 128 MiB".into());
    }
    Ok(serde_json::from_reader(std::fs::File::open(path)?)?)
}

pub(super) fn validate(baseline: &Value, input: &Value) -> Result<BTreeMap<u64, Value>> {
    if serde_json::to_vec(input)?.len() as u64 > MAX_INPUT_BYTES {
        return Err("targeted feature input exceeds 128 MiB".into());
    }
    let input = exact_object(
        input,
        &[
            "schema_version",
            "baseline_plan_sha256",
            "provenance",
            "records",
        ],
        &[],
        "targeted input",
    )?;
    if input["schema_version"] != INPUT_SCHEMA
        || input["baseline_plan_sha256"] != fingerprint(baseline)?
    {
        return Err("targeted input schema or frozen baseline hash differs".into());
    }
    validate_provenance(&input["provenance"])?;
    let baseline_ids = baseline["recordings"]
        .as_array()
        .ok_or("baseline recordings")?
        .iter()
        .map(|recording| {
            Ok((
                recording["input"]["track_id"]
                    .as_u64()
                    .ok_or("baseline track id")?,
                (
                    recording["file_sha256"]
                        .as_str()
                        .ok_or("baseline hash")?
                        .to_owned(),
                    recording["input"]["length_s"]
                        .as_f64()
                        .ok_or("baseline duration")?,
                ),
            ))
        })
        .collect::<Result<BTreeMap<_, _>>>()?;
    let mut records = BTreeMap::new();
    for record in input["records"].as_array().ok_or("records")? {
        validate_record(record)?;
        let track_id = record["track_id"].as_u64().ok_or("track id")?;
        let (hash, duration) = baseline_ids
            .get(&track_id)
            .ok_or("targeted track is absent from baseline")?;
        if record["file_sha256"].as_str() != Some(hash)
            || (record["coverage_seconds"].as_f64().ok_or("coverage")? - duration).abs() > 0.1
            || records.insert(track_id, record.clone()).is_some()
        {
            return Err(
                "targeted record is duplicated or differs from baseline audio identity".into(),
            );
        }
    }
    if records.is_empty() {
        return Err("targeted input has no records".into());
    }
    if baseline_ids.len() == 13
        && (records.len() != 13
            || records.keys().copied().collect::<BTreeSet<_>>()
                != baseline_ids.keys().copied().collect::<BTreeSet<_>>())
    {
        return Err(
            "a thirteen-recording baseline requires the same complete targeted dataset".into(),
        );
    }
    Ok(records)
}

fn baseline_labels(state: &Value, classifier: &str) -> Result<Vec<String>> {
    let observation = state["observations"]
        .as_array()
        .ok_or("baseline observations")?
        .iter()
        .find(|observation| observation["classifier"] == classifier)
        .ok_or("baseline classifier")?;
    let labels = observation["labels"].as_array().ok_or("baseline labels")?;
    if labels.len() != 8 {
        return Err(
            "baseline instrument and style observations must each contain eight labels".into(),
        );
    }
    labels
        .iter()
        .map(|item| Ok(item["label"].as_str().ok_or("baseline label")?.to_owned()))
        .collect()
}

pub(super) fn validate_profile_labels(record: &Value, state: &Value) -> Result<()> {
    for classifier in ["instrument", "style"] {
        let expected = baseline_labels(state, classifier)?;
        for profile in record["profiles"].as_array().ok_or("profiles")? {
            let actual = profile[classifier]
                .as_array()
                .ok_or("profile classifier")?
                .iter()
                .map(|item| item["label"].as_str().ok_or("profile label"))
                .collect::<std::result::Result<Vec<_>, _>>()?;
            if actual != expected.iter().map(String::as_str).collect::<Vec<_>>() {
                return Err("profile labels must match exact baseline top-eight order".into());
            }
        }
    }
    Ok(())
}
