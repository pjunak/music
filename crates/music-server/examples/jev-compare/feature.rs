//! Frozen, bounded feature-family experiment over the exact short-Noul baseline.
use super::*;
use std::collections::{BTreeMap, BTreeSet};

#[path = "feature_cards.rs"]
mod feature_cards;
#[path = "feature_format.rs"]
pub(super) mod feature_format;
#[path = "feature_input.rs"]
mod feature_input;

const SCHEMA: &str = "jev-acoustic-feature-experiment/v1";
const BASELINE_ARM: &str = "baseline";
const ARMS: [&str; 12] = [
    BASELINE_ARM,
    "rhythm",
    "texture",
    "development",
    "affect",
    "combined_acoustics",
    "combined_acoustics_bands",
    "affect_bands",
    "combined_all",
    "combined_all_bands",
    "combined_all_no_mood_head",
    "features_only",
];
const MAX_REQUESTS: usize = 200;
const MAX_INPUT_UNITS: u64 = 8_000_000;
const MAX_SELECTED_TRACKS: usize = 15;
const MAX_BODY_BYTES: usize = 62_976;
const QUESTION_COUNT: usize = ablation::QUESTION_COUNT;
const NULL_STATE: &str = "No observations of this recording are available.";

pub(super) struct Plan {
    pub document: Value,
    pub comparisons: Vec<comparison::Comparison>,
}

fn arm_families(arm: &str) -> Result<(&'static [&'static str], bool)> {
    Ok(match arm {
        BASELINE_ARM => (&[], false),
        "rhythm" => (&["rhythm"], false),
        "texture" => (&["texture"], false),
        "development" => (&["development"], false),
        "affect" => (&["affect"], false),
        "combined_acoustics" => (&["rhythm", "texture", "development"], false),
        "combined_acoustics_bands" => (&["rhythm", "texture", "development"], true),
        "affect_bands" => (&["affect"], true),
        "combined_all" => (&["rhythm", "texture", "development", "affect"], false),
        "combined_all_bands" => (&["rhythm", "texture", "development", "affect"], true),
        "combined_all_no_mood_head" | "features_only" => {
            (&["rhythm", "texture", "development", "affect"], false)
        }
        _ => return Err("unknown feature arm".into()),
    })
}

fn baseline_observation_change(arm: &str) -> Result<&'static str> {
    Ok(match arm {
        "combined_all_no_mood_head" => "remove_mood_theme",
        "features_only" => "remove_all_learned_observations",
        arm if ARMS.contains(&arm) => "none",
        _ => return Err("unknown feature arm".into()),
    })
}

fn request_for(
    arm: &'static str,
    material: &ablation::BaselineMaterial,
    record: &Value,
) -> Result<TypedDecisionRequest> {
    let (families, bands) = arm_families(arm)?;
    let mut state = material.state.clone();
    if !families.is_empty() {
        let cards = families
            .iter()
            .map(|family| feature_cards::family_card(record, family, bands))
            .collect::<Result<Vec<_>>>()?;
        state
            .as_object_mut()
            .ok_or("baseline evidence must be an object")?
            .insert("feature_observations".into(), json!(cards));
    }
    match baseline_observation_change(arm)? {
        "remove_mood_theme" => {
            let observations = state["observations"]
                .as_array_mut()
                .ok_or("baseline observations")?;
            let before = observations.len();
            observations.retain(|observation| observation["classifier"] != "mood_theme");
            if observations.len() + 1 != before {
                return Err("baseline must contain exactly one mood_theme observation".into());
            }
            state["feature_scope"] = json!(
                "The baseline mood-theme classifier observation is intentionally unavailable in this arm. Instrument and style classifier observations and all four numeric feature families remain. Missing mood-theme labels are unknown, not negative evidence."
            );
        }
        "remove_all_learned_observations" => {
            state["observations"] = json!([]);
            state["feature_scope"] = json!(
                "No learned style, instrument, or mood classifier labels are provided in this arm. Only the four bounded numeric feature families are available. Missing learned labels are unknown, not negative evidence; numeric measurements do not directly assert tag identities."
            );
        }
        "none" => {}
        _ => unreachable!(),
    }
    let request = TypedDecisionRequest {
        state,
        questions: material.questions.clone(),
    };
    if request.questions.len() != QUESTION_COUNT {
        return Err("every feature request must contain all 138 questions".into());
    }
    request.validate()?;
    let body_bytes = serde_json::to_vec(
        &json!({"model":MODEL,"state":request.state,"questions":request.questions}),
    )?
    .len();
    if body_bytes > MAX_BODY_BYTES {
        return Err("feature request exceeds the fixed typed-body bound".into());
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
        return Err("feature request exceeds the fixed typed-body bound".into());
    }
    let index = comparisons.len();
    let no_op = request.state == material.state && request.questions == material.questions;
    if arm != BASELINE_ARM && no_op {
        return Err("feature arm unexpectedly duplicates the baseline request".into());
    }
    let (families, bands) = arm_families(arm)?;
    let observation_change = baseline_observation_change(arm)?;
    mappings.push(json!({
        "request_index":index,
        "track_id":track_id,
        "arm":arm,
        "repeat_control":repeat_control,
        "control":if record.is_none() {"null_evidence"} else {"none"},
        "feature_families":families,
        "feature_representation":if families.is_empty() {"none"} else if bands {"fixed_numeric_intervals"} else {"raw_numeric"},
        "baseline_observation_change":observation_change,
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

pub(super) fn prepare(baseline: &Value, features: &Value, tracks: &[u64]) -> Result<Plan> {
    let simple = simple::prepare(baseline, tracks)?;
    let feature_records = feature_input::validate(baseline, features)?;
    let selected = tracks.iter().copied().collect::<BTreeSet<_>>();
    if tracks.is_empty() || tracks.len() > MAX_SELECTED_TRACKS || selected.len() != tracks.len() {
        return Err("select 1..15 distinct baseline track ids".into());
    }
    if selected
        .iter()
        .any(|track| !feature_records.contains_key(track))
    {
        return Err("every selected track requires all four validated feature families".into());
    }
    let mut materials = BTreeMap::new();
    for track_id in &selected {
        materials.insert(
            *track_id,
            ablation::baseline_material(baseline, &simple, *track_id)?,
        );
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
        let material = materials.get(track_id).ok_or("track material")?;
        let record = feature_records.get(track_id).ok_or("track features")?;
        for offset in 0..ARMS.len() {
            let arm = ARMS[(track_index + offset) % ARMS.len()];
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
            materials.get(track_id).ok_or("track material")?,
            Some(feature_records.get(track_id).ok_or("track features")?),
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
        return Err("feature experiment exceeds fixed coverage or budget bounds".into());
    }
    let document = json!({
        "schema_version":SCHEMA,
        "model":MODEL,
        "endpoint":ENDPOINT,
        "baseline_plan_sha256":fingerprint(baseline)?,
        "feature_input_sha256":fingerprint(features)?,
        "feature_provenance":features["provenance"],
        "selected_tracks":selected,
        "recordings":simple.document["recordings"],
        "vocabulary":simple.document["vocabulary"],
        "arms":ARMS,
        "arm_definitions":{
            "baseline":"Exact concise Noul questions and exact frozen labels8 evidence, merged across all seven partitions.",
            "rhythm":"Baseline plus raw bounded rhythm measurements.",
            "texture":"Baseline plus raw bounded texture and spectral measurements.",
            "development":"Baseline plus raw bounded dynamics and ten-bin development measurements.",
            "affect":"Baseline plus raw normalized affect-model estimates.",
            "combined_acoustics":"Baseline plus all raw rhythm, texture, and development measurements.",
            "combined_acoustics_bands":"The same acoustic datapoints represented as fixed numeric intervals.",
            "affect_bands":"The same affect datapoints represented as fixed numeric intervals.",
            "combined_all":"Baseline plus every raw feature family.",
            "combined_all_bands":"The same complete feature set represented as fixed numeric intervals.",
            "combined_all_no_mood_head":"All four raw feature families and the baseline instrument/style observations, with only the mood-theme classifier observation removed and its absence declared unknown.",
            "features_only":"All four raw feature families with every baseline learned classifier observation removed and its absence declared unknown."
        },
        "tags":first.tag_catalog,
        "request_mappings":mappings,
        "comparisons":comparisons,
        "request_count":comparisons.len(),
        "max_input_units":units,
        "hard_limits":{"max_requests":MAX_REQUESTS,"max_input_units":MAX_INPUT_UNITS,
            "max_request_body_bytes":MAX_BODY_BYTES,"questions_per_request":QUESTION_COUNT,
            "selected_tracks":MAX_SELECTED_TRACKS},
        "band_semantics":"Bands are locally reconstructed fixed numeric intervals. They preserve every metric and unit without assigning semantic labels; they are not calibrated categories.",
        "question_semantics":"Every arm uses the exact same 138 concise Noul questions. Answers are probabilities of yes to the stated fit question, not tag intensity.",
        "purpose":"A paired, non-certifying comparison of bounded numeric feature families, equivalent fixed-interval representations, and two controlled learned-observation removals. Arm order rotates deterministically by sorted track ID. Every track has twelve primary arms and one planned exact baseline repeat; one identity-free baseline null control is shared.",
        "disclosure":"Only frozen baseline observations and locally reconstructed feature cards enter provider state. Extraction provenance, hashes, paths, titles, artists, owner ratings and track identity are excluded. No retries, automatic resume, production writes, winner selection, adoption, certification or quality gate."
    });
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
            "feature experiment differs from the exact reviewed plan or fixed budget".into(),
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
    let features = pilot::read_json(features_path)?;
    let saved = pilot::read_json(saved_path)?;
    let tracks: Vec<u64> = serde_json::from_value(saved["selected_tracks"].clone())?;
    let plan = prepare(&baseline, &features, &tracks)?;
    authorize(&plan, &saved, expected, count, units)?;
    for recording in plan.document["recordings"].as_array().ok_or("recordings")? {
        let path = Path::new(recording["source_path"].as_str().ok_or("source path")?);
        if pilot::audio_hash(path)? != recording["file_sha256"] {
            return Err("original changed since baseline and feature extraction".into());
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
    let features = pilot::read_json(features_path)?;
    let saved = pilot::read_json(saved_path)?;
    let tracks: Vec<u64> = serde_json::from_value(saved["selected_tracks"].clone())?;
    let plan = prepare(&baseline, &features, &tracks)?;
    if plan.document != saved {
        return Err("feature report plan differs from exact offline reconstruction".into());
    }
    let mut bytes = Vec::new();
    File::open(journal_path)?
        .take(MAX_JOURNAL_BYTES + 1)
        .read_to_end(&mut bytes)?;
    if bytes.len() as u64 > MAX_JOURNAL_BYTES {
        return Err("feature journal exceeds report bound".into());
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
        return Err(
            "feature comparative export requires a complete journal; partial results remain unknown"
                .into(),
        );
    }
    let mappings = plan.document["request_mappings"]
        .as_array()
        .ok_or("request mappings")?;
    let mut requests = Vec::with_capacity(plan.comparisons.len());
    for index in 0..plan.comparisons.len() {
        let mut row = mappings
            .get(index)
            .ok_or("missing request mapping")?
            .clone();
        let object = row.as_object_mut().ok_or("request mapping object")?;
        object.insert(
            "raw_typed_answers".into(),
            serde_json::to_value(observed.answers.get(&index).ok_or("missing answers")?)?,
        );
        let usage = observed
            .usage_by_request
            .get(&index)
            .ok_or("missing request usage")?;
        object.insert(
            "usage".into(),
            json!({"input_tokens":usage[0],"output_tokens":usage[1]}),
        );
        requests.push(row);
    }
    Ok(json!({
        "schema_version":"jev-acoustic-feature-experiment-report/v1",
        "model":MODEL,
        "plan_sha256":fingerprint(&plan.document)?,
        "feature_input_sha256":plan.document["feature_input_sha256"],
        "feature_provenance":plan.document["feature_provenance"],
        "complete":true,
        "planned_requests":plan.comparisons.len(),
        "responses":observed.answers.len(),
        "usage":{
            "reported_input_tokens":observed.tokens[0],
            "reported_output_tokens":observed.tokens[1],
            "responses_reporting_input":observed.token_reports[0],
            "responses_reporting_output":observed.token_reports[1],
            "missing_input_reports":plan.comparisons.len()-observed.token_reports[0],
            "missing_output_reports":plan.comparisons.len()-observed.token_reports[1]
        },
        "selected_tracks":plan.document["selected_tracks"],
        "selected_recordings":plan.document["recordings"],
        "vocabulary":plan.document["vocabulary"],
        "tags":plan.document["tags"],
        "arms":plan.document["arm_definitions"],
        "band_semantics":plan.document["band_semantics"],
        "question_semantics":plan.document["question_semantics"],
        "requests":requests,
        "interpretation":"Complete raw comparative export only. Fixed intervals are alternate representations of the same measured datapoints, not calibrated semantic labels. Missing usage remains unknown rather than zero. Planned repeats and the null control are retained. No quality gate, winner selection, listening-accuracy claim, production adoption, certification or library write follows."
    }))
}

#[cfg(test)]
#[path = "feature_tests.rs"]
mod tests;
