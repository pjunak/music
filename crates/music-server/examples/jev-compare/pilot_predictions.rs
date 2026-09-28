//! Source identity checks and paired source/representation ablations, entirely offline.
use super::*;
use serde_json::value::RawValue;
use std::collections::BTreeMap;

// Graph, metadata and ordered class-array hashes are pinned independently. The last
// hash catches a renamed/reordered score vector even if metadata hash claims survived.
pub(super) const MODELS: [(&str, &str, &str, &str, &str, usize); 3] = [
    (
        "instrument",
        "mtg_jamendo_instrument-discogs-effnet-1",
        "9ae2d9e763d66bd8eed654d1ac3aa171e6539cb8a0e11f3dcd53df1428980802",
        "7d02204c6451b5615e2968ec6364bbae3b915c886e608f05f00d3a38dc5177c4",
        "02636df9eb46c754c057a443f72890fdf431819e9efb198e8e368e846888699e",
        40,
    ),
    (
        "mood_theme",
        "mtg_jamendo_moodtheme-discogs-effnet-1",
        "7d6270acaa5f4bba4b115a0d6849aca05ed6bd153dcb6d9da4f6ab9f99ef10ff",
        "d62cd90263e4d613fa7fcce7a831e339450394794af63685f96e065c1a896ab0",
        "b7e6628ca5ceb51ffe7ef5e17d6538c1ac2d77346e26685eee193c018c20811c",
        56,
    ),
    (
        "style",
        "discogs-effnet-bsdynamic-1",
        "a280825b334797cf677939db8cd5762c0392aedd0ca6415dbc1cd083f045e43c",
        "a2e85b2e7372d5f8e0f35bdd6aeae1139f101087d183d0b2fb60b0ea0f01a0ff",
        "80974bd84b6bd87a455fa3dedaf2be34673d4c672d8b898558ad1713baf0a78e",
        400,
    ),
];

pub(super) fn attach(mut corpus: Value, export_json: &str) -> Result<Value> {
    // Validate JSON/depth before traversing RawValue. Export hashes use JavaScript's
    // number spelling, which must survive Rust parsing and float serialization.
    let export: Value = serde_json::from_str(export_json)?;
    if export["schema_version"] != "song-audio-predictions/v1"
        || export["score_kind"] != "uncalibrated_sigmoid"
        || export["aggregation"]
            != json!({"patch_schedule":"nearest_patch_center_time_partition/v1",
            "top_rank_limit":3,"opening_fraction":0.1,"ending_fraction":0.1})
    {
        return Err("unsupported learned prediction export".into());
    }
    let records = export["recordings"]
        .as_array()
        .ok_or("prediction recordings")?;
    let raw_export: BTreeMap<String, &RawValue> = serde_json::from_str(export_json)?;
    let raw_records: Vec<&RawValue> = serde_json::from_str(
        raw_export
            .get("recordings")
            .ok_or("prediction recordings")?
            .get(),
    )?;
    let mut prediction_hashes = Vec::with_capacity(raw_records.len());
    for record in raw_records {
        let mut prediction: BTreeMap<String, &RawValue> = serde_json::from_str(record.get())?;
        let claimed: String = serde_json::from_str(
            prediction
                .remove("prediction_sha256")
                .ok_or("missing prediction digest")?
                .get(),
        )?;
        let actual = format!(
            "{:x}",
            Sha256::digest(canonical_export_object(&prediction)?)
        );
        if claimed != actual {
            return Err("prediction digest mismatch".into());
        }
        prediction_hashes.push(claimed);
    }
    if export["recording_set_sha256"] != fingerprint(&json!(prediction_hashes))? {
        return Err("prediction recording-set digest mismatch".into());
    }
    let hashes = records
        .iter()
        .map(|r| r["file_sha256"].as_str().ok_or("prediction hash"))
        .collect::<std::result::Result<BTreeSet<_>, _>>()?;
    let inputs = corpus["recordings"]
        .as_array_mut()
        .ok_or("corpus recordings")?;
    if records.len() != inputs.len() || hashes.len() != records.len() || records.len() > 32 {
        return Err("prediction and audio selection membership differ".into());
    }
    for recording in inputs {
        let matching = records
            .iter()
            .find(|r| r["file_sha256"] == recording["file_sha256"])
            .ok_or("no predictions for this audio hash")?;
        recording["learned"] = matching.clone();
        validate_recording(recording)?;
    }
    Ok(corpus)
}

fn canonical_export_object(fields: &BTreeMap<String, &RawValue>) -> Result<String> {
    let entries = fields
        .iter()
        .map(|(key, value)| {
            Ok(format!(
                "{}:{}",
                serde_json::to_string(key)?,
                canonical_export_json(value)?
            ))
        })
        .collect::<Result<Vec<_>>>()?;
    Ok(format!("{{{}}}", entries.join(",")))
}

fn canonical_export_json(value: &RawValue) -> Result<String> {
    let raw = value.get();
    match raw.as_bytes().first() {
        Some(b'{') => canonical_export_object(&serde_json::from_str(raw)?),
        Some(b'[') => {
            let values: Vec<&RawValue> = serde_json::from_str(raw)?;
            let entries = values
                .iter()
                .map(|value| canonical_export_json(value))
                .collect::<Result<Vec<_>>>()?;
            Ok(format!("[{}]", entries.join(",")))
        }
        Some(b'"') => Ok(serde_json::to_string(&serde_json::from_str::<String>(
            raw,
        )?)?),
        _ => Ok(raw.to_owned()),
    }
}

pub(super) fn validate_recording(recording: &Value) -> Result<()> {
    let learned = &recording["learned"];
    if learned["file_sha256"] != recording["file_sha256"] {
        return Err("learned evidence belongs to different audio".into());
    }
    if learned["pcm_sha256"]
        .as_str()
        .is_none_or(|hash| hash.len() != 64 || !hash.bytes().all(|byte| byte.is_ascii_hexdigit()))
    {
        return Err("learned evidence lacks its decoded audio identity".into());
    }
    let duration = learned["duration_seconds"]
        .as_f64()
        .ok_or("learned duration")?;
    let measured = recording["input"]["length_s"]
        .as_f64()
        .ok_or("measured duration")?;
    if !duration.is_finite()
        || duration <= 0.0
        || duration > 86_400.0
        || (duration - measured).abs() > 0.1 + measured * 0.001
        || learned["covered_seconds"].as_f64() != Some(duration)
    {
        return Err("learned evidence lacks matching whole-track coverage".into());
    }
    let sources = learned["sources"].as_array().ok_or("learned sources")?;
    if sources.len() != MODELS.len() {
        return Err("expected all three pinned prediction heads".into());
    }
    for (kind, model, graph_hash, taxonomy_hash, labels_hash, count) in MODELS {
        let matches = sources
            .iter()
            .filter(|s| s["kind"] == kind)
            .collect::<Vec<_>>();
        if matches.len() != 1 {
            return Err("missing or duplicate learned source".into());
        }
        let source = matches[0];
        if source["model_id"] != model
            || source["model_sha256"] != graph_hash
            || source["taxonomy_sha256"] != taxonomy_hash
            || source["top_rank_limit"] != 3
            || source["score_kind"] != "uncalibrated_sigmoid"
            || source["duration_seconds"].as_f64() != Some(duration)
            || source["covered_seconds"].as_f64() != Some(duration)
        {
            return Err("learned model identity or coverage mismatch".into());
        }
        let labels = source["labels"].as_array().ok_or("learned labels")?;
        let names = labels
            .iter()
            .map(|v| v["label"].clone())
            .collect::<Vec<_>>();
        if labels.len() != count || fingerprint(&json!(names))? != labels_hash {
            return Err("learned taxonomy order mismatch".into());
        }
        // Validate every raw score, including candidates outside the provider shortlist.
        for label in labels {
            if [
                "mean_score",
                "max_score",
                "top_rank_fraction",
                "opening_score",
                "ending_score",
            ]
            .iter()
            .any(|key| label[*key].as_f64().is_none())
            {
                return Err("learned export is missing a raw score".into());
            }
            let mut ranked = label.clone();
            ranked["rank"] = json!(1);
            let compact = json!({"schema_version":AUDIO_PREDICTION_CONTRACT,"sources":[{
                "kind":kind,"model_id":model,"score_kind":"uncalibrated_sigmoid",
                "duration_seconds":duration,"covered_seconds":duration,"labels":[ranked]}]});
            AudioPredictionEvidence::parse(&compact)?;
        }
    }
    Ok(())
}

pub(super) fn input(recording: &Value, arm: &str) -> Result<Value> {
    let mut input = recording["input"].clone();
    input
        .as_object_mut()
        .ok_or("input")?
        .remove("audio_predictions");
    if arm == "physical" {
        return Ok(input);
    }
    if matches!(arm, "mood" | "instrument_style" | "learned") {
        input["context_evidence"] = Value::Null;
    }
    let limit = if arm == "combined_top3" { 3 } else { 6 };
    let mut sources = Vec::new();
    for source in recording["learned"]["sources"]
        .as_array()
        .ok_or("learned sources")?
    {
        let kind = source["kind"].as_str().ok_or("kind")?;
        if (arm == "mood" && kind != "mood_theme")
            || (arm == "instrument_style" && kind == "mood_theme")
        {
            continue;
        }
        let mut labels = source["labels"].as_array().ok_or("labels")?.clone();
        labels.sort_by(|a, b| {
            b["mean_score"]
                .as_f64()
                .unwrap_or(-1.0)
                .total_cmp(&a["mean_score"].as_f64().unwrap_or(-1.0))
                .then_with(|| a["label"].as_str().cmp(&b["label"].as_str()))
        });
        let labels = labels
            .iter()
            .take(limit)
            .enumerate()
            .map(|(index, label)| {
                let temporal = arm == "combined_temporal";
                let numeric = temporal || arm == "combined_scored";
                Ok(RankedAudioLabel {
                    rank: index + 1,
                    label: label["label"].as_str().ok_or("label")?.to_owned(),
                    mean_score: numeric.then(|| score(&label["mean_score"])).flatten(),
                    max_score: temporal.then(|| score(&label["max_score"])).flatten(),
                    top_rank_fraction: temporal
                        .then(|| score(&label["top_rank_fraction"]))
                        .flatten(),
                    opening_score: temporal.then(|| score(&label["opening_score"])).flatten(),
                    ending_score: temporal.then(|| score(&label["ending_score"])).flatten(),
                })
            })
            .collect::<Result<Vec<_>>>()?;
        sources.push(AudioPredictionSource {
            kind: kind.to_owned(),
            model_id: source["model_id"].as_str().ok_or("model")?.to_owned(),
            score_kind: "uncalibrated_sigmoid".into(),
            duration_seconds: source["duration_seconds"].as_f64().ok_or("duration")?,
            covered_seconds: source["covered_seconds"].as_f64().ok_or("coverage")?,
            labels,
        });
    }
    // Stable source order keeps JSON ordering from becoming another experimental factor.
    sources.sort_by(|a, b| a.kind.cmp(&b.kind));
    let evidence = serde_json::to_value(AudioPredictionEvidence {
        schema_version: AUDIO_PREDICTION_CONTRACT.into(),
        sources,
    })?;
    AudioPredictionEvidence::parse(&evidence)?;
    input["audio_predictions"] = evidence;
    Ok(input)
}

fn score(value: &Value) -> Option<f64> {
    // Preserve exact raw values in the local artifact, not spurious decimal precision in text input.
    value.as_f64().map(|v| (v * 10_000.0).round() / 10_000.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn import_checks_record_and_recording_set_integrity() -> Result<()> {
        let corpus = super::super::tests::corpus()?;
        let records = corpus["recordings"]
            .as_array()
            .ok_or("recordings")?
            .iter()
            .map(|recording| {
                let mut prediction = recording["learned"].clone();
                prediction["prediction_sha256"] = json!(fingerprint(&prediction)?);
                Ok(prediction)
            })
            .collect::<Result<Vec<_>>>()?;
        let digests = records
            .iter()
            .map(|record| record["prediction_sha256"].clone())
            .collect::<Vec<_>>();
        let export = json!({"schema_version":"song-audio-predictions/v1",
            "score_kind":"uncalibrated_sigmoid", "recording_set_sha256":fingerprint(&json!(digests))?,
            "aggregation":{"patch_schedule":"nearest_patch_center_time_partition/v1",
                "top_rank_limit":3,"opening_fraction":0.1,"ending_fraction":0.1},
            "recordings":records});
        attach(corpus.clone(), &serde_json::to_string_pretty(&export)?)?;
        for (pointer, value) in [
            ("/recordings/0/pcm_sha256", json!("b".repeat(64))),
            ("/recordings/0/sources/0/labels/0/mean_score", json!(0.21)),
            ("/recordings/0/prediction_sha256", json!("0".repeat(64))),
            ("/recording_set_sha256", json!("0".repeat(64))),
        ] {
            let mut changed = export.clone();
            *changed.pointer_mut(pointer).ok_or("pointer")? = value;
            assert!(
                attach(corpus.clone(), &changed.to_string()).is_err(),
                "{pointer}"
            );
        }
        let mut reordered = export;
        reordered["recordings"]
            .as_array_mut()
            .ok_or("recordings")?
            .swap(0, 1);
        assert!(attach(corpus, &reordered.to_string()).is_err());
        Ok(())
    }

    #[test]
    fn export_integrity_preserves_original_decimal_spelling() -> Result<()> {
        let raw: &RawValue =
            serde_json::from_str(r#"{ "z": [0.000001,1.2345678901234568e-7,60], "a": "flute" }"#)?;
        assert_eq!(
            canonical_export_json(raw)?,
            r#"{"a":"flute","z":[0.000001,1.2345678901234568e-7,60]}"#
        );
        Ok(())
    }

    #[test]
    fn omitted_sources_and_scores_cannot_reappear_in_grounding() -> Result<()> {
        let recording = super::super::tests::recording()?;
        for arm in ARMS {
            let task = task(&recording, arm)?;
            for request in task
                .assessment_requests()
                .iter()
                .chain(task.grounding_requests([0])?.iter())
            {
                let observed = request.state["observations"]
                    .as_object()
                    .ok_or("observations")?;
                for (id, card) in observed {
                    if arm == "physical" {
                        assert!(!id.starts_with("prediction."));
                    }
                    if matches!(arm, "mood" | "instrument_style" | "learned") {
                        assert!(!id.starts_with("audio."));
                    }
                    if arm == "mood" {
                        assert_ne!(id, "prediction.instrument");
                        assert_ne!(id, "prediction.style");
                    }
                    if arm == "instrument_style" {
                        assert_ne!(id, "prediction.mood_theme");
                    }
                    if id.starts_with("prediction.") {
                        let labels = card["value"]["labels"].as_array().ok_or("labels")?;
                        assert_eq!(labels.len(), if arm == "combined_top3" { 3 } else { 6 });
                        assert_eq!(
                            labels[0].get("mean_score").is_some(),
                            matches!(arm, "combined_scored" | "combined_temporal")
                        );
                        assert_eq!(
                            labels[0].get("ending_score").is_some(),
                            arm == "combined_temporal"
                        );
                        assert!(
                            card["meaning"]
                                .as_str()
                                .unwrap_or("")
                                .contains("uncalibrated")
                        );
                    }
                }
            }
        }
        Ok(())
    }
}
