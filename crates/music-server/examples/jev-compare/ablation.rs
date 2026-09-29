//! Frozen, bounded evidence ablation over one 138-question request per song and arm.
use super::*;
use music_application::assistant::TypedQuestion;
use std::collections::{BTreeMap, BTreeSet};

#[path = "ablation_auxiliary.rs"]
mod auxiliary;

const SCHEMA: &str = "jev-evidence-ablation/v1";
const BASELINE_ARM: &str = "baseline_batch";
const ARMS: [&str; 8] = [
    BASELINE_ARM,
    "no_mood_head",
    "rank_only",
    "compact_score",
    "specific_context",
    "plus_danceability",
    "plus_contrasts",
    "section_styles",
];
const MAX_REQUESTS: usize = 146;
const MAX_INPUT_UNITS: u64 = 8_000_000;
const MAX_BODY_BYTES: usize = 63_000;
const QUESTION_COUNT: usize = 138;
const NULL_STATE: &str = "No observations of this recording are available.";

pub(super) struct Plan {
    pub document: Value,
    pub comparisons: Vec<comparison::Comparison>,
}

struct BaselineMaterial {
    state: Value,
    questions: BTreeMap<String, TypedQuestion>,
    tags: BTreeMap<String, comparison::FitTarget>,
    tag_catalog: BTreeMap<String, Value>,
    evidence_sha256: String,
    simple_questions_sha256: String,
    graded_questions_sha256: String,
}

#[derive(Clone, Copy)]
struct RequestSpec {
    track_id: Option<u64>,
    arm: &'static str,
    repeat_control: bool,
    null_control: bool,
}

fn hash_serializable<T: serde::Serialize>(value: &T) -> Result<String> {
    fingerprint(&serde_json::to_value(value)?)
}

fn baseline_material(
    baseline: &Value,
    simple: &simple::Plan,
    track_id: u64,
) -> Result<BaselineMaterial> {
    let mut state = None;
    let mut questions = BTreeMap::new();
    let mut tags = BTreeMap::new();
    let mut partitions = 0;
    for comparison in simple
        .comparisons
        .iter()
        .filter(|comparison| comparison.case_id == format!("track-{track_id}"))
    {
        partitions += 1;
        if comparison.variant != "simple_noul" {
            return Err("unexpected simple baseline arm".into());
        }
        if let Some(previous) = &state {
            if previous != &comparison.request.state {
                return Err("labels8 partitions do not share exact evidence".into());
            }
        } else {
            state = Some(comparison.request.state.clone());
        }
        for (id, question) in &comparison.request.questions {
            if questions.insert(id.clone(), question.clone()).is_some() {
                return Err("duplicate baseline question".into());
            }
        }
        for (id, tag) in &comparison.tags {
            if tags.insert(id.clone(), tag.clone()).is_some() {
                return Err("duplicate baseline tag".into());
            }
        }
    }
    if partitions != 7 || questions.len() != QUESTION_COUNT || tags.len() != QUESTION_COUNT {
        return Err("baseline must contain seven partitions and 138 unique questions".into());
    }
    let state = state.ok_or("missing baseline evidence")?;
    let mut graded_questions = BTreeMap::<String, Value>::new();
    let mut graded_partitions = 0;
    for case in baseline["cases"].as_array().ok_or("baseline cases")? {
        if case["track_id"].as_u64() != Some(track_id)
            || case["arm"] != "labels8"
            || case["repeat_control"] != false
        {
            continue;
        }
        graded_partitions += 1;
        if case["request"]["state"] != state {
            return Err("simple evidence differs from frozen labels8 evidence".into());
        }
        for (id, question) in case["request"]["questions"]
            .as_object()
            .ok_or("graded questions")?
        {
            if graded_questions
                .insert(id.clone(), question.clone())
                .is_some()
            {
                return Err("duplicate graded question".into());
            }
        }
    }
    if graded_partitions != 7 || graded_questions.len() != QUESTION_COUNT {
        return Err("frozen labels8 question coverage is incomplete".into());
    }
    let mut tag_catalog = BTreeMap::new();
    for (id, question) in &questions {
        let TypedQuestion::Noul { instructions, .. } = question else {
            return Err("simple baseline question must be Noul".into());
        };
        let target = tags.get(id).ok_or("missing tag metadata")?;
        tag_catalog.insert(
            id.clone(),
            json!({"name":target.tag,"group":target.group,
                "definition":instructions["definition"]}),
        );
    }
    Ok(BaselineMaterial {
        evidence_sha256: fingerprint(&state)?,
        simple_questions_sha256: hash_serializable(&questions)?,
        graded_questions_sha256: hash_serializable(&graded_questions)?,
        state,
        questions,
        tags,
        tag_catalog,
    })
}

fn specific_question(group: &str, name: &str) -> Result<String> {
    Ok(match group {
        "scene" => format!(
            "Is this music a strong, characteristic accompaniment match for {name}, based on its pacing and atmosphere?"
        ),
        "setting" => format!(
            "Does this music strongly evoke an atmosphere characteristic of {name}, rather than merely being generically compatible?"
        ),
        _ => return Err("specific-context wording is limited to scene and setting".into()),
    })
}

fn compact_levels(group: &str, name: &str) -> Result<Vec<Value>> {
    Ok(match group {
        "mood" => vec![
            json!(format!("{name} mood: absent or conflicting.")),
            json!(format!("{name} mood: recognizable but secondary.")),
            json!(format!("{name} mood: strong or central.")),
        ],
        "scene" => vec![
            json!(format!(
                "{name} scene accompaniment: conflicting or no useful support."
            )),
            json!(format!(
                "{name} scene accompaniment: useful with some adjustment."
            )),
            json!(format!(
                "{name} scene accompaniment: strong characteristic match."
            )),
        ],
        "setting" => vec![
            json!(format!(
                "{name} setting atmosphere: conflicting or no distinctive association."
            )),
            json!(format!(
                "{name} setting atmosphere: recognizable with scene context."
            )),
            json!(format!(
                "{name} setting atmosphere: strongly characteristic."
            )),
        ],
        "period" => vec![
            json!(format!(
                "{name} period flavor: conflicting or unrecognizable."
            )),
            json!(format!("{name} period flavor: recognizable but partial.")),
            json!(format!(
                "{name} period flavor: strongly or centrally evoked."
            )),
        ],
        _ => return Err("unsupported compact-score group".into()),
    })
}

fn questions_for(
    arm: &str,
    baseline: &BaselineMaterial,
) -> Result<BTreeMap<String, TypedQuestion>> {
    if !ARMS.contains(&arm) {
        return Err("unknown ablation arm".into());
    }
    let mut questions = baseline.questions.clone();
    if arm == "specific_context" {
        for (id, question) in &mut questions {
            let target = baseline.tags.get(id).ok_or("tag metadata")?;
            if !["scene", "setting"].contains(&target.group.as_str()) {
                continue;
            }
            let TypedQuestion::Noul { instructions, .. } = question else {
                return Err("specific-context baseline must be Noul".into());
            };
            instructions["question"] = json!(specific_question(&target.group, &target.tag)?);
        }
    } else if arm == "compact_score" {
        for (id, question) in &mut questions {
            let target = baseline.tags.get(id).ok_or("tag metadata")?;
            let TypedQuestion::Noul { instructions, .. } = question else {
                return Err("compact-score baseline must be Noul".into());
            };
            let definition = instructions["definition"].clone();
            let task = match target.group.as_str() {
                "mood" => format!("Rate {} as musical mood.", target.tag),
                "scene" => format!("Rate {} as scene accompaniment.", target.tag),
                "setting" => format!("Rate {} as evoked setting.", target.tag),
                "period" => format!("Rate {} as musical period flavor.", target.tag),
                _ => return Err("unsupported compact-score group".into()),
            };
            *question = TypedQuestion::Score {
                instructions: json!({"question":task,"definition":definition}),
                criteria: compact_levels(&target.group, &target.tag)?,
            };
        }
    }
    Ok(questions)
}

fn state_for(arm: &str, baseline: &Value, auxiliary: &Value) -> Result<Value> {
    let mut state = baseline.clone();
    match arm {
        BASELINE_ARM | "compact_score" | "specific_context" => {}
        "no_mood_head" => {
            let observations = state["observations"]
                .as_array_mut()
                .ok_or("baseline observations")?;
            let before = observations.len();
            observations.retain(|observation| observation["classifier"] != "mood_theme");
            if observations.len() + 1 != before {
                return Err("baseline must contain exactly one mood_theme observation".into());
            }
        }
        "rank_only" => {
            for observation in state["observations"]
                .as_array_mut()
                .ok_or("baseline observations")?
            {
                for label in observation["labels"]
                    .as_array_mut()
                    .ok_or("baseline labels")?
                {
                    if label
                        .as_object_mut()
                        .ok_or("baseline label")?
                        .remove("mean_response")
                        .and_then(|value| value.as_f64())
                        .is_none()
                    {
                        return Err("rank-only arm requires numeric mean responses".into());
                    }
                }
            }
            state["interpretation"] = json!(
                "Labels are shown only in ordinal response rank within each classifier. Ranks are relative positions, not probabilities, response magnitudes, truth, or absence of omitted classes. The classifiers share one audio encoder and are correlated. No title, artist, location, story, lyrics transcription, owner ratings, or verified period evidence is provided."
            );
        }
        "plus_danceability" => {
            state["auxiliary_observations"] = json!([{
                "kind":"danceability_model_response_summary",
                "mean_response":auxiliary["danceability"]["mean_response"],
                "low_response":auxiliary["danceability"]["low_response"],
                "high_response":auxiliary["danceability"]["high_response"],
                "coverage_seconds":auxiliary["danceability"]["coverage_seconds"],
                "interpretation":"Responses are uncalibrated danceability-model outputs over the recording. Low and high are the observed response range, not probabilities, truth labels, or confidence bounds."
            }]);
        }
        "plus_contrasts" => {
            state["auxiliary_observations"] = json!([{
                "kind":"independent_relative_audio_comparisons",
                "comparisons":auxiliary["contrasts"],
                "interpretation":"All three paired text paraphrases preferred the stated side in one independent audio/text model. These are correlated prompt variants, not three models. Margin is an uncalibrated cosine-similarity separation. A preference is not a probability, does not prove the preferred quality is present, and does not prove the other quality is absent. Ambiguous comparisons are omitted."
            }]);
        }
        "section_styles" => {
            if !auxiliary["section_styles"]
                .as_array()
                .is_some_and(Vec::is_empty)
            {
                state["auxiliary_observations"] = json!([{
                    "kind":"equal_time_bin_style_responses",
                    "labels":auxiliary["section_styles"],
                    "interpretation":"These style-model labels were outside the whole-recording top eight but ranked in the top three within the reported number of ten equal-time bins. The bins are not detected musical sections. Mean responses are uncalibrated model similarities, not probabilities, truth, or style duration."
                }]);
            }
        }
        _ => return Err("unknown ablation arm".into()),
    }
    Ok(state)
}

fn request_for(
    arm: &'static str,
    baseline: &BaselineMaterial,
    auxiliary: &Value,
) -> Result<TypedDecisionRequest> {
    let request = TypedDecisionRequest {
        state: state_for(arm, &baseline.state, auxiliary)?,
        questions: questions_for(arm, baseline)?,
    };
    if request.questions.len() != QUESTION_COUNT {
        return Err("every ablation request must contain all 138 questions".into());
    }
    request.validate()?;
    let body_bytes = serde_json::to_vec(
        &json!({"model":MODEL,"state":request.state,"questions":request.questions}),
    )?
    .len();
    if body_bytes > MAX_BODY_BYTES {
        return Err("138-question request exceeds the fixed 63 KB body bound".into());
    }
    Ok(request)
}

fn push_request(
    comparisons: &mut Vec<comparison::Comparison>,
    mappings: &mut Vec<Value>,
    material: &BaselineMaterial,
    auxiliary: &Value,
    spec: RequestSpec,
) -> Result<()> {
    let request = if spec.null_control {
        let request = TypedDecisionRequest {
            state: json!(NULL_STATE),
            questions: questions_for(spec.arm, material)?,
        };
        request.validate()?;
        request
    } else {
        request_for(spec.arm, material, auxiliary)?
    };
    if request.questions.len() != QUESTION_COUNT {
        return Err("every ablation request must contain all 138 questions".into());
    }
    let body_bytes = serde_json::to_vec(
        &json!({"model":MODEL,"state":request.state,"questions":request.questions}),
    )?
    .len();
    if body_bytes > MAX_BODY_BYTES {
        return Err("138-question request exceeds the fixed 63 KB body bound".into());
    }
    let index = comparisons.len();
    let evidence_changed = request.state != material.state;
    let no_op = spec.arm != BASELINE_ARM
        && request.state == material.state
        && request.questions == material.questions;
    mappings.push(json!({
        "request_index":index,
        "track_id":spec.track_id,
        "arm":spec.arm,
        "repeat_control":spec.repeat_control,
        "control":if spec.null_control {"null_evidence"} else {"none"},
        "evidence_changed":evidence_changed,
        "no_op":no_op,
        "baseline_evidence_sha256":material.evidence_sha256,
        "request_evidence_sha256":fingerprint(&request.state)?,
        "frozen_graded_questions_sha256":material.graded_questions_sha256,
        "baseline_questions_sha256":material.simple_questions_sha256,
        "request_questions_sha256":hash_serializable(&request.questions)?,
        "question_count":request.questions.len(),
        "body_bytes":body_bytes,
    }));
    comparisons.push(comparison::Comparison {
        case_id: spec.track_id.map_or_else(
            || "null-evidence-control".to_owned(),
            |id| format!("track-{id}"),
        ),
        partition: index,
        variant: spec.arm,
        fit_threshold: 0.5,
        tags: material.tags.clone(),
        request,
    });
    Ok(())
}

pub(super) fn prepare(baseline: &Value, auxiliary: &Value, tracks: &[u64]) -> Result<Plan> {
    let simple = simple::prepare(baseline, tracks)?;
    let selected = tracks.iter().copied().collect::<BTreeSet<_>>();
    if tracks.is_empty() || tracks.len() > 16 || selected.len() != tracks.len() {
        return Err("select 1..16 distinct baseline track ids".into());
    }
    let auxiliaries = auxiliary::validate(baseline, auxiliary, &selected)?;
    let mut materials = BTreeMap::new();
    for track_id in &selected {
        materials.insert(*track_id, baseline_material(baseline, &simple, *track_id)?);
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
        let auxiliary = auxiliaries.get(track_id).ok_or("track auxiliary")?;
        for offset in 0..ARMS.len() {
            let arm = ARMS[(track_index + offset) % ARMS.len()];
            push_request(
                &mut comparisons,
                &mut mappings,
                material,
                auxiliary,
                RequestSpec {
                    track_id: Some(*track_id),
                    arm,
                    repeat_control: false,
                    null_control: false,
                },
            )?;
        }
    }
    // These are planned identical-input repeats, never retries after an outcome.
    for track_id in &selected {
        push_request(
            &mut comparisons,
            &mut mappings,
            materials.get(track_id).ok_or("track material")?,
            auxiliaries.get(track_id).ok_or("track auxiliary")?,
            RequestSpec {
                track_id: Some(*track_id),
                arm: BASELINE_ARM,
                repeat_control: true,
                null_control: false,
            },
        )?;
    }
    // Both controls share the same exact absence statement and complete vocabulary.
    let null_auxiliary = auxiliaries.values().next().ok_or("auxiliary")?;
    push_request(
        &mut comparisons,
        &mut mappings,
        first,
        null_auxiliary,
        RequestSpec {
            track_id: None,
            arm: BASELINE_ARM,
            repeat_control: false,
            null_control: true,
        },
    )?;
    push_request(
        &mut comparisons,
        &mut mappings,
        first,
        null_auxiliary,
        RequestSpec {
            track_id: None,
            arm: "specific_context",
            repeat_control: false,
            null_control: true,
        },
    )?;

    let units = comparisons
        .iter()
        .map(|comparison| model_request_reservation(&comparison.request.accounting_request(), 0))
        .sum::<u64>();
    let expected_requests = selected.len() * 9 + 2;
    if comparisons.len() != expected_requests
        || comparisons.len() > MAX_REQUESTS
        || units > MAX_INPUT_UNITS
    {
        return Err("ablation exceeds fixed coverage or budget bounds".into());
    }
    let document = json!({
        "schema_version":SCHEMA,
        "model":MODEL,
        "endpoint":ENDPOINT,
        "baseline_plan_sha256":fingerprint(baseline)?,
        "auxiliary_sha256":fingerprint(auxiliary)?,
        "selected_tracks":selected,
        "recordings":simple.document["recordings"],
        "vocabulary":simple.document["vocabulary"],
        "arms":ARMS,
        "arm_definitions":{
            "baseline_batch":"The exact concise Noul questions from the short-question pilot, merged across all seven frozen labels8 partitions.",
            "no_mood_head":"Baseline questions and state with only the mood_theme classifier observation removed.",
            "rank_only":"Baseline questions with label names and ordinal ranks retained, numeric mean responses removed, and rank interpretation made explicit.",
            "compact_score":"The same tag definitions in three self-contained ordered levels. Preserve raw Score answers; normalized fit is raw expected position divided by 2.",
            "specific_context":"Baseline evidence and exact mood/period questions; only scene and setting wording asks for a strong characteristic match.",
            "plus_danceability":"Baseline plus one reconstructed danceability response-summary card.",
            "plus_contrasts":"Baseline plus qualified independent relative comparison preferences; preferences do not assert absence or probability.",
            "section_styles":"Baseline plus up to four style labels salient in ten equal-time bins and absent from the whole-recording top eight."
        },
        "tags":first.tag_catalog,
        "request_mappings":mappings,
        "comparisons":comparisons,
        "request_count":comparisons.len(),
        "max_input_units":units,
        "hard_limits":{"max_requests":MAX_REQUESTS,"max_input_units":MAX_INPUT_UNITS,
            "max_request_body_bytes":MAX_BODY_BYTES,"questions_per_request":QUESTION_COUNT},
        "score_semantics":"Noul is probability of answering yes, not tag intensity. Compact Score is ordinal; divide raw expected position by two only when reporting normalized fit. Preserve every raw answer and do not impose a quality gate.",
        "question_semantics":{"baseline_and_context_noul":"Probability of answering yes to the stated fit question; not musical intensity.",
            "compact_score":"Three-level ordinal expected position normalized as raw_score / 2.",
            "frozen_graded_reference":"Historical five-level ordinal expected position was normalized as raw_score / 4; it is not numerically interchangeable with the new compact Score."},
        "purpose":"A paired, non-certifying evidence and question-form ablation. Arm order rotates deterministically by sorted track ID. Every selected track has eight primary arms and one exact baseline repeat; two identity-free null-evidence controls are common to the 138-tag vocabulary.",
        "disclosure":"Only reconstructed, bounded audio-model observations are sent. Auxiliary provenance, hashes, paths, titles, artist, story, owner ratings and track identity are excluded from request state. No retries, production writes, adoption, certification or automatic quality gate."
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
        return Err("ablation differs from the exact reviewed plan or fixed budget".into());
    }
    Ok(())
}

pub(super) fn load_authorized(
    baseline_path: &Path,
    auxiliary_path: &Path,
    saved_path: &Path,
    expected: &str,
    count: usize,
    units: u64,
) -> Result<Plan> {
    let baseline = pilot::read_json(baseline_path)?;
    let auxiliary = pilot::read_json(auxiliary_path)?;
    let saved = pilot::read_json(saved_path)?;
    let tracks: Vec<u64> = serde_json::from_value(saved["selected_tracks"].clone())?;
    let plan = prepare(&baseline, &auxiliary, &tracks)?;
    authorize(&plan, &saved, expected, count, units)?;
    for recording in plan.document["recordings"].as_array().ok_or("recordings")? {
        let path = Path::new(recording["source_path"].as_str().ok_or("source path")?);
        if pilot::audio_hash(path)? != recording["file_sha256"] {
            return Err("original changed since baseline analysis".into());
        }
    }
    Ok(plan)
}

pub(super) fn report(
    baseline_path: &Path,
    auxiliary_path: &Path,
    saved_path: &Path,
    journal_path: &Path,
) -> Result<Value> {
    const MAX_JOURNAL_BYTES: u64 = 256 * 1024 * 1024;
    let baseline = pilot::read_json(baseline_path)?;
    let auxiliary = pilot::read_json(auxiliary_path)?;
    let saved = pilot::read_json(saved_path)?;
    let tracks: Vec<u64> = serde_json::from_value(saved["selected_tracks"].clone())?;
    let plan = prepare(&baseline, &auxiliary, &tracks)?;
    if plan.document != saved {
        return Err("ablation report plan differs from exact offline reconstruction".into());
    }
    let mut bytes = Vec::new();
    File::open(journal_path)?
        .take(MAX_JOURNAL_BYTES + 1)
        .read_to_end(&mut bytes)?;
    if bytes.len() as u64 > MAX_JOURNAL_BYTES {
        return Err("ablation journal exceeds report bound".into());
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
            "ablation comparative export requires a complete journal; partial results remain unknown"
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
        "schema_version":"jev-evidence-ablation-report/v1",
        "model":MODEL,
        "plan_sha256":fingerprint(&plan.document)?,
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
        "interpretation":"Complete raw comparative export only. Noul probabilities and compact ordinal Scores have different meanings. Missing provider usage remains unknown rather than zero. Repeats and no-op sensitivity controls are retained. No quality gate, winner selection, listening-accuracy claim, production adoption, certification or library write follows from this report."
    }))
}

#[cfg(test)]
#[path = "ablation_tests.rs"]
mod tests;
