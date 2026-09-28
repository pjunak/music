//! Explicit private inputs, offline preparation, and non-certifying full-engine comparisons.
use super::*;
use music_analysis::{AudioContextAnalyzer, FfmpegContextAnalyzer, VoiceContextPreparation};
use music_application::assistant::{
    AUDIO_PREDICTION_CONTRACT, AudioPredictionEvidence, AudioPredictionSource, JevTaggerTask,
    LOCAL_CONTEXT_ANALYZER_ID, LOCAL_CONTEXT_IMPLEMENTATION_ID, RankedAudioLabel,
    compact_context_evidence, default_vocabulary_snapshot,
};
use std::collections::BTreeSet;
use std::sync::atomic::AtomicBool;

#[path = "pilot_run.rs"]
mod execution;
pub(super) use execution::run;
#[path = "pilot_predictions.rs"]
pub(super) mod predictions;
#[cfg(test)]
#[path = "pilot_tests.rs"]
pub(super) mod tests;

const CORPUS_SCHEMA: &str = "jev-private-corpus/v1";
const PLAN_SCHEMA: &str = "jev-private-pilot/v2";
const ARMS: [&str; 8] = [
    "physical",
    "mood",
    "instrument_style",
    "learned",
    "combined",
    "combined_top3",
    "combined_scored",
    "combined_temporal",
];
const MAX_REQUESTS: u64 = 1_200;
const MAX_UNITS: u64 = 60_000_000;

pub(super) fn read_json(path: &Path) -> Result<Value> {
    if path.metadata()?.len() > 32 * 1024 * 1024 {
        return Err("pilot document exceeds 32 MiB".into());
    }
    Ok(serde_json::from_reader(File::open(path)?)?)
}

pub(super) fn audio_hash(path: &Path) -> Result<String> {
    let mut file = File::open(path)?;
    let mut digest = Sha256::new();
    let mut buffer = [0_u8; 65536];
    loop {
        let count = file.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        digest.update(&buffer[..count]);
    }
    Ok(format!("{:x}", digest.finalize()))
}

pub(super) fn analyze(paths: &Path, output: &Path, ffmpeg: &Path, ffprobe: &Path) -> Result<()> {
    let paths: Vec<PathBuf> = serde_json::from_value(read_json(paths)?)?;
    if paths.is_empty() || paths.len() > 32 || output.exists() {
        return Err("supply 1..32 explicit audio files and a new output".into());
    }
    let analyzer = FfmpegContextAnalyzer::new(ffmpeg, ffprobe);
    let metadata = music_media::MetadataAdapter::native_only();
    let mut recordings = Vec::new();
    let mut seen = BTreeSet::new();
    for (index, path) in paths.iter().enumerate() {
        let path = path.canonicalize()?;
        let hash = audio_hash(&path)?;
        if !seen.insert(hash.clone()) {
            return Err("duplicate audio content in pilot".into());
        }
        let tags = metadata.read(&path)?;
        let document = analyzer.analyze(
            &path,
            &AtomicBool::new(false),
            VoiceContextPreparation::NotConfigured,
        )?;
        if audio_hash(&path)? != hash {
            return Err("audio changed during analysis".into());
        }
        let mut context = Value::Object(document.summary.clone());
        context["analyzer_id"] = json!(LOCAL_CONTEXT_ANALYZER_ID);
        context["completeness"] = json!(document.completeness);
        context["sections"] = json!(document.sections);
        let input = json!({"track_id":index as i64 + 1, "artist":tags.artist,
            "album":tags.album,"genre":tags.genre,"origin":"","length_s":document.performance.audio_seconds,
            "bpm":tags.bpm,"context_evidence":compact_context_evidence(&context),
            "catalog_evidence":null,"evidence_contract":"song-evidence/v1"});
        // Filenames and folder labels are local listening aids, never model observations.
        recordings.push(json!({"source_path":path,"file_sha256":hash,
            "display_name":path.file_name().and_then(|v| v.to_str()),
            "collection":path.parent().and_then(Path::file_name).and_then(|v| v.to_str()),
            "input":input,"analysis_seconds":document.performance.elapsed_seconds}));
        println!("Analyzed {}/{}", index + 1, paths.len());
    }
    let corpus = json!({"schema_version":CORPUS_SCHEMA,
        "analyzer_id":LOCAL_CONTEXT_ANALYZER_ID,"implementation_id":LOCAL_CONTEXT_IMPLEMENTATION_ID,
        "voice":"not_configured","catalog":"not_retrieved","recordings":recordings});
    serde_json::to_writer_pretty(new_file(output)?, &corpus)?;
    Ok(())
}

fn task(recording: &Value, arm: &str) -> Result<JevTaggerTask> {
    if !ARMS.contains(&arm) {
        return Err("unknown pilot arm".into());
    }
    let input = predictions::input(recording, arm)?;
    plan_jev_tagging(&[input], &default_vocabulary_snapshot()?)?
        .pop()
        .ok_or_else(|| "missing pilot task".into())
}

fn assessment_request(request: &TypedDecisionRequest, _arm: &str) -> Result<TypedDecisionRequest> {
    let mut compact = request.clone();
    compact.state = evidence::variants::transform("compact_cards", &request.state, &Value::Null)?;
    compact.validate()?;
    if model_request_reservation(&compact.accounting_request(), 0)
        > model_request_reservation(&request.accounting_request(), 0)
    {
        return Err("compaction exceeds the native request reservation".into());
    }
    Ok(compact)
}

pub(super) fn plan(corpus: &Path, predictions: &Path, tracks: &[i64]) -> Result<Value> {
    let mut export = String::new();
    File::open(predictions)?
        .take(32 * 1024 * 1024 + 1)
        .read_to_string(&mut export)?;
    if export.len() > 32 * 1024 * 1024 {
        return Err("pilot document exceeds 32 MiB".into());
    }
    let corpus = predictions::attach(read_json(corpus)?, &export)?;
    build_plan(&corpus, tracks)
}

fn build_plan(corpus: &Value, tracks: &[i64]) -> Result<Value> {
    if corpus["schema_version"] != CORPUS_SCHEMA
        || corpus["analyzer_id"] != LOCAL_CONTEXT_ANALYZER_ID
        || corpus["implementation_id"] != LOCAL_CONTEXT_IMPLEMENTATION_ID
        || tracks.is_empty()
        || tracks.len() > 8
        || tracks.iter().collect::<BTreeSet<_>>().len() != tracks.len()
    {
        return Err("pilot needs current analysis and 1..8 distinct selected recordings".into());
    }
    let records = corpus["recordings"].as_array().ok_or("recordings")?;
    let mut selected = Vec::new();
    let mut hashes = BTreeSet::new();
    for id in tracks {
        let matches = records
            .iter()
            .filter(|r| r["input"]["track_id"] == *id)
            .collect::<Vec<_>>();
        if matches.len() != 1 {
            return Err("selected recording missing or ambiguous".into());
        }
        let recording = matches[0];
        predictions::validate_recording(recording)?;
        let hash = recording["file_sha256"].as_str().ok_or("audio hash")?;
        if hash.len() != 64 || !hash.bytes().all(|v| v.is_ascii_hexdigit()) || !hashes.insert(hash)
        {
            return Err("missing or duplicate recording identity".into());
        }
        selected.push(recording.clone());
    }
    let conformance = typed_conformance_request("private-pilot");
    let mut calls = 1;
    let mut units = model_request_reservation(&conformance.accounting_request(), 0);
    let mut cases = Vec::new();
    let mut add = |recording: &Value, arm: &str, repeat: bool| -> Result<()> {
        let task = task(recording, arm)?;
        let assessment = task
            .assessment_requests()
            .iter()
            .map(|r| assessment_request(r, arm))
            .collect::<Result<Vec<_>>>()?;
        calls += task.max_requests as u64;
        units += task.token_reservation;
        let state_bytes = assessment
            .iter()
            .map(|r| r.state.to_string().len())
            .sum::<usize>();
        let question_bytes = assessment
            .iter()
            .map(|r| serde_json::to_vec(&r.questions).map(|v| v.len()))
            .collect::<std::result::Result<Vec<_>, _>>()?
            .iter()
            .sum::<usize>();
        cases.push(json!({"track_id":recording["input"]["track_id"],"arm":arm,"repeat_control":repeat,
            "assessment":assessment,"assessment_state_bytes":state_bytes,"assessment_question_bytes":question_bytes,
            "max_requests":task.max_requests,"max_input_units":task.token_reservation}));
        Ok(())
    };
    for (index, recording) in selected.iter().enumerate() {
        for offset in 0..ARMS.len() {
            add(recording, ARMS[(index + offset) % ARMS.len()], false)?;
        }
    }
    // Deliberate repeats expose model variation; failures never trigger an automatic retry.
    add(&selected[0], "combined", true)?;
    if selected.len() > 1 {
        add(&selected[selected.len() - 1], "combined", true)?;
    }
    if calls > MAX_REQUESTS || units > MAX_UNITS {
        return Err("pilot exceeds fixed developer ceiling".into());
    }
    Ok(
        json!({"schema_version":PLAN_SCHEMA,"engine_id":JEV_TAGGER_CONTRACT,
        "inference_identity":jev_inference_identity(),"model":MODEL,"endpoint":ENDPOINT,
        "implementation":"learned-source-rank-score-time-combinations/v1",
        "prediction_contract":AUDIO_PREDICTION_CONTRACT,"arms":ARMS,
        "sampling_rule":"Explicit preselected development tracks; reuse the prior pilot selection for paired comparisons. Predictions shown to the owner are assisted feedback, never blind confirmation.",
        "vocabulary":default_vocabulary_snapshot()?.document,"corpus":{
            "schema_version":CORPUS_SCHEMA,"analyzer_id":LOCAL_CONTEXT_ANALYZER_ID,
            "implementation_id":LOCAL_CONTEXT_IMPLEMENTATION_ID,"recordings":selected},
        "selected_tracks":tracks,"conformance":conformance,"cases":cases,
        "max_requests":calls,"max_input_units":units,"certifies_model":false,
        "assessment_mode":"assisted_development_diagnostic",
        "disclosure":"Only embedded metadata, locally measured evidence and selected audio-classifier predictions go to pinned Jev. No audio, paths, filenames, collection labels, content hashes, listener judgments or app credentials are sent. Default vocabulary; optional voice and catalog evidence absent. Classifier scores are uncalibrated, never probabilities or verified tags. All source, amount and representation omissions apply to both matching and grounding. Assessment compaction and questions are identical across arms. Full native candidate, support/conflict and period gates remain. Requests reserve conservative uncompressed native bounds. No retries, no app acceptance or tag writes."}),
    )
}

fn authorize(plan: &Value, expected: &str, calls: usize, units: u64) -> Result<()> {
    let tracks: Vec<i64> = serde_json::from_value(plan["selected_tracks"].clone())?;
    let rebuilt = build_plan(&plan["corpus"], &tracks)?;
    if &rebuilt != plan
        || fingerprint(plan)? != expected
        || plan["max_requests"].as_u64() != Some(calls as u64)
        || plan["max_input_units"].as_u64() != Some(units)
    {
        return Err("private pilot differs from its exact reviewed plan or budget".into());
    }
    Ok(())
}
