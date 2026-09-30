//! Follow-up presentation ablation derived from the immutable feature experiment.
use super::*;
use std::{collections::BTreeSet, fs::File, io::Read, path::Path};

#[path = "feature_format_cards.rs"]
mod feature_format_cards;
use feature_format_cards::{CardStats, Presentation};

const SCHEMA: &str = "jev-acoustic-feature-format-experiment/v1";
const BASELINE: &str = "baseline";
const DETAILED_ALL: &str = "detailed_all";
const COMPACT_ALL: &str = "compact_all";
const COMPACT_ACOUSTICS: &str = "compact_acoustics";
const DEFINITIONS_ONLY: &str = "definitions_only";
const ARMS: [&str; 5] = [
    BASELINE,
    DETAILED_ALL,
    COMPACT_ALL,
    COMPACT_ACOUSTICS,
    DEFINITIONS_ONLY,
];
const MAX_REQUESTS: usize = 100;
const MAX_INPUT_UNITS: u64 = 3_500_000;

pub(crate) struct Plan {
    pub document: Value,
    pub comparisons: Vec<comparison::Comparison>,
}

fn source_index(
    source: &super::Plan,
    track_id: Option<u64>,
    arm: &str,
    repeat: bool,
) -> Result<usize> {
    source.document["request_mappings"]
        .as_array()
        .ok_or("source request mappings")?
        .iter()
        .position(|mapping| {
            mapping["track_id"].as_u64() == track_id
                && mapping["arm"] == arm
                && mapping["repeat_control"] == repeat
                && mapping["control"]
                    == if track_id.is_none() {
                        "null_evidence"
                    } else {
                        "none"
                    }
        })
        .ok_or_else(|| "source feature request is missing".into())
}

fn push_request(
    comparisons: &mut Vec<comparison::Comparison>,
    mappings: &mut Vec<Value>,
    source: &super::Plan,
    track_id: Option<u64>,
    arm: &'static str,
    repeat_control: bool,
) -> Result<()> {
    let baseline_index = source_index(source, track_id, BASELINE, false)?;
    let baseline = &source.comparisons[baseline_index];
    let detailed_index = if track_id.is_some() {
        Some(source_index(source, track_id, "combined_all", false)?)
    } else {
        None
    };
    let (request, stats, presentation) = if let Some(index) = detailed_index {
        feature_format_cards::request_for(
            arm,
            &baseline.request,
            &source.comparisons[index].request,
        )?
    } else {
        if arm != BASELINE {
            return Err("null control is baseline only".into());
        }
        (
            source.comparisons[baseline_index].request.clone(),
            CardStats {
                point_slots: 0,
                unavailable_points: 0,
            },
            Presentation {
                metric_definitions: false,
                units: false,
                card_interpretations: false,
                card_type_labels: false,
            },
        )
    };
    let body_bytes = serde_json::to_vec(
        &json!({"model":MODEL,"state":request.state,"questions":request.questions}),
    )?
    .len();
    if body_bytes > MAX_BODY_BYTES {
        return Err("feature-format request exceeds the fixed typed-body bound".into());
    }
    let index = comparisons.len();
    let values_disclosed = if arm == DEFINITIONS_ONLY {
        0
    } else {
        stats.point_slots - stats.unavailable_points
    };
    mappings.push(json!({
        "request_index":index,
        "track_id":track_id,
        "arm":arm,
        "repeat_control":repeat_control,
        "control":if track_id.is_none() {"null_evidence"} else {"none"},
        "source_baseline_request_index":baseline_index,
        "source_detailed_request_index":detailed_index,
        "feature_families":match arm {
            BASELINE=>json!([]),
            COMPACT_ACOUSTICS=>json!(["rhythm","texture","development"]),
            _=>json!(["rhythm","texture","development","affect"]),
        },
        "feature_point_slots":stats.point_slots,
        "unavailable_points":stats.unavailable_points,
        "numeric_values_disclosed":values_disclosed,
        "metric_definitions_included":presentation.metric_definitions,
        "units_included":presentation.units,
        "card_interpretations_included":presentation.card_interpretations,
        "card_type_labels_included":presentation.card_type_labels,
        "baseline_observations_changed":false,
        "request_state_sha256":fingerprint(&request.state)?,
        "request_questions_sha256":fingerprint(&serde_json::to_value(&request.questions)?)?,
        "question_count":request.questions.len(),
        "body_bytes":body_bytes,
    }));
    comparisons.push(comparison::Comparison {
        case_id: baseline.case_id.clone(),
        partition: index,
        variant: arm,
        fit_threshold: baseline.fit_threshold,
        tags: baseline.tags.clone(),
        request,
    });
    Ok(())
}

pub(crate) fn prepare(baseline: &Value, features: &Value, tracks: &[u64]) -> Result<Plan> {
    let source = super::prepare(baseline, features, tracks)?;
    let selected = tracks.iter().copied().collect::<BTreeSet<_>>();
    let mut comparisons = Vec::new();
    let mut mappings = Vec::new();
    for (track_index, track_id) in selected.iter().enumerate() {
        for offset in 0..ARMS.len() {
            let arm = ARMS[(track_index + offset) % ARMS.len()];
            push_request(
                &mut comparisons,
                &mut mappings,
                &source,
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
            &source,
            Some(*track_id),
            BASELINE,
            true,
        )?;
    }
    push_request(
        &mut comparisons,
        &mut mappings,
        &source,
        None,
        BASELINE,
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
        return Err("feature-format experiment exceeds fixed coverage or budget bounds".into());
    }
    let document = json!({
        "schema_version":SCHEMA,
        "model":MODEL,
        "endpoint":ENDPOINT,
        "source_feature_plan_sha256":fingerprint(&source.document)?,
        "baseline_plan_sha256":source.document["baseline_plan_sha256"],
        "feature_input_sha256":source.document["feature_input_sha256"],
        "feature_provenance":source.document["feature_provenance"],
        "selected_tracks":source.document["selected_tracks"],
        "recordings":source.document["recordings"],
        "vocabulary":source.document["vocabulary"],
        "tags":source.document["tags"],
        "arms":ARMS,
        "arm_definitions":{
            "baseline":"The exact frozen short-Noul baseline request from the completed feature experiment.",
            "detailed_all":"The exact completed-experiment combined_all request, including all raw values, units, metric definitions, card type fields and card interpretations.",
            "compact_all":"The same four feature families, raw values or explicit unavailability, units, metric IDs and coverage; metric definitions and card type/interpretation fields are removed.",
            "compact_acoustics":"The compact raw rhythm, texture and development cards only; affect is withheld rather than replaced.",
            "definitions_only":"The exact detailed card families, units, metric definitions, type fields and interpretations with every numeric scalar/array withheld and every metric explicitly unavailable. Unavailability is unknown input, not negative evidence."
        },
        "request_mappings":mappings,
        "comparisons":comparisons,
        "request_count":comparisons.len(),
        "max_input_units":units,
        "hard_limits":{"max_requests":MAX_REQUESTS,"max_input_units":MAX_INPUT_UNITS,
            "max_request_body_bytes":MAX_BODY_BYTES,"questions_per_request":QUESTION_COUNT},
        "question_semantics":"All five arms use the exact same 138 frozen short-Noul questions. Answers are probabilities of yes to the stated fit question, not tag intensity.",
        "purpose":"A paired, non-certifying follow-up that isolates feature-card presentation and affect inclusion while preserving baseline learned observations and questions. Five primary arms rotate by sorted track ID; every track has one planned exact baseline repeat and one identity-free baseline null control is shared.",
        "disclosure":"Only the same frozen baseline observations and locally reconstructed feature data from the completed experiment enter provider state. Compact arms remove explanations, not values or units. Definitions-only withholds values explicitly as unavailable. No retries, automatic resume, production writes, winner selection, adoption, certification or accuracy claim."
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
            "feature-format experiment differs from the exact reviewed plan or fixed budget".into(),
        );
    }
    Ok(())
}

pub(crate) fn load_authorized(
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
            return Err("original changed since the source feature experiment".into());
        }
    }
    Ok(plan)
}

pub(crate) fn report(
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
        return Err("feature-format report plan differs from exact offline reconstruction".into());
    }
    let mut bytes = Vec::new();
    File::open(journal_path)?
        .take(MAX_JOURNAL_BYTES + 1)
        .read_to_end(&mut bytes)?;
    if bytes.len() as u64 > MAX_JOURNAL_BYTES {
        return Err("feature-format journal exceeds report bound".into());
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
            "feature-format export requires a complete journal; partial results remain unknown"
                .into(),
        );
    }
    let mappings = plan.document["request_mappings"]
        .as_array()
        .ok_or("request mappings")?;
    let mut requests = Vec::with_capacity(plan.comparisons.len());
    for index in 0..plan.comparisons.len() {
        let mut row = mappings.get(index).ok_or("request mapping")?.clone();
        let object = row.as_object_mut().ok_or("request mapping object")?;
        object.insert(
            "raw_typed_answers".into(),
            serde_json::to_value(observed.answers.get(&index).ok_or("missing answers")?)?,
        );
        let usage = observed
            .usage_by_request
            .get(&index)
            .ok_or("missing usage")?;
        object.insert(
            "usage".into(),
            json!({"input_tokens":usage[0],"output_tokens":usage[1]}),
        );
        requests.push(row);
    }
    Ok(json!({
        "schema_version":"jev-acoustic-feature-format-experiment-report/v1",
        "model":MODEL,
        "plan_sha256":fingerprint(&plan.document)?,
        "source_feature_plan_sha256":plan.document["source_feature_plan_sha256"],
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
        "question_semantics":plan.document["question_semantics"],
        "requests":requests,
        "interpretation":"Complete raw comparative export only. Presentation sensitivity does not establish musical accuracy or a preferred production input. Definitions-only unavailability is unknown rather than negative evidence. Missing usage remains unknown rather than zero. No quality gate, winner selection, production adoption, certification or library write follows."
    }))
}

#[cfg(test)]
#[path = "feature_format_tests.rs"]
mod tests;
