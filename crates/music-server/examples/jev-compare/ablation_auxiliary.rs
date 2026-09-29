//! Strict local validation for evidence reconstructed outside the Jev harness.
use super::*;

const CONTRASTS: [(&str, [&str; 2]); 12] = [
    ("instrumental_vs_sung", ["instrumental", "sung"]),
    ("acoustic_vs_electronic", ["acoustic", "electronic"]),
    ("sparse_vs_dense", ["sparse", "dense"]),
    ("gentle_vs_forceful", ["gentle", "forceful"]),
    ("bright_vs_dark_timbre", ["bright_timbre", "dark_timbre"]),
    ("sustained_vs_percussive", ["sustained", "percussive"]),
    (
        "small_vs_large_ensemble",
        ["small_ensemble", "large_ensemble"],
    ),
    ("strings_vs_keyboard", ["strings", "keyboard"]),
    ("winds_vs_drums", ["winds", "drums"]),
    ("slow_vs_fast_motion", ["slow_motion", "fast_motion"]),
    (
        "steady_vs_changing_arrangement",
        ["steady_arrangement", "changing_arrangement"],
    ),
    ("consonant_vs_dissonant", ["consonant", "dissonant"]),
];

fn exact_keys(value: &Value, expected: &[&str]) -> bool {
    value.as_object().is_some_and(|object| {
        object.len() == expected.len() && expected.iter().all(|key| object.contains_key(*key))
    })
}

fn finite_between(value: &Value, low: f64, high: f64) -> Option<f64> {
    value
        .as_f64()
        .filter(|number| number.is_finite() && (low..=high).contains(number))
}

pub(super) fn rounded3(value: f64) -> f64 {
    (value * 1000.0).round() / 1000.0
}

pub(super) fn validate(
    baseline: &Value,
    auxiliary: &Value,
    selected: &BTreeSet<u64>,
) -> Result<BTreeMap<u64, Value>> {
    if !exact_keys(auxiliary, &["schema_version", "recordings", "provenance"])
        || auxiliary["schema_version"] != "jev-audio-auxiliary/v1"
        || auxiliary["provenance"]
            .as_object()
            .is_none_or(serde_json::Map::is_empty)
        || serde_json::to_vec(auxiliary)?.len() > 4 * 1024 * 1024
    {
        return Err("invalid auxiliary document".into());
    }
    let baseline_recordings = baseline["recordings"]
        .as_array()
        .ok_or("baseline recordings")?;
    let mut result = BTreeMap::new();
    for auxiliary_recording in auxiliary["recordings"]
        .as_array()
        .ok_or("auxiliary recordings")?
    {
        if !exact_keys(
            auxiliary_recording,
            &[
                "track_id",
                "file_sha256",
                "danceability",
                "contrasts",
                "section_styles",
            ],
        ) {
            return Err("invalid auxiliary recording fields".into());
        }
        let track_id = auxiliary_recording["track_id"]
            .as_u64()
            .ok_or("auxiliary track id")?;
        if !selected.contains(&track_id) || result.contains_key(&track_id) {
            return Err("auxiliary recordings must exactly match selected tracks".into());
        }
        let matches = baseline_recordings
            .iter()
            .filter(|recording| recording["input"]["track_id"].as_u64() == Some(track_id))
            .collect::<Vec<_>>();
        if matches.len() != 1 {
            return Err("selected baseline recording missing or ambiguous".into());
        }
        let baseline_recording = matches[0];
        let hash = auxiliary_recording["file_sha256"]
            .as_str()
            .ok_or("auxiliary file hash")?;
        if hash.len() != 64
            || !hash.bytes().all(|byte| byte.is_ascii_hexdigit())
            || baseline_recording["file_sha256"] != hash
        {
            return Err("auxiliary source hash differs from baseline".into());
        }

        let danceability = &auxiliary_recording["danceability"];
        if !exact_keys(
            danceability,
            &[
                "mean_response",
                "low_response",
                "high_response",
                "coverage_seconds",
            ],
        ) {
            return Err("invalid danceability fields".into());
        }
        let mean =
            finite_between(&danceability["mean_response"], 0.0, 1.0).ok_or("danceability mean")?;
        let low =
            finite_between(&danceability["low_response"], 0.0, 1.0).ok_or("danceability low")?;
        let high =
            finite_between(&danceability["high_response"], 0.0, 1.0).ok_or("danceability high")?;
        let coverage = danceability["coverage_seconds"]
            .as_f64()
            .filter(|number| number.is_finite() && *number > 0.0)
            .ok_or("danceability coverage")?;
        let duration = baseline_recording["input"]["length_s"]
            .as_f64()
            .ok_or("baseline duration")?;
        if low > mean || mean > high || (coverage - duration).abs() > 0.1 {
            return Err("invalid danceability response range or coverage".into());
        }

        let contrasts = auxiliary_recording["contrasts"]
            .as_array()
            .ok_or("contrasts")?;
        if contrasts.len() > CONTRASTS.len() {
            return Err("too many contrast preferences".into());
        }
        let mut dimensions = BTreeSet::new();
        for contrast in contrasts {
            if !exact_keys(
                contrast,
                &["dimension", "preference", "agreement", "margin"],
            ) {
                return Err("invalid contrast fields".into());
            }
            let dimension = contrast["dimension"].as_str().ok_or("contrast dimension")?;
            let preference = contrast["preference"]
                .as_str()
                .ok_or("contrast preference")?;
            let allowed = CONTRASTS
                .iter()
                .find(|(candidate, _)| *candidate == dimension)
                .map(|(_, values)| values)
                .ok_or("unsupported contrast dimension")?;
            if !allowed.contains(&preference)
                || contrast["agreement"].as_u64() != Some(3)
                || finite_between(&contrast["margin"], -2.0, 2.0).is_none()
                || !dimensions.insert(dimension)
            {
                return Err("invalid or duplicate contrast preference".into());
            }
        }

        let styles = auxiliary_recording["section_styles"]
            .as_array()
            .ok_or("section styles")?;
        if styles.len() > 4 {
            return Err("too many section-salient styles".into());
        }
        let style_source = baseline_recording["learned"]["sources"]
            .as_array()
            .ok_or("learned sources")?
            .iter()
            .filter(|source| source["kind"] == "style")
            .collect::<Vec<_>>();
        if style_source.len() != 1 {
            return Err("baseline style source missing or ambiguous".into());
        }
        let mut source_styles = style_source[0]["labels"]
            .as_array()
            .ok_or("style labels")?
            .iter()
            .map(|label| {
                Ok((
                    label["label"].as_str().ok_or("style label")?.to_owned(),
                    label["mean_score"].as_f64().ok_or("style response")?,
                ))
            })
            .collect::<Result<Vec<_>>>()?;
        source_styles.sort_by(|(left_label, left), (right_label, right)| {
            right
                .total_cmp(left)
                .then_with(|| left_label.cmp(right_label))
        });
        let top_eight = source_styles
            .iter()
            .take(8)
            .map(|(label, _)| label.clone())
            .collect::<BTreeSet<_>>();
        let source_styles = source_styles.into_iter().collect::<BTreeMap<_, _>>();
        let mut seen_styles = BTreeSet::new();
        for style in styles {
            if !exact_keys(
                style,
                &[
                    "label",
                    "mean_response",
                    "top3_section_count",
                    "section_count",
                ],
            ) {
                return Err("invalid section-style fields".into());
            }
            let label = style["label"].as_str().ok_or("section style label")?;
            let mean = style["mean_response"]
                .as_f64()
                .filter(|number| number.is_finite() && (0.0..=1.0).contains(number))
                .ok_or("section style response")?;
            let expected = source_styles.get(label).ok_or("unknown section style")?;
            if top_eight.contains(label)
                || !seen_styles.insert(label)
                || (mean - rounded3(mean)).abs() > 1e-9
                || (mean - rounded3(*expected)).abs() > 0.001
                || !(1..=10).contains(
                    &style["top3_section_count"]
                        .as_u64()
                        .ok_or("top-three bin count")?,
                )
                || style["section_count"].as_u64() != Some(10)
            {
                return Err("invalid section-salient style".into());
            }
        }
        result.insert(track_id, auxiliary_recording.clone());
    }
    if result.keys().copied().collect::<BTreeSet<_>>() != *selected {
        return Err("auxiliary recordings must exactly match selected tracks".into());
    }
    Ok(result)
}
