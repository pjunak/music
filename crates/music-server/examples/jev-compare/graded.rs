//! Frozen, non-certifying ordinal tag-fit experiment. No production profile writes.
use super::*;
use music_application::assistant::{
    LOCAL_CONTEXT_ANALYZER_ID, LOCAL_CONTEXT_IMPLEMENTATION_ID, TypedAnswer, TypedQuestion,
    default_vocabulary_snapshot, typed_answers,
};
use std::collections::{BTreeMap, BTreeSet};

#[path = "graded_run.rs"]
pub(super) mod execution;
#[cfg(test)]
#[path = "graded_tests.rs"]
mod tests;

const SCHEMA: &str = "jev-graded-pilot/v1";
const RUBRIC: &str = "musical-fit-ordinal/v1";
const ARMS: [&str; 4] = ["labels3", "labels8", "labels8_physical", "labels8_temporal"];
const PARTITION: usize = 25;
const PHYSICAL_FEATURES: [&str; 4] = ["brightness", "density", "rhythmic_drive", "spectral_flux"];

fn validate_physical(recording: &Value) -> Result<()> {
    let context = &recording["input"]["context_evidence"];
    let duration = recording["input"]["length_s"].as_f64().ok_or("duration")?;
    if context["analyzer_id"] != LOCAL_CONTEXT_ANALYZER_ID
        || context["completeness"] != "full"
        || context["coverage"]["scope"] != "whole_track"
        || context["coverage"]["decoded_seconds"]
            .as_f64()
            .is_none_or(|decoded| {
                !decoded.is_finite()
                    || decoded <= 0.0
                    || (decoded - duration).abs() > 0.1 + duration * 0.001
            })
    {
        return Err("current complete whole-track physical evidence required".into());
    }
    for feature in PHYSICAL_FEATURES {
        let trajectory = &context["trajectories"][feature];
        let numeric_fields = ["typical", "low", "high", "start", "end", "peak_at_fraction"];
        if trajectory
            .as_object()
            .is_none_or(|v| v.len() != numeric_fields.len() + 1)
            || numeric_fields.iter().any(|field| {
                trajectory[*field]
                    .as_f64()
                    .is_none_or(|v| !v.is_finite() || !(0.0..=1.0).contains(&v))
            })
            || !matches!(
                trajectory["shape"].as_str(),
                Some(
                    "steady"
                        | "volatile"
                        | "arch"
                        | "dip_then_recovery"
                        | "gradual_rise"
                        | "stepped_build"
                        | "gradual_fall"
                        | "stepped_release"
                        | "alternating"
                        | "rising"
                        | "falling"
                        | "mixed"
                )
            )
        {
            return Err("complete bounded physical trajectory required".into());
        }
        if trajectory["low"].as_f64() > trajectory["typical"].as_f64()
            || trajectory["typical"].as_f64() > trajectory["high"].as_f64()
        {
            return Err("physical trajectory percentiles are inconsistent".into());
        }
    }
    for feature in PHYSICAL_FEATURES.into_iter().chain(["structure"]) {
        if !matches!(
            context["measurement_reliability"][feature].as_str(),
            Some("low" | "medium" | "high")
        ) {
            return Err("physical measurement reliability required".into());
        }
    }
    let structure = &context["structure"];
    let sections = structure["section_count"]
        .as_u64()
        .ok_or("physical section count")?;
    if structure.as_object().is_none_or(|v| v.len() != 4)
        || sections == 0
        || structure["major_change_count"].as_u64() != Some(sections - 1)
        || structure["repeated_section_count"]
            .as_u64()
            .is_none_or(|v| v > sections)
        || !matches!(
            structure["development"].as_str(),
            Some("continuous" | "sectional" | "repetitive")
        )
    {
        return Err("complete physical structure required".into());
    }
    Ok(())
}

pub(super) fn prepare(sources: &Path) -> Result<Value> {
    let sources = pilot::read_json(sources)?;
    let sources = sources.as_array().ok_or("expected corpus source array")?;
    if sources.is_empty() || sources.len() > 4 {
        return Err("supply 1..4 explicit corpus sources".into());
    }
    let mut selected = Vec::new();
    for source in sources {
        let cohort = source["cohort"].as_str().ok_or("cohort")?;
        if !["development", "unrated"].contains(&cohort) {
            return Err("cohort must be development or unrated".into());
        }
        let mut raw = String::new();
        File::open(source["predictions"].as_str().ok_or("predictions path")?)?
            .take(32 * 1024 * 1024 + 1)
            .read_to_string(&mut raw)?;
        if raw.len() > 32 * 1024 * 1024 {
            return Err("prediction document exceeds bound".into());
        }
        let corpus = pilot::predictions::attach(
            pilot::read_json(Path::new(source["corpus"].as_str().ok_or("corpus path")?))?,
            &raw,
        )?;
        if corpus["schema_version"] != "jev-private-corpus/v1"
            || corpus["analyzer_id"] != LOCAL_CONTEXT_ANALYZER_ID
            || corpus["implementation_id"] != LOCAL_CONTEXT_IMPLEMENTATION_ID
        {
            return Err("current whole-track analysis required".into());
        }
        let tracks: Vec<i64> = serde_json::from_value(source["tracks"].clone())?;
        if tracks.is_empty() || tracks.iter().collect::<BTreeSet<_>>().len() != tracks.len() {
            return Err("select distinct tracks".into());
        }
        for track in tracks {
            let matches = corpus["recordings"]
                .as_array()
                .ok_or("recordings")?
                .iter()
                .filter(|r| r["input"]["track_id"] == track)
                .collect::<Vec<_>>();
            if matches.len() != 1 {
                return Err("selected recording missing or ambiguous".into());
            }
            let mut recording = matches[0].clone();
            recording["input"]["track_id"] = json!(selected.len() + 1);
            recording["cohort"] = json!(cohort);
            selected.push(recording);
        }
    }
    build(&selected)
}

fn levels(group: &str) -> Result<[&'static str; 5]> {
    Ok(match group {
        "mood" => [
            "The described musical character conflicts with this mood or gives no recognizable expression of it.",
            "This mood is a faint secondary color; a listener could notice it behind the main musical character.",
            "This mood is a recognizable part of the musical character alongside other emotions.",
            "This mood strongly characterizes the music and would be a useful prominent listening description.",
            "This mood defines the music's central emotional character and is an especially clear description of it.",
        ],
        "scene" => [
            "The described music would distract from or give no useful musical support to this scene activity.",
            "The music has a slight connection to this activity, useful only for an unusual or limited scene variant.",
            "The music could usefully accompany this activity, with some adjustment to the scene's tone or pacing.",
            "The music is a strong background choice for a normal scene involving this activity.",
            "The music is an especially characteristic soundtrack for this activity, matching its tone and pacing directly.",
        ],
        "setting" => [
            "The described musical atmosphere conflicts with this setting or provides no distinctive association with it.",
            "The music gives a faint association with this setting, requiring substantial scene context from the listener.",
            "The music gives a recognizable atmosphere suitable for this setting when the scene supplies its location.",
            "The music strongly evokes an atmosphere characteristic of this setting and supports it as background.",
            "The music's defining atmosphere is especially characteristic of this setting, making it a primary association.",
        ],
        "period" => [
            "The described instrumentation and musical style conflict with or provide no recognizable flavor of this period category.",
            "The music contains a faint stylistic trace associated with this period category.",
            "The music has a recognizable partial stylistic association with this period category alongside other influences.",
            "The instrumentation and musical style strongly evoke this period category.",
            "The instrumentation and musical style make this period category the defining stylistic character of the music.",
        ],
        _ => return Err("unsupported vocabulary group".into()),
    })
}

fn question(group: &str, tag: &Value) -> Result<TypedQuestion> {
    let task = match group {
        "mood" => {
            "Rate expression of this emotional musical character, not how certain the evidence is. Quietness alone does not establish calm; setting and instrument names do not establish emotions."
        }
        "scene" => {
            "Rate usefulness as musical accompaniment to this tabletop scene activity. The activity need not literally occur in the recording. Distinguish musical pacing and atmosphere from a factual story claim."
        }
        "setting" => {
            "Rate atmospheric suitability for this tabletop setting. This is an evoked association, not the real recording location or a claim that a specific place is audible. A generic compatible mood alone gives only a weak setting association."
        }
        "period" => {
            "Rate evoked musical style or period flavor, never release date, recording technology, nationality, or actual historical origin. Timeless requires era-neutral character; cross-era requires recognizable different-era influences, not uncertainty."
        }
        _ => return Err("unsupported vocabulary group".into()),
    };
    Ok(TypedQuestion::Score {
        instructions: json!({"task":task,"tag":{
            "name":tag["name"],"definition":tag["description"]},
            "evidence_rule":"Use only the supplied audio observations. Learned labels are fallible clues, not verified tags; compare their response values only within the same classifier. Rate each tag independently; compatible tags may coexist. Missing labels are not proof of absence. Do not infer a storyline or copy tag examples into evidence."}),
        criteria: levels(group)?.into_iter().map(|v| json!(v)).collect(),
    })
}

fn rounded(value: &Value) -> Value {
    value
        .as_f64()
        .map_or(Value::Null, |v| json!((v * 1000.0).round() / 1000.0))
}

fn evidence(recording: &Value, arm: &str) -> Result<Value> {
    if !ARMS.contains(&arm) {
        return Err("unknown evidence recipe".into());
    }
    let limit = if arm == "labels3" { 3 } else { 8 };
    let mut cards = Vec::new();
    for source in recording["learned"]["sources"]
        .as_array()
        .ok_or("sources")?
    {
        let mut labels = source["labels"]
            .as_array()
            .ok_or("labels")?
            .iter()
            .collect::<Vec<_>>();
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
            .map(|(rank, label)| {
                let mut card = json!({"label":label["label"],"rank":rank+1,
                "mean_response":rounded(&label["mean_score"])});
                if arm == "labels8_temporal" {
                    card["peak_response"] = rounded(&label["max_score"]);
                    card["opening_response"] = rounded(&label["opening_score"]);
                    card["ending_response"] = rounded(&label["ending_score"]);
                    card["fraction_of_time_in_head_top3"] = rounded(&label["top_rank_fraction"]);
                }
                card
            })
            .collect::<Vec<_>>();
        cards.push(json!({"classifier":source["kind"],"labels":labels}));
    }
    cards.sort_by(|a, b| a["classifier"].as_str().cmp(&b["classifier"].as_str()));
    let mut state = json!({"observations":cards,
        "coverage":"Whole recording, with time-weighted classifier means.",
        "interpretation":"Classifier response values are uncalibrated pattern similarities, not tag probabilities or emotional intensity. A shortlist omits other classes without judging them absent. Classifiers share one audio encoder, so they are correlated. No title, artist, location, story, lyrics transcription, owner ratings, or verified period evidence is provided."});
    if arm == "labels8_temporal" {
        state["temporal_interpretation"] = json!(
            "Opening and ending describe first and last 10% of time. Top-three time fraction measures classifier ranking persistence, not the fraction of time a mood is truly audible. Peak response may describe a brief passage."
        );
    }
    if arm == "labels8_physical" {
        let context = &recording["input"]["context_evidence"];
        let mut measurements = BTreeMap::new();
        for feature in PHYSICAL_FEATURES {
            measurements.insert(
                feature,
                json!({"trajectory":context["trajectories"][feature],
                "reliability":context["measurement_reliability"][feature]}),
            );
        }
        state["physical"] = json!({"measurements":measurements,"structure":context["structure"],
            "interpretation":"These are normalized descriptive acoustic indices, not calibrated emotion measurements. Spectral brightness is frequency balance, density is acoustic activity, rhythmic drive is envelope pulse activity, flux is spectral change. A low value alone does not establish calm. Unverified tempo and unclassified voice are omitted."});
    }
    Ok(state)
}

fn requests(recording: &Value, arm: &str, vocabulary: &Value) -> Result<Vec<Value>> {
    let state = evidence(recording, arm)?;
    let mut result = Vec::new();
    for group in vocabulary["groups"].as_array().ok_or("vocabulary groups")? {
        let key = group["key"].as_str().ok_or("group key")?;
        for partition in group["tags"].as_array().ok_or("tags")?.chunks(PARTITION) {
            let questions = partition
                .iter()
                .map(|tag| {
                    Ok((
                        tag["id"].as_str().ok_or("tag ID")?.to_owned(),
                        question(key, tag)?,
                    ))
                })
                .collect::<Result<BTreeMap<_, _>>>()?;
            let request = TypedDecisionRequest {
                state: state.clone(),
                questions,
            };
            request.validate()?;
            result.push(json!({"group":key,"request":request}));
        }
    }
    Ok(result)
}

fn probes() -> Result<Vec<Value>> {
    [false, true].into_iter().map(|present| {
        let description = if present { "The music is peaceful, relaxed and soothing throughout, with no tension or agitation." }
            else { "The music is fiercely agitated, abrasive and violently tense throughout, with no peaceful or relaxed passages." };
        let tag = json!({"name":"calm","description":"Relaxed, peaceful, or settled emotional atmosphere."});
        Ok(json!({"group":"probe","expected_extreme":if present {4} else {0},
            "request":{"state":{"synthetic_musical_description":description},
                "questions":{"calm":question("mood", &tag)?}}}))
    }).collect()
}

fn build(recordings: &[Value]) -> Result<Value> {
    if recordings.is_empty() || recordings.len() > 16 {
        return Err("select 1..16 recordings".into());
    }
    let vocabulary = serde_json::to_value(default_vocabulary_snapshot()?.document)?;
    let mut hashes = BTreeSet::new();
    let mut cases = probes()?;
    for (index, recording) in recordings.iter().enumerate() {
        pilot::predictions::validate_recording(recording)?;
        validate_physical(recording)?;
        if recording["input"]["track_id"].as_u64() != Some(index as u64 + 1)
            || !["development", "unrated"].contains(&recording["cohort"].as_str().unwrap_or(""))
            || !hashes.insert(recording["file_sha256"].as_str().ok_or("hash")?)
        {
            return Err("invalid recording membership".into());
        }
        for offset in 0..ARMS.len() {
            let arm = ARMS[(index + offset) % ARMS.len()];
            for mut case in requests(recording, arm, &vocabulary)? {
                case["track_id"] = json!(index + 1);
                case["arm"] = json!(arm);
                case["repeat_control"] = json!(false);
                cases.push(case);
            }
        }
    }
    // Two preselected endpoints repeat the exact smallest evidence view; never a failure retry.
    for index in BTreeSet::from([0, recordings.len() - 1]) {
        for mut case in requests(&recordings[index], "labels3", &vocabulary)? {
            case["track_id"] = json!(index + 1);
            case["arm"] = json!("labels3");
            case["repeat_control"] = json!(true);
            cases.push(case);
        }
    }
    let units = cases
        .iter()
        .map(|case| request(case).map(|r| model_request_reservation(&r.accounting_request(), 0)))
        .collect::<Result<Vec<_>>>()?
        .iter()
        .sum::<u64>();
    if cases.len() > 600 || units > 40_000_000 {
        return Err("graded pilot exceeds hard limits".into());
    }
    Ok(
        json!({"schema_version":SCHEMA,"rubric":RUBRIC,"model":MODEL,"endpoint":ENDPOINT,
        "analyzer_id":LOCAL_CONTEXT_ANALYZER_ID,"implementation_id":LOCAL_CONTEXT_IMPLEMENTATION_ID,
        "vocabulary":vocabulary,"recordings":recordings,"arms":ARMS,
        "score_semantics":"Expected position among five described musical-fit levels divided by four. Ordinal-derived fit, not probability, percentage of song, or calibrated intensity. Confidence is distribution concentration, not correctness.",
        "display_cutoff":0.25,"display_cutoff_status":"provisional, not fitted to listening labels",
        "period_policy":"All era-fit scores retained. Recommend at most one era when its fit is at least 0.5 and exceeds the next by at least 0.15; otherwise report ambiguous. Never silently co-assign component eras with cross-era.",
        "disclosure":"Only audio-derived classifier labels and optional acoustic measurements go to Jev. No audio, paths, filenames, artist, album, collection, hashes, owner ratings or app credentials are sent. Raw dense scores remain local for listening review. No app tag writes, certification, retries or automatic resume.",
        "cases":cases,"max_requests":cases.len(),"max_input_units":units,"certifies_model":false}),
    )
}

fn request(case: &Value) -> Result<TypedDecisionRequest> {
    let request = TypedDecisionRequest {
        state: case["request"]["state"].clone(),
        questions: serde_json::from_value(case["request"]["questions"].clone())?,
    };
    request.validate()?;
    Ok(request)
}

pub(super) fn authorize(plan: &Value, expected: &str, calls: usize, units: u64) -> Result<()> {
    let rebuilt = build(plan["recordings"].as_array().ok_or("recordings")?)?;
    if &rebuilt != plan
        || fingerprint(plan)? != expected
        || plan["max_requests"].as_u64() != Some(calls as u64)
        || plan["max_input_units"].as_u64() != Some(units)
    {
        return Err("graded pilot differs from exact reviewed plan or budget".into());
    }
    Ok(())
}

#[cfg(test)]
pub(super) fn fixture_plan() -> Result<Value> {
    build(&tests::recordings()?)
}

fn scores(case: &Value, payload: Value) -> Result<Value> {
    let request = request(case)?;
    let answers = typed_answers(&request, payload)?;
    let mut scores = BTreeMap::new();
    for (id, answer) in answers {
        let TypedAnswer::Score {
            score,
            confidence,
            probabilities,
        } = answer
        else {
            return Err("graded result must contain native Scores".into());
        };
        let TypedQuestion::Score { criteria, .. } = &request.questions[&id] else {
            return Err("graded question must use native Score".into());
        };
        scores.insert(
            id,
            json!({"relevance":score/(criteria.len()-1) as f64,
            "score_confidence":confidence,"level_probabilities":probabilities,"raw_score":score,
            "status":"needs_listening_review"}),
        );
    }
    Ok(json!(scores))
}
