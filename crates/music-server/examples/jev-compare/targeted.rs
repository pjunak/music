//! Frozen targeted comparison of beat, harmony and shared-encoder time profiles.
use super::*;
use std::collections::{BTreeMap, BTreeSet};

#[path = "targeted_cards.rs"]
mod cards;
#[path = "targeted_input.rs"]
pub(super) mod input;

const SCHEMA: &str = "jev-targeted-feature-experiment/v1";
const BASELINE_ARM: &str = "baseline";
const ARMS: [&str; 10] = [
    BASELINE_ARM,
    "beat",
    "harmony",
    "physical",
    "profile_means",
    "profile_timeline",
    "all_numeric",
    "all_described",
    "all_no_mood_head",
    "missing_all",
];
const MAX_REQUESTS: usize = 160;
const MAX_INPUT_UNITS: u64 = 6_000_000;
const MAX_SELECTED_TRACKS: usize = 15;
const MAX_BODY_BYTES: usize = 62_976;
const QUESTION_COUNT: usize = ablation::QUESTION_COUNT;
const NULL_STATE: &str = "No observations of this recording are available.";

fn rotated_arm(track_index: usize, offset: usize) -> &'static str {
    ARMS[(track_index + offset) % ARMS.len()]
}

pub(super) struct Plan {
    pub document: Value,
    pub comparisons: Vec<comparison::Comparison>,
}

fn state_for(arm: &str, material: &ablation::BaselineMaterial, record: &Value) -> Result<Value> {
    let mut state = material.state.clone();
    match arm {
        BASELINE_ARM => {}
        "beat" => {
            state["targeted_numeric_cards"] = cards::numeric_cards(record, &["beat"], false, false)?
        }
        "harmony" => {
            state["targeted_numeric_cards"] =
                cards::numeric_cards(record, &["harmony"], false, false)?
        }
        "physical" => {
            state["targeted_numeric_cards"] =
                cards::numeric_cards(record, &["beat", "harmony"], false, false)?
        }
        "profile_means" => state["targeted_profile_card"] = cards::means_card(record)?,
        "profile_timeline" => state["targeted_profile_card"] = cards::timeline_card(record, false)?,
        "all_numeric" | "all_no_mood_head" => {
            state["targeted_numeric_cards"] =
                cards::numeric_cards(record, &["beat", "harmony"], false, false)?;
            state["targeted_profile_card"] = cards::timeline_card(record, false)?;
        }
        "all_described" => {
            state["targeted_numeric_cards"] =
                cards::numeric_cards(record, &["beat", "harmony"], true, false)?;
            state["targeted_profile_card"] = cards::timeline_card(record, false)?;
        }
        "missing_all" => {
            state["targeted_numeric_cards"] =
                cards::numeric_cards(record, &["beat", "harmony"], false, true)?;
            state["targeted_profile_card"] = cards::timeline_card(record, true)?;
        }
        _ => return Err("unknown targeted arm".into()),
    }
    if arm == "all_no_mood_head" {
        let observations = state["observations"]
            .as_array_mut()
            .ok_or("baseline observations")?;
        let before = observations.len();
        observations.retain(|observation| observation["classifier"] != "mood_theme");
        if observations.len() + 1 != before {
            return Err("baseline must contain exactly one mood_theme observation".into());
        }
    }
    Ok(state)
}

fn request_for(
    arm: &'static str,
    material: &ablation::BaselineMaterial,
    record: &Value,
) -> Result<TypedDecisionRequest> {
    let request = TypedDecisionRequest {
        state: state_for(arm, material, record)?,
        questions: material.questions.clone(),
    };
    if request.questions.len() != QUESTION_COUNT {
        return Err("every targeted request must contain all 138 questions".into());
    }
    request.validate()?;
    let body_bytes = serde_json::to_vec(
        &json!({"model":MODEL,"state":request.state,"questions":request.questions}),
    )?
    .len();
    if body_bytes > MAX_BODY_BYTES {
        return Err("targeted request exceeds the fixed typed-body bound".into());
    }
    Ok(request)
}

fn push_request(
    comparisons: &mut Vec<comparison::Comparison>,
    mappings: &mut Vec<Value>,
    material: &ablation::BaselineMaterial,
    record: Option<&Value>,
    track_id: Option<u64>,
    arm: &'static str,
    repeat_control: bool,
) -> Result<()> {
    let request = if let Some(record) = record {
        request_for(arm, material, record)?
    } else {
        let request = TypedDecisionRequest {
            state: json!(NULL_STATE),
            questions: material.questions.clone(),
        };
        request.validate()?;
        request
    };
    let body_bytes = serde_json::to_vec(
        &json!({"model":MODEL,"state":request.state,"questions":request.questions}),
    )?
    .len();
    if body_bytes > MAX_BODY_BYTES {
        return Err("targeted request exceeds the fixed typed-body bound".into());
    }
    let index = comparisons.len();
    let no_op = request.state == material.state && request.questions == material.questions;
    if arm != BASELINE_ARM && no_op {
        return Err("targeted arm unexpectedly duplicates the baseline request".into());
    }
    mappings.push(json!({
        "request_index":index,
        "track_id":track_id,
        "arm":arm,
        "repeat_control":repeat_control,
        "control":if record.is_none() {"null_evidence"} else {"none"},
        "evidence_changed":request.state != material.state,
        "no_op":no_op,
        "feature_record_sha256":record.map(fingerprint).transpose()?,
        "baseline_evidence_sha256":material.evidence_sha256,
        "request_evidence_sha256":fingerprint(&request.state)?,
        "frozen_graded_questions_sha256":material.graded_questions_sha256,
        "baseline_questions_sha256":material.simple_questions_sha256,
        "request_questions_sha256":fingerprint(&serde_json::to_value(&request.questions)?)?,
        "question_count":request.questions.len(),
        "body_bytes":body_bytes,
    }));
    comparisons.push(comparison::Comparison {
        case_id: track_id.map_or_else(
            || "null-evidence-control".to_owned(),
            |id| format!("track-{id}"),
        ),
        partition: index,
        variant: arm,
        fit_threshold: 0.5,
        tags: material.tags.clone(),
        request,
    });
    Ok(())
}

fn public_provenance(input: &Value) -> Result<Value> {
    let mut provenance = input["provenance"].clone();
    provenance
        .as_object_mut()
        .ok_or("targeted provenance")?
        .remove("details");
    Ok(provenance)
}

pub(super) fn prepare(baseline: &Value, features: &Value, tracks: &[u64]) -> Result<Plan> {
    let simple = simple::prepare(baseline, tracks)?;
    let records = input::validate(baseline, features)?;
    let selected = tracks.iter().copied().collect::<BTreeSet<_>>();
    if tracks.is_empty() || tracks.len() > MAX_SELECTED_TRACKS || selected.len() != tracks.len() {
        return Err("select 1..15 distinct baseline track ids".into());
    }
    if selected.iter().any(|track| !records.contains_key(track)) {
        return Err("every selected track requires a validated targeted record".into());
    }
    let mut materials = BTreeMap::new();
    for track_id in &selected {
        let material = ablation::baseline_material(baseline, &simple, *track_id)?;
        input::validate_profile_labels(
            records.get(track_id).ok_or("targeted record")?,
            &material.state,
        )?;
        materials.insert(*track_id, material);
    }
    let first = materials.values().next().ok_or("baseline material")?;
    for material in materials.values().skip(1) {
        if material.simple_questions_sha256 != first.simple_questions_sha256
            || material.graded_questions_sha256 != first.graded_questions_sha256
            || material.tag_catalog != first.tag_catalog
        {
            return Err("selected tracks do not share exact baseline question identities".into());
        }
    }

    let mut comparisons = Vec::new();
    let mut mappings = Vec::new();
    for (track_index, track_id) in selected.iter().enumerate() {
        let material = materials.get(track_id).ok_or("baseline material")?;
        let record = records.get(track_id).ok_or("targeted record")?;
        for offset in 0..ARMS.len() {
            let arm = rotated_arm(track_index, offset);
            push_request(
                &mut comparisons,
                &mut mappings,
                material,
                Some(record),
                Some(*track_id),
                arm,
                false,
            )?;
        }
    }
    for track_id in &selected {
        push_request(
            &mut comparisons,
            &mut mappings,
            materials.get(track_id).ok_or("baseline material")?,
            Some(records.get(track_id).ok_or("targeted record")?),
            Some(*track_id),
            BASELINE_ARM,
            true,
        )?;
    }
    push_request(
        &mut comparisons,
        &mut mappings,
        first,
        None,
        None,
        BASELINE_ARM,
        false,
    )?;

    let units = comparisons
        .iter()
        .map(|comparison| model_request_reservation(&comparison.request.accounting_request(), 0))
        .sum::<u64>();
    let expected_requests = selected.len() * (ARMS.len() + 1) + 1;
    if comparisons.len() != expected_requests
        || comparisons.len() > MAX_REQUESTS
        || units > MAX_INPUT_UNITS
    {
        return Err("targeted experiment exceeds fixed coverage or budget bounds".into());
    }
    let document = json!({
        "schema_version":SCHEMA,
        "model":MODEL,
        "endpoint":ENDPOINT,
        "baseline_plan_sha256":fingerprint(baseline)?,
        "targeted_input_sha256":fingerprint(features)?,
        "targeted_provenance":public_provenance(features)?,
        "selected_tracks":selected,
        "recordings":simple.document["recordings"],
        "vocabulary":simple.document["vocabulary"],
        "arms":ARMS,
        "arm_definitions":{
            "baseline":"Exact concise Noul questions and exact frozen labels8 evidence.",
            "beat":"Baseline plus bounded beat-estimator measurements.",
            "harmony":"Baseline plus bounded harmony measurements.",
            "physical":"Baseline plus the same beat and harmony measurements together.",
            "profile_means":"Baseline plus time-weighted instrument and style profile means without a timeline.",
            "profile_timeline":"Baseline plus three equal-time instrument and style response profiles.",
            "all_numeric":"Baseline plus beat, harmony and the three-profile timeline.",
            "all_described":"All-numeric with short factual metric definitions; values, units, IDs and labels are identical.",
            "all_no_mood_head":"All-numeric with only the baseline mood_theme observation removed.",
            "missing_all":"Baseline retained with all added metric IDs and profile labels present but their values unavailable. Label presence itself remains information."
        },
        "tags":first.tag_catalog,
        "request_mappings":mappings,
        "comparisons":comparisons,
        "request_count":comparisons.len(),
        "max_input_units":units,
        "hard_limits":{"max_requests":MAX_REQUESTS,"max_input_units":MAX_INPUT_UNITS,
            "max_request_body_bytes":MAX_BODY_BYTES,"questions_per_request":QUESTION_COUNT,
            "selected_tracks":MAX_SELECTED_TRACKS},
        "profile_semantics":"Profile responses are uncalibrated outputs from the same shared audio encoder and are not independent corroboration.",
        "definition_semantics":"Metric definitions describe computation and units only; they do not establish musical mood or tag truth.",
        "question_semantics":"Every arm uses the exact same 138 concise Noul questions. Answers are probabilities of yes to the stated fit question, not tag intensity.",
        "purpose":"A paired, non-certifying ten-arm targeted feature comparison. Arm order rotates by sorted track ID. Each selected track has ten primary arms and one planned exact baseline repeat; one identity-free null control is shared.",
        "disclosure":"Only frozen baseline observations and validated targeted cards enter provider state. Provenance details, hashes, paths, identity and owner comments are excluded. No retries, automatic resume, writes, cutoff, winner, adoption, certification or quality gate."
    });
    // Persisted plans are parsed JSON Values. Normalize once here so exact
    // authorization is stable even when an input used an equivalent numeric spelling.
    let document = serde_json::from_slice(&serde_json::to_vec(&document)?)?;
    Ok(Plan {
        document,
        comparisons,
    })
}

fn authorize(plan: &Plan, saved: &Value, expected: &str, count: usize, units: u64) -> Result<()> {
    if &plan.document != saved
        || fingerprint(saved)? != expected
        || saved["request_count"].as_u64() != Some(count as u64)
        || saved["max_input_units"].as_u64() != Some(units)
        || count != plan.comparisons.len()
        || count > MAX_REQUESTS
        || units > MAX_INPUT_UNITS
    {
        return Err(
            "targeted experiment differs from the exact reviewed plan or fixed budget".into(),
        );
    }
    Ok(())
}

pub(super) fn load_authorized(
    baseline_path: &Path,
    features_path: &Path,
    saved_path: &Path,
    expected: &str,
    count: usize,
    units: u64,
) -> Result<Plan> {
    let baseline = pilot::read_json(baseline_path)?;
    let features = input::read(features_path)?;
    let saved = pilot::read_json(saved_path)?;
    let tracks: Vec<u64> = serde_json::from_value(saved["selected_tracks"].clone())?;
    let plan = prepare(&baseline, &features, &tracks)?;
    authorize(&plan, &saved, expected, count, units)?;
    for recording in plan.document["recordings"].as_array().ok_or("recordings")? {
        let path = Path::new(recording["source_path"].as_str().ok_or("source path")?);
        if pilot::audio_hash(path)? != recording["file_sha256"] {
            return Err("original changed since baseline and targeted extraction".into());
        }
    }
    Ok(plan)
}

pub(super) fn report(
    baseline_path: &Path,
    features_path: &Path,
    saved_path: &Path,
    journal_path: &Path,
) -> Result<Value> {
    const MAX_JOURNAL_BYTES: u64 = 256 * 1024 * 1024;
    let baseline = pilot::read_json(baseline_path)?;
    let features = input::read(features_path)?;
    let saved = pilot::read_json(saved_path)?;
    let tracks: Vec<u64> = serde_json::from_value(saved["selected_tracks"].clone())?;
    let plan = prepare(&baseline, &features, &tracks)?;
    if plan.document != saved {
        return Err("targeted report plan differs from exact offline reconstruction".into());
    }
    let mut bytes = Vec::new();
    File::open(journal_path)?
        .take(MAX_JOURNAL_BYTES + 1)
        .read_to_end(&mut bytes)?;
    if bytes.len() as u64 > MAX_JOURNAL_BYTES {
        return Err("targeted journal exceeds report bound".into());
    }
    report_text(&plan, std::str::from_utf8(&bytes)?)
}

fn report_text(plan: &Plan, text: &str) -> Result<Value> {
    let observed = journal::parse(&plan.document, &plan.comparisons, text)?;
    if !observed.complete
        || observed.pending.is_some()
        || observed.answers.len() != plan.comparisons.len()
        || observed.usage_by_request.len() != plan.comparisons.len()
    {
        return Err("targeted comparative export requires a complete journal; missing answers and usage remain unknown".into());
    }
    let mappings = plan.document["request_mappings"]
        .as_array()
        .ok_or("mappings")?;
    let mut requests = Vec::with_capacity(plan.comparisons.len());
    for index in 0..plan.comparisons.len() {
        let mut row = mappings.get(index).ok_or("mapping")?.clone();
        let object = row.as_object_mut().ok_or("mapping object")?;
        object.insert(
            "raw_typed_answers".into(),
            serde_json::to_value(observed.answers.get(&index).ok_or("answers")?)?,
        );
        let usage = observed.usage_by_request.get(&index).ok_or("usage")?;
        object.insert(
            "usage".into(),
            json!({"input_tokens":usage[0],"output_tokens":usage[1]}),
        );
        requests.push(row);
    }
    Ok(json!({
        "schema_version":"jev-targeted-feature-experiment-report/v1",
        "model":MODEL,
        "plan_sha256":fingerprint(&plan.document)?,
        "targeted_input_sha256":plan.document["targeted_input_sha256"],
        "targeted_provenance":plan.document["targeted_provenance"],
        "complete":true,
        "planned_requests":plan.comparisons.len(),
        "responses":observed.answers.len(),
        "usage":{"reported_input_tokens":observed.tokens[0],
            "reported_output_tokens":observed.tokens[1],
            "responses_reporting_input":observed.token_reports[0],
            "responses_reporting_output":observed.token_reports[1],
            "missing_input_reports":plan.comparisons.len()-observed.token_reports[0],
            "missing_output_reports":plan.comparisons.len()-observed.token_reports[1]},
        "selected_tracks":plan.document["selected_tracks"],
        "selected_recordings":plan.document["recordings"],
        "vocabulary":plan.document["vocabulary"],
        "tags":plan.document["tags"],
        "arms":plan.document["arm_definitions"],
        "profile_semantics":plan.document["profile_semantics"],
        "definition_semantics":plan.document["definition_semantics"],
        "question_semantics":plan.document["question_semantics"],
        "requests":requests,
        "interpretation":"Complete raw comparative export only. Missing usage remains unknown rather than zero. Added profile-label presence is information even when responses are unavailable. Repeats and the null control are retained. No cutoff, quality gate, winner, listening-accuracy claim, production adoption, certification, tag decision or library write follows."
    }))
}

#[cfg(test)]
#[path = "targeted_tests.rs"]
mod tests;
