use std::collections::{BTreeMap, BTreeSet};

use serde::Deserialize;
use serde_json::{Value, json};

use super::structured_harness::{
    ModelTaskError, StructuredTaskDefinition, build_structured_request, safe_execution_error,
    truncate_chars,
};
use super::{
    AssistantTrackEvidence, PlaylistCandidate, PlaylistPlan, PlaylistSuggestion,
    PlaylistSuggestionRequest, StructuredModelRequest, StructuredModelResult,
    suggest_local_playlist,
};

pub const MODEL_PLAYLIST_INPUT_CONTRACT: &str = "assistant-playlist-planner-input/v3";
pub const MODEL_PLAYLIST_OUTPUT_CONTRACT: &str = "assistant-playlist-planner-output/v1";
pub const MODEL_PLAYLIST_ENGINE_ID: &str = "model-playlist-planner/v2";
pub const PLAYLIST_QUALITY_SUITE_ID: &str = "model-dnd-playlist-quality-v6";
pub const MAX_MODEL_PLAYLIST_CANDIDATES: usize = 100;

const PLAYLIST_TASK: StructuredTaskDefinition = StructuredTaskDefinition {
    task_id: "assistant-playlist-planner",
    role: "A cautious playlist refinement engine operating on a local plan.",
    objective: "Refine the server's deterministic candidate ranking and playback sequence without inventing tracks, metadata, scores, or evidence.",
    untrusted_data: &[
        "request.prompt",
        "candidate titles",
        "artists",
        "albums",
        "origins",
        "genres",
        "manual_tags",
        "analysis_tags",
    ],
    rules: &[
        "Every candidate already passed local exclusions and BPM eligibility. Use only candidate track_id values and never infer missing candidates.",
        "Treat manual_tags as operator-owned evidence, then explicit descriptive metadata, then generated analysis_tags and numeric local evidence. A weak source must not overrule a strong source without clear support.",
        "Use local_match_score, local_rank, local_default_selected, and local_plan as the deterministic baseline. Change that baseline only when the supplied evidence better satisfies the request.",
        "A null local_rank identifies an additional vocabulary-recalled candidate outside the original bounded local plan. It is not a top-ranked or default-selected local recommendation.",
        "Respect request.candidate_limit, target duration, energy_curve, effective_bpm, and the intended playback order. Unknown BPM is not zero BPM.",
        "ranked_track_ids contains the best review candidates in relevance order. selected_track_ids is a unique subset of those IDs in intended playback order.",
        "Do not explain the ranking or copy candidate text into the response; the server reconstructs all public metadata and reasons locally.",
    ],
};

#[derive(Debug)]
pub struct ModelPlaylistTask {
    request: PlaylistSuggestionRequest,
    baseline: PlaylistSuggestion,
    local_ranks: BTreeMap<music_domain::TrackId, usize>,
}

impl ModelPlaylistTask {
    pub fn new(
        tracks: &[AssistantTrackEvidence],
        request: &PlaylistSuggestionRequest,
    ) -> Result<Self, ModelTaskError> {
        let vocabulary =
            super::default_vocabulary().map_err(|_| ModelTaskError::new("model_input_invalid"))?;
        Self::with_vocabulary(tracks, request, &vocabulary)
    }

    pub fn with_vocabulary(
        tracks: &[AssistantTrackEvidence],
        request: &PlaylistSuggestionRequest,
        vocabulary: &super::TagVocabularyDocument,
    ) -> Result<Self, ModelTaskError> {
        let vocabulary = vocabulary
            .clone()
            .normalized()
            .map_err(|_| ModelTaskError::new("model_input_invalid"))?;
        request
            .validate()
            .map_err(|_| ModelTaskError::new("model_input_invalid"))?;
        let requested_limit = usize::from(request.candidate_limit);
        let prefilter_limit = requested_limit
            .saturating_mul(3)
            .max(requested_limit)
            .min(MAX_MODEL_PLAYLIST_CANDIDATES);
        let mut prefilter_request = request.clone();
        prefilter_request.candidate_limit = u16::try_from(prefilter_limit)
            .map_err(|_| ModelTaskError::new("model_input_invalid"))?;
        let mut baseline = suggest_local_playlist(tracks, &prefilter_request)
            .map_err(|_| ModelTaskError::new("model_input_invalid"))?;
        let local_ranks = baseline
            .candidates
            .iter()
            .enumerate()
            .map(|(index, candidate)| (candidate.track_id, index + 1))
            .collect();
        super::playlist_retrieval::supplement_candidates(
            tracks,
            &prefilter_request,
            &vocabulary,
            &mut baseline,
        )
        .map_err(|_| ModelTaskError::new("model_input_invalid"))?;
        if baseline.candidates.is_empty() {
            baseline.engine = MODEL_PLAYLIST_ENGINE_ID.to_owned();
        }
        Ok(Self {
            request: request.clone(),
            baseline,
            local_ranks,
        })
    }

    #[must_use]
    pub fn immediate_result(&self) -> Option<PlaylistSuggestion> {
        self.baseline
            .candidates
            .is_empty()
            .then(|| self.baseline.clone())
    }

    /// IDs in the actual locally prepared provider pool, before model ranking.
    pub fn candidate_track_ids(&self) -> impl Iterator<Item = i64> + '_ {
        self.baseline
            .candidates
            .iter()
            .map(|candidate| candidate.track_id.get())
    }

    #[must_use]
    pub fn request(&self) -> Option<StructuredModelRequest> {
        if self.baseline.candidates.is_empty() {
            return None;
        }
        let baseline_selected = selected_in_sequence(&self.baseline.candidates);
        let candidate_ids = self
            .baseline
            .candidates
            .iter()
            .map(|candidate| candidate.track_id.get())
            .collect::<Vec<_>>();
        let baseline_ranked_ids = candidate_ids
            .iter()
            .copied()
            .take(usize::from(self.request.candidate_limit))
            .collect::<Vec<_>>();
        let ranked = baseline_ranked_ids.iter().copied().collect::<BTreeSet<_>>();
        let baseline_selected_ids = baseline_selected
            .iter()
            .map(|candidate| candidate.track_id.get())
            .filter(|track_id| ranked.contains(track_id))
            .collect::<Vec<_>>();
        let input = json!({
            "schema_version": MODEL_PLAYLIST_INPUT_CONTRACT,
            "request": request_payload(&self.request),
            "intent_hint": {
                "matched_moods": self.baseline.intent.matched_moods,
                "search_terms": self.baseline.intent.search_terms,
                "energy": self.baseline.intent.energy,
                "brightness": self.baseline.intent.brightness,
                "tension": self.baseline.intent.tension,
            },
            "local_plan": {
                "selected_track_ids": baseline_selected
                    .iter()
                    .map(|candidate| candidate.track_id.get())
                    .collect::<Vec<_>>(),
                "selected_duration_s": self.baseline.plan.selected_duration_s,
                "target_duration_s": f64::from(self.request.target_minutes) * 60.0,
                "energy_curve": self.request.energy_curve.as_str(),
            },
            "candidates": self.baseline.candidates.iter()
                .map(|candidate| candidate_payload(candidate, self.local_ranks.get(&candidate.track_id).copied()))
                .collect::<Vec<_>>(),
        });
        Some(build_structured_request(
            &PLAYLIST_TASK,
            input,
            playlist_output_schema(&candidate_ids, usize::from(self.request.candidate_limit)),
            json!({
                "schema_version": MODEL_PLAYLIST_OUTPUT_CONTRACT,
                "ranked_track_ids": baseline_ranked_ids,
                "selected_track_ids": baseline_selected_ids,
            }),
            8_000,
        ))
    }

    pub fn finish(
        &self,
        result: StructuredModelResult,
    ) -> Result<PlaylistSuggestion, ModelTaskError> {
        if self.baseline.candidates.is_empty() {
            return Ok(self.baseline.clone());
        }
        if !result.succeeded {
            return Err(ModelTaskError::new(safe_execution_error(
                result.error_code.as_deref(),
            )));
        }
        if matches!(
            result.finish_reason.as_deref(),
            Some("length" | "max_tokens")
        ) {
            return Err(ModelTaskError::new("model_output_incomplete"));
        }
        let output: ModelPlaylistOutput = serde_json::from_value(
            result
                .payload
                .ok_or_else(|| ModelTaskError::new("model_execution_failed"))?,
        )
        .map_err(|error| ModelTaskError::invalid_output(error.to_string()))?;
        self.reconstruct(output)
    }

    fn reconstruct(
        &self,
        output: ModelPlaylistOutput,
    ) -> Result<PlaylistSuggestion, ModelTaskError> {
        let ranked_ids = output
            .ranked_track_ids
            .iter()
            .copied()
            .collect::<BTreeSet<_>>();
        let selected_ids = output
            .selected_track_ids
            .iter()
            .copied()
            .collect::<BTreeSet<_>>();
        if output.schema_version != MODEL_PLAYLIST_OUTPUT_CONTRACT
            || ranked_ids.len() != output.ranked_track_ids.len()
            || selected_ids.len() != output.selected_track_ids.len()
            || !selected_ids.is_subset(&ranked_ids)
        {
            return Err(ModelTaskError::invalid_output(
                "playlist IDs must be unique and selected IDs must be ranked",
            ));
        }
        if output.ranked_track_ids.len() > usize::from(self.request.candidate_limit) {
            return Err(ModelTaskError::new("model_output_candidate_limit_exceeded"));
        }
        let by_id = self
            .baseline
            .candidates
            .iter()
            .map(|candidate| (candidate.track_id.get(), candidate))
            .collect::<BTreeMap<_, _>>();
        if ranked_ids
            .iter()
            .any(|track_id| !by_id.contains_key(track_id))
        {
            return Err(ModelTaskError::new("model_output_unknown_track"));
        }
        let selected_positions = output
            .selected_track_ids
            .iter()
            .enumerate()
            .map(|(index, track_id)| (*track_id, index + 1))
            .collect::<BTreeMap<_, _>>();
        let candidates = output
            .ranked_track_ids
            .iter()
            .filter_map(|track_id| by_id.get(track_id))
            .map(|candidate| {
                let mut candidate = (*candidate).clone();
                candidate.default_selected =
                    selected_positions.contains_key(&candidate.track_id.get());
                candidate.sequence_position =
                    selected_positions.get(&candidate.track_id.get()).copied();
                candidate
            })
            .collect::<Vec<_>>();
        let selected = output
            .selected_track_ids
            .iter()
            .filter_map(|track_id| by_id.get(track_id))
            .collect::<Vec<_>>();
        Ok(PlaylistSuggestion {
            engine: MODEL_PLAYLIST_ENGINE_ID.to_owned(),
            library_tracks: self.baseline.library_tracks,
            eligible_tracks: self.baseline.eligible_tracks,
            intent: self.baseline.intent.clone(),
            plan: PlaylistPlan {
                energy_curve: self.request.energy_curve,
                selected_tracks: selected.len(),
                selected_duration_s: round_to(
                    selected.iter().map(|candidate| candidate.length_s).sum(),
                    3,
                ),
                audio_profile_tracks: candidates
                    .iter()
                    .filter(|candidate| candidate.audio_signal.is_some())
                    .count(),
            },
            candidates,
        })
    }
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
struct ModelPlaylistOutput {
    schema_version: String,
    ranked_track_ids: Vec<i64>,
    selected_track_ids: Vec<i64>,
}

fn selected_in_sequence(candidates: &[PlaylistCandidate]) -> Vec<&PlaylistCandidate> {
    let mut selected = candidates
        .iter()
        .filter(|candidate| candidate.default_selected && candidate.sequence_position.is_some())
        .collect::<Vec<_>>();
    selected.sort_by_key(|candidate| candidate.sequence_position.unwrap_or(usize::MAX));
    selected
}

fn request_payload(request: &PlaylistSuggestionRequest) -> Value {
    json!({
        "prompt": request.prompt,
        "target_minutes": request.target_minutes,
        "candidate_limit": request.candidate_limit,
        "min_bpm": request.min_bpm,
        "max_bpm": request.max_bpm,
        "include_unknown_bpm": request.include_unknown_bpm,
        "exclude_track_ids": request.exclude_track_ids.iter().map(|track_id| track_id.get()).collect::<Vec<_>>(),
        "energy_curve": request.energy_curve.as_str(),
    })
}

#[must_use]
pub fn playlist_suggestion_payload(suggestion: &PlaylistSuggestion) -> Value {
    json!({
        "engine": suggestion.engine,
        "library_tracks": suggestion.library_tracks,
        "eligible_tracks": suggestion.eligible_tracks,
        "intent": {
            "matched_moods": suggestion.intent.matched_moods,
            "search_terms": suggestion.intent.search_terms,
            "energy": suggestion.intent.energy,
            "brightness": suggestion.intent.brightness,
            "tension": suggestion.intent.tension,
        },
        "plan": {
            "energy_curve": suggestion.plan.energy_curve.as_str(),
            "selected_tracks": suggestion.plan.selected_tracks,
            "selected_duration_s": suggestion.plan.selected_duration_s,
            "audio_profile_tracks": suggestion.plan.audio_profile_tracks,
        },
        "candidates": suggestion.candidates.iter().map(|candidate| json!({
            "track_id": candidate.track_id.get(),
            "path": candidate.path,
            "title": candidate.title,
            "display_title": candidate.display_title,
            "artist": candidate.artist,
            "album": candidate.album,
            "origin": candidate.origin,
            "genre": candidate.genre,
            "manual_tags": candidate.manual_tags,
            "analysis_tags": candidate.analysis_tags,
            "length_s": candidate.length_s,
            "bpm": candidate.bpm,
            "match_score": candidate.match_score,
            "confidence": candidate.confidence.as_str(),
            "reasons": candidate.reasons,
            "default_selected": candidate.default_selected,
            "sequence_position": candidate.sequence_position,
            "planning_energy": candidate.planning_energy,
            "audio_signal": candidate.audio_signal.as_ref().map(|signal| json!({
                "analyzer_id": signal.analyzer_id,
                "energy": signal.energy,
                "brightness": signal.brightness,
                "tension": signal.tension,
                "tempo_bpm": signal.tempo_bpm,
                "confidence": signal.confidence.as_str(),
            })),
        })).collect::<Vec<_>>(),
    })
}

fn candidate_payload(candidate: &PlaylistCandidate, local_rank: Option<usize>) -> Value {
    let effective_bpm = candidate.bpm.map(f64::from).or_else(|| {
        candidate
            .audio_signal
            .as_ref()
            .and_then(|signal| signal.tempo_bpm)
    });
    let effective_bpm_source = if candidate.bpm.is_some() {
        "metadata"
    } else if effective_bpm.is_some() {
        "local-audio"
    } else {
        "unknown"
    };
    json!({
        "track_id": candidate.track_id.get(),
        "title": truncate_chars(&candidate.title, 512),
        "display_title": truncate_chars(&candidate.display_title, 512),
        "artist": truncate_chars(&candidate.artist, 512),
        "album": truncate_chars(&candidate.album, 512),
        "origin": truncate_chars(&candidate.origin, 512),
        "genre": truncate_chars(&candidate.genre, 128),
        "length_s": candidate.length_s,
        "bpm": candidate.bpm,
        "manual_tags": bounded_tags(&candidate.manual_tags, 32),
        "analysis_tags": bounded_tags(&candidate.analysis_tags, 50),
        "local_match_score": candidate.match_score,
        "planning_energy": candidate.planning_energy,
        "evidence_confidence": candidate.confidence.as_str(),
        "audio_signal": candidate.audio_signal.as_ref().map(|signal| json!({
            "analyzer_id": truncate_chars(&signal.analyzer_id, 128),
            "energy": signal.energy,
            "brightness": signal.brightness,
            "tension": signal.tension,
            "tempo_bpm": signal.tempo_bpm,
            "confidence": signal.confidence.as_str(),
        })),
        "local_rank": local_rank,
        "local_default_selected": candidate.default_selected,
        "local_sequence_position": candidate.sequence_position,
        "effective_bpm": effective_bpm,
        "effective_bpm_source": effective_bpm_source,
    })
}

fn bounded_tags(tags: &[String], maximum: usize) -> Vec<String> {
    tags.iter()
        .take(maximum)
        .map(|tag| truncate_chars(tag, 64))
        .filter(|tag| !tag.is_empty())
        .collect()
}

fn playlist_output_schema(candidate_ids: &[i64], candidate_limit: usize) -> Value {
    let maximum = candidate_limit.min(candidate_ids.len());
    let mut schema = super::structured_harness::output_schema::<ModelPlaylistOutput>();
    let properties = &mut schema["properties"];
    properties["schema_version"]["const"] = json!(MODEL_PLAYLIST_OUTPUT_CONTRACT);
    for field in ["ranked_track_ids", "selected_track_ids"] {
        properties[field]["maxItems"] = json!(maximum);
        properties[field]["uniqueItems"] = json!(true);
        properties[field]["items"]["enum"] = json!(candidate_ids);
    }
    schema
}

fn round_to(value: f64, places: i32) -> f64 {
    let factor = 10_f64.powi(places);
    (value * factor).round() / factor
}

#[cfg(test)]
mod tests {
    use crate::assistant::{PlaylistSuggestion, suggest_local_playlist};
    use std::collections::BTreeMap;
    use std::time::Duration;

    use music_domain::{IndexedTrack, LibraryPath, TrackId, TrackMetadata};
    use serde_json::json;

    use super::{MODEL_PLAYLIST_ENGINE_ID, ModelPlaylistTask};
    use crate::assistant::{
        AssistantTrackEvidence, EnergyCurve, PlaylistSuggestionRequest, StructuredModelResult,
    };

    fn track(
        id: i64,
        path: &str,
        title: &str,
    ) -> Result<AssistantTrackEvidence, Box<dyn std::error::Error>> {
        Ok(AssistantTrackEvidence {
            track: IndexedTrack {
                id: TrackId::new(id)?,
                path: LibraryPath::parse(path)?,
                metadata: TrackMetadata {
                    title: title.to_owned(),
                    artist: "Artist".to_owned(),
                    album_artist: String::new(),
                    album: "Album".to_owned(),
                    track_no: None,
                    disc_no: None,
                    year: None,
                    genre: "ambient".to_owned(),
                    bpm: None,
                },
                duration: Duration::from_secs(300),
                display_title: title.to_owned(),
                origin: String::new(),
                size_bytes: 1,
                mtime_unix_seconds: 1,
                added_at_unix_seconds: 1,
            },
            manual_tags: vec!["calm".to_owned()],
            analyses: Vec::new(),
            reviews: Vec::new(),
        })
    }

    fn request() -> PlaylistSuggestionRequest {
        PlaylistSuggestionRequest {
            prompt: "calm ambient".to_owned(),
            target_minutes: 5,
            candidate_limit: 5,
            min_bpm: None,
            max_bpm: None,
            include_unknown_bpm: true,
            exclude_track_ids: Vec::new(),
            energy_curve: EnergyCurve::Steady,
        }
    }

    #[test]
    fn derived_schema_agrees_with_strict_playlist_results() -> Result<(), Box<dyn std::error::Error>>
    {
        use crate::assistant::structured_harness::tests::{assert_output_contract, model_result};
        let task = ModelPlaylistTask::new(&[track(1, "Calm.flac", "Calm")?], &request())?;
        let schema = task
            .request()
            .and_then(|request| request.output_schema)
            .ok_or("missing schema")?;
        let valid = json!({"schema_version": super::MODEL_PLAYLIST_OUTPUT_CONTRACT,
            "ranked_track_ids": [1], "selected_track_ids": [1]});
        assert_output_contract(&schema, &valid, |value| {
            task.finish(model_result(value)).is_ok()
        })?;
        for ids in [json!([999]), json!([1, 1])] {
            let mut invalid = valid.clone();
            invalid["ranked_track_ids"] = ids;
            assert!(!jsonschema::is_valid(&schema, &invalid));
            assert!(task.finish(model_result(invalid)).is_err());
        }
        // Selected IDs must also be ranked, beyond each array's closed ID set.
        let mut unranked = valid;
        unranked["ranked_track_ids"] = json!([]);
        assert!(jsonschema::is_valid(&schema, &unranked));
        assert!(task.finish(model_result(unranked)).is_err());
        Ok(())
    }

    #[test]
    fn provider_payload_omits_local_paths_and_output_reuses_trusted_metadata()
    -> Result<(), Box<dyn std::error::Error>> {
        let task = ModelPlaylistTask::new(&[track(1, "Private/Calm.flac", "Calm")?], &request())?;
        let provider_request = task.request().ok_or("expected provider request")?;
        assert!(!provider_request.user_prompt.contains("Private/Calm.flac"));
        let response = task.finish(StructuredModelResult {
            outcome: crate::assistant::ProviderAttemptOutcome::ResponseReceived,
            succeeded: true,
            error_code: None,
            payload: Some(json!({
                "schema_version": "assistant-playlist-planner-output/v1",
                "ranked_track_ids": [1],
                "selected_track_ids": [1]
            })),
            provider_model_id: None,
            finish_reason: Some("stop".to_owned()),
            input_tokens: None,
            output_tokens: None,
        })?;
        assert_eq!(response.engine, MODEL_PLAYLIST_ENGINE_ID);
        assert_eq!(response.candidates[0].path, "Private/Calm.flac");
        assert_eq!(response.candidates[0].sequence_position, Some(1));
        Ok(())
    }

    #[test]
    fn unknown_provider_track_ids_fail_closed() -> Result<(), Box<dyn std::error::Error>> {
        let task = ModelPlaylistTask::new(&[track(1, "Calm.flac", "Calm")?], &request())?;
        let Err(error) = task.finish(StructuredModelResult {
            outcome: crate::assistant::ProviderAttemptOutcome::ResponseReceived,
            succeeded: true,
            error_code: None,
            payload: Some(json!({
                "schema_version": "assistant-playlist-planner-output/v1",
                "ranked_track_ids": [999],
                "selected_track_ids": []
            })),
            provider_model_id: None,
            finish_reason: Some("stop".to_owned()),
            input_tokens: None,
            output_tokens: None,
        }) else {
            return Err("unknown IDs must not be repaired".into());
        };
        assert_eq!(error.code, "model_output_unknown_track");
        Ok(())
    }

    #[test]
    fn vocabulary_recall_keeps_original_ranks_defaults_sources_and_pool_limits()
    -> Result<(), Box<dyn std::error::Error>> {
        let mut tracks = (1..=100)
            .map(|id| track(id, &format!("Private/{id}.flac"), "Neutral"))
            .collect::<Result<Vec<_>, _>>()?;
        let mut recalled = track(1001, "Private/Recall.flac", "Neutral")?;
        recalled.manual_tags = vec!["stealth".to_owned()];
        tracks.push(recalled);
        for limit in [5, 100] {
            let mut request = request();
            request.prompt = "covert approach".to_owned();
            request.candidate_limit = limit;
            let mut expanded = request.clone();
            expanded.candidate_limit = limit.saturating_mul(3).min(100);
            let original = suggest_local_playlist(&tracks, &expanded)?;
            let original_ranks = original
                .candidates
                .iter()
                .enumerate()
                .map(|(index, candidate)| (candidate.track_id, index + 1))
                .collect::<BTreeMap<_, _>>();
            let task = ModelPlaylistTask::new(&tracks, &request)?;
            assert_eq!(
                task.baseline.candidates.len(),
                usize::from(expanded.candidate_limit)
            );
            assert_eq!(task.baseline.plan, original.plan);
            let selected = |plan: &PlaylistSuggestion| {
                plan.candidates
                    .iter()
                    .filter(|candidate| candidate.default_selected)
                    .map(|candidate| candidate.track_id)
                    .collect::<Vec<_>>()
            };
            assert_eq!(selected(&task.baseline), selected(&original));
            for candidate in &task.baseline.candidates {
                if candidate.track_id.get() == 1001 {
                    assert!(!candidate.default_selected);
                    assert!(candidate.sequence_position.is_none());
                    assert_eq!(candidate.manual_tags, vec!["stealth"]);
                    assert_eq!(candidate.path, "Private/Recall.flac");
                    assert!(
                        super::candidate_payload(
                            candidate,
                            task.local_ranks.get(&candidate.track_id).copied()
                        )["local_rank"]
                            .is_null()
                    );
                } else {
                    assert_eq!(
                        task.local_ranks.get(&candidate.track_id),
                        original_ranks.get(&candidate.track_id)
                    );
                }
            }
            assert!(task.candidate_track_ids().any(|id| id == 1001));
            let provider_request = task.request().ok_or("request missing")?;
            assert!(!provider_request.user_prompt.contains("Private/"));
            let schema = provider_request.output_schema.ok_or("schema missing")?;
            assert!(
                schema["properties"]["ranked_track_ids"]["items"]["enum"]
                    .as_array()
                    .ok_or("ids missing")?
                    .contains(&json!(1001))
            );
            request.target_minutes = 600;
            let task = ModelPlaylistTask::new(&tracks, &request)?;
            assert!(
                !task.candidate_track_ids().any(|id| id == 1001),
                "recall must not evict local default selections"
            );
            request.target_minutes = 5;
            let mut many_matches = tracks.clone();
            for id in 1002..=1030 {
                let mut extra = track(id, &format!("Private/{id}.flac"), "Neutral")?;
                extra.manual_tags = vec!["stealth".to_owned()];
                many_matches.push(extra);
            }
            let task = ModelPlaylistTask::new(&many_matches, &request)?;
            assert_eq!(
                task.candidate_track_ids().count(),
                usize::from(expanded.candidate_limit)
            );
            assert_eq!(
                task.candidate_track_ids().filter(|id| *id > 100).count(),
                (usize::from(expanded.candidate_limit) / 4).min(20)
            );
        }
        Ok(())
    }

    #[test]
    fn recall_rechecks_eligibility_and_does_not_promote_unaccepted_or_partial_labels()
    -> Result<(), Box<dyn std::error::Error>> {
        let base = (1..=100)
            .map(|id| {
                let mut item = track(id, &format!("Fixture/{id}.flac"), "Neutral")?;
                item.track.metadata.bpm = Some(100);
                Ok(item)
            })
            .collect::<Result<Vec<_>, Box<dyn std::error::Error>>>()?;
        for scenario in [
            "canonical",
            "alias",
            "excluded",
            "slow",
            "unknown",
            "partial",
            "metadata-only",
            "analysis-only",
        ] {
            let mut tracks = base.clone();
            let mut target = track(1001, "Fixture/stealth.flac", "stealth")?;
            target.track.metadata.bpm = Some(100);
            target.manual_tags = vec!["stealth".to_owned()];
            let mut request = request();
            request.prompt = "covert approach".to_owned();
            request.min_bpm = Some(80);
            request.include_unknown_bpm = false;
            match scenario {
                "alias" => target.manual_tags = vec!["sneaking".to_owned()],
                "excluded" => request.exclude_track_ids.push(target.track.id),
                "slow" => target.track.metadata.bpm = Some(60),
                "unknown" => target.track.metadata.bpm = None,
                "partial" => target.manual_tags = vec!["stealth combat".to_owned()],
                "metadata-only" => target.manual_tags.clear(),
                "analysis-only" => {
                    target.manual_tags.clear();
                    target.analyses.push(super::super::StoredAnalysis {
                        analyzer_id: super::super::LOCAL_METADATA_ANALYZER_ID.to_owned(),
                        source_signature: super::super::metadata_source_signature(&target.track)?,
                        energy: 0.5,
                        brightness: 0.5,
                        tension: 0.5,
                        moods: vec!["stealth".to_owned()],
                        evidence: vec!["Synthetic suggestion".to_owned()],
                        metrics: serde_json::Map::new(),
                        confidence: "high".to_owned(),
                    });
                }
                _ => {}
            }
            tracks.push(target);
            let task = ModelPlaylistTask::new(&tracks, &request)?;
            assert_eq!(
                task.candidate_track_ids().any(|id| id == 1001),
                matches!(scenario, "canonical" | "alias"),
                "{scenario}"
            );
        }
        Ok(())
    }
}
