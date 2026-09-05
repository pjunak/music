use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Component, Path, PathBuf};
use std::time::Duration;

use music_domain::{IndexedTrack, LibraryPath, TrackId, TrackMetadata};
use serde::{Deserialize, Serialize};
use serde_json::{Map, json};
use sha2::{Digest, Sha256};

use super::{
    AssistantTrackEvidence, Confidence, EnergyCurve, LOCAL_AUDIO_ANALYZER_ID,
    LOCAL_METADATA_ANALYZER_ID, LOCAL_PLAYLIST_ENGINE_ID, MODEL_PLAYLIST_ENGINE_ID,
    ModelPlaylistTask, ModelTaskError, PLAYLIST_QUALITY_SUITE_ID, PlaylistCandidate,
    PlaylistSuggestion, PlaylistSuggestionRequest, StoredAnalysis, audio_source_signature,
    metadata_source_signature, playlist_suggestion_payload, suggest_local_playlist,
};

pub const PLAYLIST_EVALUATION_CONTRACT: &str = "playlist-evaluation/v1";
pub const PLAYLIST_EVALUATION_RESULT_CONTRACT: &str = "playlist-evaluation-result/v1";
const MAX_EVALUATION_SUITE_BYTES: u64 = 4 * 1_024 * 1_024;
const MAX_EVALUATION_INCLUDE_DEPTH: usize = 8;

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct EvaluationAnalysisProfile {
    energy: f64,
    brightness: f64,
    tension: f64,
    #[serde(default)]
    moods: Vec<String>,
    #[serde(default)]
    evidence: Vec<String>,
    confidence: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct EvaluationSignalProfile {
    #[serde(default = "default_signal_analyzer")]
    analyzer_id: String,
    energy: f64,
    brightness: f64,
    tension: f64,
    #[serde(default)]
    tempo_bpm: Option<f64>,
    confidence: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct EvaluationTrack {
    id: i64,
    path: String,
    title: String,
    #[serde(default)]
    display_title: String,
    #[serde(default)]
    artist: String,
    #[serde(default)]
    album: String,
    #[serde(default)]
    origin: String,
    #[serde(default)]
    genre: String,
    #[serde(default = "default_track_length")]
    length_s: f64,
    #[serde(default)]
    bpm: Option<u32>,
    #[serde(default)]
    manual_tags: Vec<String>,
    #[serde(default)]
    analysis: Option<EvaluationAnalysisProfile>,
    #[serde(default)]
    signal: Option<EvaluationSignalProfile>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct GeneratedEvaluationTracks {
    count: usize,
    id_start: i64,
    path_prefix: String,
    title_prefix: String,
    #[serde(default = "default_generated_artist")]
    artist: String,
    #[serde(default = "default_generated_genre")]
    genre: String,
    #[serde(default = "default_track_length")]
    length_s: f64,
    #[serde(default)]
    manual_tags: Vec<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct EvaluationRequest {
    prompt: String,
    #[serde(default = "default_target_minutes")]
    target_minutes: u16,
    #[serde(default = "default_candidate_limit")]
    candidate_limit: u16,
    #[serde(default)]
    min_bpm: Option<u32>,
    #[serde(default)]
    max_bpm: Option<u32>,
    #[serde(default = "default_true")]
    include_unknown_bpm: bool,
    #[serde(default)]
    exclude_track_ids: Vec<i64>,
    #[serde(default)]
    energy_curve: EnergyCurve,
}

impl EvaluationRequest {
    fn application_request(&self) -> Result<PlaylistSuggestionRequest, ModelTaskError> {
        let request = PlaylistSuggestionRequest {
            prompt: self.prompt.trim().to_owned(),
            target_minutes: self.target_minutes,
            candidate_limit: self.candidate_limit,
            min_bpm: self.min_bpm,
            max_bpm: self.max_bpm,
            include_unknown_bpm: self.include_unknown_bpm,
            exclude_track_ids: self
                .exclude_track_ids
                .iter()
                .map(|track_id| TrackId::new(*track_id))
                .collect::<Result<Vec<_>, _>>()
                .map_err(|_| ModelTaskError::new("model_evaluation_suite_invalid"))?,
            energy_curve: self.energy_curve,
        };
        request
            .validate()
            .map_err(|_| ModelTaskError::new("model_evaluation_suite_invalid"))?;
        Ok(request)
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct EvaluationExpectations {
    #[serde(default = "default_top_k")]
    top_k: usize,
    relevant_track_ids: Vec<i64>,
    #[serde(default)]
    forbidden_track_ids: Vec<i64>,
    #[serde(default)]
    required_default_track_ids: Vec<i64>,
    #[serde(default)]
    order_pairs: Vec<(i64, i64)>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct EvaluationThresholds {
    #[serde(default)]
    min_precision_at_k: f64,
    #[serde(default)]
    min_recall_at_k: f64,
    #[serde(default)]
    min_reciprocal_rank: f64,
    #[serde(default)]
    min_required_selected_recall: f64,
    #[serde(default)]
    min_order_pair_accuracy: f64,
    #[serde(default = "default_one")]
    min_reason_coverage: f64,
    #[serde(default)]
    min_selected_artist_diversity: f64,
    #[serde(default = "default_one")]
    max_duration_error_ratio: f64,
    #[serde(default)]
    max_forbidden_candidates: usize,
    #[serde(default)]
    require_deterministic: bool,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawPlaylistQualityCase {
    id: String,
    description: String,
    request: EvaluationRequest,
    #[serde(default)]
    tracks: Vec<EvaluationTrack>,
    #[serde(default)]
    generated_tracks: Option<GeneratedEvaluationTracks>,
    #[serde(default)]
    vocabulary: Option<super::TagVocabularyDocument>,
    expectations: EvaluationExpectations,
    #[serde(default = "default_thresholds")]
    thresholds: EvaluationThresholds,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawPlaylistQualitySuite {
    schema_version: String,
    id: String,
    description: String,
    #[serde(default)]
    include_cases_from: Option<String>,
    cases: Vec<RawPlaylistQualityCase>,
}

#[derive(Debug, Clone)]
pub struct PlaylistQualityCase {
    id: String,
    description: String,
    request: PlaylistSuggestionRequest,
    source: Vec<AssistantTrackEvidence>,
    fixture_tracks: Vec<EvaluationTrack>,
    expectations: EvaluationExpectations,
    thresholds: EvaluationThresholds,
    vocabulary: super::TagVocabularyDocument,
}

impl PlaylistQualityCase {
    #[must_use]
    pub fn id(&self) -> &str {
        &self.id
    }

    #[must_use]
    pub const fn requires_repeat(&self) -> bool {
        self.thresholds.require_deterministic
    }

    pub fn task(&self) -> Result<ModelPlaylistTask, ModelTaskError> {
        ModelPlaylistTask::with_vocabulary(&self.source, &self.request, &self.vocabulary)
    }

    #[must_use]
    pub fn candidate_recall(&self, task: &ModelPlaylistTask) -> PlaylistCandidateRecall {
        let pool = task.candidate_track_ids().collect::<BTreeSet<_>>();
        let missing_relevant_track_ids = self
            .expectations
            .relevant_track_ids
            .iter()
            .copied()
            .filter(|id| !pool.contains(id))
            .collect::<Vec<_>>();
        let relevant_tracks = self.expectations.relevant_track_ids.len();
        let relevant_in_pool = relevant_tracks - missing_relevant_track_ids.len();
        PlaylistCandidateRecall {
            pool_tracks: pool.len(),
            relevant_tracks,
            relevant_in_pool,
            recall: round_to(relevant_in_pool as f64 / relevant_tracks as f64, 4),
            missing_relevant_track_ids,
        }
    }

    #[must_use]
    pub fn assess(
        &self,
        first: Result<PlaylistSuggestion, ModelTaskError>,
        repeated: Option<Result<PlaylistSuggestion, ModelTaskError>>,
    ) -> PlaylistEvaluationCaseResult {
        self.assess_for_engine(MODEL_PLAYLIST_ENGINE_ID, first, repeated)
    }

    #[must_use]
    pub fn assess_for_engine(
        &self,
        engine_id: &str,
        first: Result<PlaylistSuggestion, ModelTaskError>,
        repeated: Option<Result<PlaylistSuggestion, ModelTaskError>>,
    ) -> PlaylistEvaluationCaseResult {
        let candidate_recall = (engine_id == MODEL_PLAYLIST_ENGINE_ID)
            .then(|| self.task().map(|task| self.candidate_recall(&task)).ok())
            .flatten();
        let Ok(response) = first else {
            let mut result = engine_error_result(self, first.err());
            result.candidate_recall = candidate_recall;
            return result;
        };
        let assessment = assess_response(self, engine_id, &response);
        let mut metrics = assessment.metrics.clone();
        let mut failures = assessment.failures.clone();
        let mut repeated_top_track_ids = None;
        let mut repeated_selected_track_ids = None;
        let mut repeated_response_fingerprint = None;
        let mut exact_response_match = None;

        if self.thresholds.require_deterministic {
            match repeated {
                Some(Ok(repeated_response)) => {
                    let repeated_assessment = assess_response(self, engine_id, &repeated_response);
                    let exact =
                        assessment.response_fingerprint == repeated_assessment.response_fingerprint;
                    let top_stable = assessment
                        .top_track_ids
                        .iter()
                        .copied()
                        .collect::<BTreeSet<_>>()
                        == repeated_assessment
                            .top_track_ids
                            .iter()
                            .copied()
                            .collect::<BTreeSet<_>>();
                    let selected_stable =
                        assessment.selected_track_ids == repeated_assessment.selected_track_ids;
                    metrics.deterministic = Some(top_stable && selected_stable);
                    if !exact {
                        failures.extend(
                            repeated_assessment
                                .failures
                                .iter()
                                .map(|failure| format!("repeated response {failure}")),
                        );
                    }
                    if !top_stable {
                        failures.push(format!(
                            "repeated response changed the top candidate set: first {}, repeated {}",
                            format_track_ids(&assessment.top_track_ids),
                            format_track_ids(&repeated_assessment.top_track_ids),
                        ));
                    }
                    if !selected_stable {
                        failures.push(format!(
                            "repeated response changed the selected playback sequence: first {}, repeated {}",
                            format_track_ids(&assessment.selected_track_ids),
                            format_track_ids(&repeated_assessment.selected_track_ids),
                        ));
                    }
                    repeated_top_track_ids = Some(repeated_assessment.top_track_ids);
                    repeated_selected_track_ids = Some(repeated_assessment.selected_track_ids);
                    repeated_response_fingerprint = Some(repeated_assessment.response_fingerprint);
                    exact_response_match = Some(exact);
                }
                Some(Err(error)) => {
                    metrics.deterministic = Some(false);
                    failures.push(format_task_failure("repeated response model error", &error));
                }
                None => {
                    metrics.deterministic = Some(false);
                    failures.push("required repeated response was not executed".to_owned());
                }
            }
        }

        PlaylistEvaluationCaseResult {
            candidate_recall,
            id: self.id.clone(),
            description: self.description.clone(),
            passed: failures.is_empty(),
            metrics,
            failures,
            top_track_ids: assessment.top_track_ids,
            selected_track_ids: assessment.selected_track_ids,
            response_fingerprint: assessment.response_fingerprint,
            repeated_top_track_ids,
            repeated_selected_track_ids,
            repeated_response_fingerprint,
            exact_response_match,
        }
    }
}

#[derive(Debug)]
pub struct PlaylistQualitySuite {
    pub schema_version: &'static str,
    pub id: String,
    pub description: String,
    pub cases: Vec<PlaylistQualityCase>,
}

#[derive(Debug, Clone, Serialize)]
pub struct PlaylistEvaluationMetrics {
    pub precision_at_k: f64,
    pub recall_at_k: f64,
    pub reciprocal_rank: f64,
    pub required_selected_recall: Option<f64>,
    pub order_pair_accuracy: Option<f64>,
    pub reason_coverage: f64,
    pub selected_artist_diversity: f64,
    pub duration_error_ratio: f64,
    pub forbidden_candidate_count: usize,
    pub unknown_candidate_count: usize,
    pub excluded_candidate_count: usize,
    pub source_mismatch_count: usize,
    pub deterministic: Option<bool>,
    pub contract_valid: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct PlaylistEvaluationCaseResult {
    pub candidate_recall: Option<PlaylistCandidateRecall>,
    pub id: String,
    pub description: String,
    pub passed: bool,
    pub metrics: PlaylistEvaluationMetrics,
    pub failures: Vec<String>,
    pub top_track_ids: Vec<i64>,
    pub selected_track_ids: Vec<i64>,
    pub response_fingerprint: String,
    pub repeated_top_track_ids: Option<Vec<i64>>,
    pub repeated_selected_track_ids: Option<Vec<i64>>,
    pub repeated_response_fingerprint: Option<String>,
    pub exact_response_match: Option<bool>,
}

#[derive(Debug, Clone, Serialize)]
pub struct PlaylistCandidateRecall {
    pub pool_tracks: usize,
    pub relevant_tracks: usize,
    pub relevant_in_pool: usize,
    pub recall: f64,
    pub missing_relevant_track_ids: Vec<i64>,
}

#[derive(Debug, Serialize)]
pub struct PlaylistEvaluationSummary {
    pub cases: u32,
    pub passed_cases: u32,
    pub failed_cases: u32,
    pub mean_precision_at_k: f64,
    pub mean_recall_at_k: f64,
    pub mean_reciprocal_rank: f64,
    pub mean_required_selected_recall: Option<f64>,
    pub mean_order_pair_accuracy: Option<f64>,
    pub mean_reason_coverage: f64,
    pub mean_selected_artist_diversity: f64,
    pub mean_duration_error_ratio: f64,
}

#[derive(Debug, Serialize)]
pub struct PlaylistQualityEvaluationResult {
    pub schema_version: &'static str,
    pub suite_id: String,
    pub engine_id: &'static str,
    pub passed: bool,
    pub summary: PlaylistEvaluationSummary,
    pub cases: Vec<PlaylistEvaluationCaseResult>,
}

impl PlaylistQualityEvaluationResult {
    pub fn from_cases(
        suite: &PlaylistQualitySuite,
        engine_id: &'static str,
        cases: Vec<PlaylistEvaluationCaseResult>,
    ) -> Result<Self, ModelTaskError> {
        if cases
            .iter()
            .map(|case| case.id.as_str())
            .ne(suite.cases.iter().map(PlaylistQualityCase::id))
        {
            return Err(ModelTaskError::new("model_evaluation_result_invalid"));
        }
        let total_cases = u32::try_from(cases.len()).unwrap_or(u32::MAX);
        let passed_cases =
            u32::try_from(cases.iter().filter(|case| case.passed).count()).unwrap_or(u32::MAX);
        Ok(Self {
            schema_version: PLAYLIST_EVALUATION_RESULT_CONTRACT,
            suite_id: suite.id.clone(),
            engine_id,
            passed: passed_cases == total_cases,
            summary: PlaylistEvaluationSummary {
                cases: total_cases,
                passed_cases,
                failed_cases: total_cases.saturating_sub(passed_cases),
                mean_precision_at_k: mean_or_zero(
                    cases.iter().map(|case| case.metrics.precision_at_k),
                ),
                mean_recall_at_k: mean_or_zero(cases.iter().map(|case| case.metrics.recall_at_k)),
                mean_reciprocal_rank: mean_or_zero(
                    cases.iter().map(|case| case.metrics.reciprocal_rank),
                ),
                mean_required_selected_recall: mean(
                    cases
                        .iter()
                        .filter_map(|case| case.metrics.required_selected_recall),
                ),
                mean_order_pair_accuracy: mean(
                    cases
                        .iter()
                        .filter_map(|case| case.metrics.order_pair_accuracy),
                ),
                mean_reason_coverage: mean_or_zero(
                    cases.iter().map(|case| case.metrics.reason_coverage),
                ),
                mean_selected_artist_diversity: mean_or_zero(
                    cases
                        .iter()
                        .map(|case| case.metrics.selected_artist_diversity),
                ),
                mean_duration_error_ratio: mean_or_zero(
                    cases.iter().map(|case| case.metrics.duration_error_ratio),
                ),
            },
            cases,
        })
    }
}

#[derive(Debug)]
struct ResponseAssessment {
    metrics: PlaylistEvaluationMetrics,
    failures: Vec<String>,
    top_track_ids: Vec<i64>,
    selected_track_ids: Vec<i64>,
    response_fingerprint: String,
}

pub fn playlist_quality_suite() -> Result<PlaylistQualitySuite, ModelTaskError> {
    let local: RawPlaylistQualitySuite =
        serde_json::from_str(include_str!("evaluation_suites/playlist-local-v1.json"))
            .map_err(|error| ModelTaskError::invalid_output(error.to_string()))?;
    let model: RawPlaylistQualitySuite =
        serde_json::from_str(include_str!("evaluation_suites/playlist-model-v1.json"))
            .map_err(|error| ModelTaskError::invalid_output(error.to_string()))?;
    if local.schema_version != PLAYLIST_EVALUATION_CONTRACT
        || local.id != "local-dnd-playlist-baseline-v4"
        || local.include_cases_from.is_some()
        || model.schema_version != PLAYLIST_EVALUATION_CONTRACT
        || model.id != PLAYLIST_QUALITY_SUITE_ID
        || model.include_cases_from.as_deref() != Some("playlist-local-v1.json")
    {
        return Err(ModelTaskError::new("model_evaluation_suite_invalid"));
    }
    let cases = local.cases.into_iter().chain(model.cases).collect();
    materialize_suite(RawPlaylistQualitySuite {
        schema_version: model.schema_version,
        id: model.id,
        description: model.description,
        include_cases_from: None,
        cases,
    })
}

pub fn load_playlist_quality_suite(path: &Path) -> Result<PlaylistQualitySuite, ModelTaskError> {
    let mut visited = BTreeSet::new();
    let raw = load_raw_suite(path, 0, &mut visited)?;
    materialize_suite(raw)
}

pub fn evaluate_local_playlist_suite(
    suite: &PlaylistQualitySuite,
) -> Result<PlaylistQualityEvaluationResult, ModelTaskError> {
    let cases = suite
        .cases
        .iter()
        .map(|case| {
            let evaluate = || {
                suggest_local_playlist(&case.source, &case.request)
                    .map_err(|_| ModelTaskError::new("local_evaluation_failed"))
            };
            let first = evaluate();
            let repeated = case.requires_repeat().then(evaluate);
            case.assess_for_engine(LOCAL_PLAYLIST_ENGINE_ID, first, repeated)
        })
        .collect();
    PlaylistQualityEvaluationResult::from_cases(suite, LOCAL_PLAYLIST_ENGINE_ID, cases)
}

/// Local candidate availability only; this result cannot certify model quality.
#[derive(Debug, Serialize)]
pub struct PlaylistCandidateEvaluation {
    pub schema_version: &'static str,
    pub suite_id: String,
    pub cases: Vec<PlaylistCandidateCaseEvaluation>,
}

#[derive(Debug, Serialize)]
pub struct PlaylistCandidateCaseEvaluation {
    pub id: String,
    pub candidate_recall: PlaylistCandidateRecall,
}

pub fn evaluate_playlist_candidates(
    suite: &PlaylistQualitySuite,
) -> Result<PlaylistCandidateEvaluation, ModelTaskError> {
    Ok(PlaylistCandidateEvaluation {
        schema_version: "playlist-candidate-evaluation/v1",
        suite_id: suite.id.clone(),
        cases: suite
            .cases
            .iter()
            .map(|case| {
                let task = case.task()?;
                Ok(PlaylistCandidateCaseEvaluation {
                    id: case.id.clone(),
                    candidate_recall: case.candidate_recall(&task),
                })
            })
            .collect::<Result<Vec<_>, ModelTaskError>>()?,
    })
}

fn load_raw_suite(
    path: &Path,
    depth: usize,
    visited: &mut BTreeSet<PathBuf>,
) -> Result<RawPlaylistQualitySuite, ModelTaskError> {
    if depth > MAX_EVALUATION_INCLUDE_DEPTH {
        return Err(ModelTaskError::new("model_evaluation_suite_invalid"));
    }
    let canonical = fs::canonicalize(path)
        .map_err(|_| ModelTaskError::new("model_evaluation_suite_unreadable"))?;
    if !visited.insert(canonical.clone()) {
        return Err(ModelTaskError::new("model_evaluation_suite_invalid"));
    }
    let metadata = fs::metadata(&canonical)
        .map_err(|_| ModelTaskError::new("model_evaluation_suite_unreadable"))?;
    if !metadata.is_file() || metadata.len() > MAX_EVALUATION_SUITE_BYTES {
        return Err(ModelTaskError::new("model_evaluation_suite_invalid"));
    }
    let source = fs::read_to_string(&canonical)
        .map_err(|_| ModelTaskError::new("model_evaluation_suite_unreadable"))?;
    let mut raw: RawPlaylistQualitySuite = serde_json::from_str(&source)
        .map_err(|_| ModelTaskError::new("model_evaluation_suite_invalid"))?;
    if let Some(include) = raw.include_cases_from.take() {
        let include_path = Path::new(&include);
        if include_path.components().count() != 1
            || !matches!(include_path.components().next(), Some(Component::Normal(_)))
        {
            return Err(ModelTaskError::new("model_evaluation_suite_invalid"));
        }
        let parent = canonical
            .parent()
            .ok_or_else(|| ModelTaskError::new("model_evaluation_suite_invalid"))?;
        let included = load_raw_suite(&parent.join(include_path), depth + 1, visited)?;
        raw.cases = included.cases.into_iter().chain(raw.cases).collect();
    }
    visited.remove(&canonical);
    Ok(raw)
}

fn materialize_suite(raw: RawPlaylistQualitySuite) -> Result<PlaylistQualitySuite, ModelTaskError> {
    if raw.schema_version != PLAYLIST_EVALUATION_CONTRACT
        || !valid_case_id(&raw.id)
        || raw.description.is_empty()
        || raw.description.chars().count() > 2_000
        || raw.include_cases_from.is_some()
    {
        return Err(ModelTaskError::new("model_evaluation_suite_invalid"));
    }
    let cases = raw
        .cases
        .into_iter()
        .map(materialize_case)
        .collect::<Result<Vec<_>, _>>()?;
    let case_ids = cases
        .iter()
        .map(PlaylistQualityCase::id)
        .collect::<BTreeSet<_>>();
    if cases.is_empty() || cases.len() > 200 || case_ids.len() != cases.len() {
        return Err(ModelTaskError::new("model_evaluation_suite_invalid"));
    }
    Ok(PlaylistQualitySuite {
        schema_version: PLAYLIST_EVALUATION_CONTRACT,
        id: raw.id,
        description: raw.description,
        cases,
    })
}

fn materialize_case(
    mut raw: RawPlaylistQualityCase,
) -> Result<PlaylistQualityCase, ModelTaskError> {
    if !valid_case_id(&raw.id)
        || raw.description.is_empty()
        || raw.description.chars().count() > 1_000
    {
        return Err(ModelTaskError::new("model_evaluation_suite_invalid"));
    }
    if let Some(generated) = raw.generated_tracks.take() {
        raw.tracks.extend(materialize_generated(&generated)?);
    }
    validate_case(&raw)?;
    let vocabulary = raw
        .vocabulary
        .clone()
        .map_or_else(super::default_vocabulary, Ok)
        .and_then(super::TagVocabularyDocument::normalized)
        .map_err(|_| ModelTaskError::new("model_evaluation_suite_invalid"))?;
    let request = raw.request.application_request()?;
    let source = raw
        .tracks
        .iter()
        .map(evaluation_track_evidence)
        .collect::<Result<Vec<_>, _>>()?;
    Ok(PlaylistQualityCase {
        id: raw.id,
        description: raw.description,
        request,
        source,
        fixture_tracks: raw.tracks,
        expectations: raw.expectations,
        thresholds: raw.thresholds,
        vocabulary,
    })
}

fn materialize_generated(
    generated: &GeneratedEvaluationTracks,
) -> Result<Vec<EvaluationTrack>, ModelTaskError> {
    if !(1..=100).contains(&generated.count)
        || generated.id_start <= 0
        || generated.path_prefix.is_empty()
        || generated.path_prefix.chars().count() > 200
        || generated.title_prefix.is_empty()
        || generated.title_prefix.chars().count() > 200
    {
        return Err(ModelTaskError::new("model_evaluation_suite_invalid"));
    }
    (0..generated.count)
        .map(|offset| {
            let numeric_offset = i64::try_from(offset)
                .map_err(|_| ModelTaskError::new("model_evaluation_suite_invalid"))?;
            let id = generated
                .id_start
                .checked_add(numeric_offset)
                .ok_or_else(|| ModelTaskError::new("model_evaluation_suite_invalid"))?;
            Ok(EvaluationTrack {
                id,
                path: format!("{}/{:03}.flac", generated.path_prefix, offset + 1),
                title: format!("{} {:03}", generated.title_prefix, offset + 1),
                display_title: String::new(),
                artist: generated.artist.clone(),
                album: String::new(),
                origin: String::new(),
                genre: generated.genre.clone(),
                length_s: generated.length_s,
                bpm: None,
                manual_tags: generated.manual_tags.clone(),
                analysis: None,
                signal: None,
            })
        })
        .collect()
}

fn validate_case(raw: &RawPlaylistQualityCase) -> Result<(), ModelTaskError> {
    let request = raw.request.application_request()?;
    let known = raw
        .tracks
        .iter()
        .map(|track| track.id)
        .collect::<BTreeSet<_>>();
    let relevant = raw
        .expectations
        .relevant_track_ids
        .iter()
        .copied()
        .collect::<BTreeSet<_>>();
    let forbidden = raw
        .expectations
        .forbidden_track_ids
        .iter()
        .copied()
        .collect::<BTreeSet<_>>();
    let required = raw
        .expectations
        .required_default_track_ids
        .iter()
        .copied()
        .collect::<BTreeSet<_>>();
    let referenced = relevant
        .union(&forbidden)
        .copied()
        .chain(required.iter().copied())
        .chain(
            raw.expectations
                .order_pairs
                .iter()
                .flat_map(|pair| [pair.0, pair.1]),
        )
        .collect::<BTreeSet<_>>();
    if raw.tracks.is_empty()
        || raw.tracks.len() > 1_000
        || known.len() != raw.tracks.len()
        || relevant.is_empty()
        || raw.expectations.relevant_track_ids.len() != relevant.len()
        || raw.expectations.forbidden_track_ids.len() != forbidden.len()
        || raw.expectations.required_default_track_ids.len() != required.len()
        || !referenced.is_subset(&known)
        || !relevant.is_disjoint(&forbidden)
        || !required.is_subset(&relevant)
        || raw.expectations.top_k == 0
        || raw.expectations.top_k > usize::from(request.candidate_limit)
        || raw
            .expectations
            .order_pairs
            .iter()
            .any(|(before, after)| before == after)
        || raw
            .expectations
            .order_pairs
            .iter()
            .collect::<BTreeSet<_>>()
            .len()
            != raw.expectations.order_pairs.len()
        || (required.is_empty() && raw.thresholds.min_required_selected_recall > 0.0)
        || (raw.expectations.order_pairs.is_empty() && raw.thresholds.min_order_pair_accuracy > 0.0)
        || !thresholds_valid(&raw.thresholds)
        || raw.tracks.iter().any(|track| !track_valid(track))
    {
        return Err(ModelTaskError::new("model_evaluation_suite_invalid"));
    }
    Ok(())
}

fn track_valid(track: &EvaluationTrack) -> bool {
    track.id > 0
        && !track.path.is_empty()
        && track.path.chars().count() <= 1_000
        && LibraryPath::parse(track.path.clone()).is_ok()
        && !track.title.is_empty()
        && track.title.chars().count() <= 500
        && track.display_title.chars().count() <= 500
        && track.artist.chars().count() <= 500
        && track.album.chars().count() <= 500
        && track.origin.chars().count() <= 500
        && track.genre.chars().count() <= 500
        && track.length_s.is_finite()
        && (0.0..=86_400.0).contains(&track.length_s)
        && track.bpm.is_none_or(|bpm| (1..=999).contains(&bpm))
        && track.manual_tags.len() <= 100
        && track.analysis.as_ref().is_none_or(|analysis| {
            axes_valid(analysis.energy, analysis.brightness, analysis.tension)
                && Confidence::parse(&analysis.confidence).is_some()
                && analysis.moods.len() <= 50
                && analysis.evidence.len() <= 50
        })
        && track.signal.as_ref().is_none_or(|signal| {
            !signal.analyzer_id.is_empty()
                && signal.analyzer_id.chars().count() <= 128
                && axes_valid(signal.energy, signal.brightness, signal.tension)
                && signal
                    .tempo_bpm
                    .is_none_or(|tempo| tempo.is_finite() && tempo > 0.0 && tempo <= 999.0)
                && Confidence::parse(&signal.confidence).is_some()
        })
}

fn thresholds_valid(thresholds: &EvaluationThresholds) -> bool {
    [
        thresholds.min_precision_at_k,
        thresholds.min_recall_at_k,
        thresholds.min_reciprocal_rank,
        thresholds.min_required_selected_recall,
        thresholds.min_order_pair_accuracy,
        thresholds.min_reason_coverage,
        thresholds.min_selected_artist_diversity,
        thresholds.max_duration_error_ratio,
    ]
    .iter()
    .all(|value| value.is_finite() && (0.0..=1.0).contains(value))
}

fn evaluation_track_evidence(
    fixture: &EvaluationTrack,
) -> Result<AssistantTrackEvidence, ModelTaskError> {
    let track = IndexedTrack {
        id: TrackId::new(fixture.id)
            .map_err(|_| ModelTaskError::new("model_evaluation_suite_invalid"))?,
        path: LibraryPath::parse(fixture.path.clone())
            .map_err(|_| ModelTaskError::new("model_evaluation_suite_invalid"))?,
        metadata: TrackMetadata {
            title: fixture.title.clone(),
            artist: fixture.artist.clone(),
            album_artist: String::new(),
            album: fixture.album.clone(),
            track_no: None,
            disc_no: None,
            year: None,
            genre: fixture.genre.clone(),
            bpm: fixture.bpm,
        },
        duration: Duration::from_secs_f64(fixture.length_s),
        display_title: fixture.display_title.clone(),
        origin: fixture.origin.clone(),
        size_bytes: 1,
        mtime_unix_seconds: fixture.id,
        added_at_unix_seconds: 0,
    };
    let mut analyses = Vec::new();
    if let Some(analysis) = &fixture.analysis {
        analyses.push(StoredAnalysis {
            analyzer_id: LOCAL_METADATA_ANALYZER_ID.to_owned(),
            source_signature: metadata_source_signature(&track)
                .map_err(|_| ModelTaskError::new("model_evaluation_suite_invalid"))?,
            energy: analysis.energy,
            brightness: analysis.brightness,
            tension: analysis.tension,
            moods: analysis.moods.clone(),
            evidence: analysis.evidence.clone(),
            metrics: Map::new(),
            confidence: analysis.confidence.clone(),
        });
    }
    if let Some(signal) = &fixture.signal {
        let mut metrics = Map::new();
        metrics.insert("schema".to_owned(), json!(LOCAL_AUDIO_ANALYZER_ID));
        metrics.insert("tempo_bpm".to_owned(), json!(signal.tempo_bpm));
        analyses.push(StoredAnalysis {
            analyzer_id: LOCAL_AUDIO_ANALYZER_ID.to_owned(),
            source_signature: audio_source_signature(&track)
                .map_err(|_| ModelTaskError::new("model_evaluation_suite_invalid"))?,
            energy: signal.energy,
            brightness: signal.brightness,
            tension: signal.tension,
            moods: Vec::new(),
            evidence: vec![format!("Synthetic {} fixture", signal.analyzer_id)],
            metrics,
            confidence: signal.confidence.clone(),
        });
    }
    Ok(AssistantTrackEvidence {
        track,
        manual_tags: fixture.manual_tags.clone(),
        analyses,
        reviews: Vec::new(),
    })
}

fn assess_response(
    case: &PlaylistQualityCase,
    engine_id: &str,
    response: &PlaylistSuggestion,
) -> ResponseAssessment {
    let candidate_ids = response
        .candidates
        .iter()
        .map(|candidate| candidate.track_id.get())
        .collect::<Vec<_>>();
    let top_track_ids = candidate_ids
        .iter()
        .copied()
        .take(case.expectations.top_k)
        .collect::<Vec<_>>();
    let mut selected = response
        .candidates
        .iter()
        .filter(|candidate| candidate.default_selected)
        .collect::<Vec<_>>();
    selected.sort_by_key(|candidate| {
        (
            candidate.sequence_position.is_none(),
            candidate.sequence_position.unwrap_or(usize::MAX),
            candidate_ids
                .iter()
                .position(|track_id| *track_id == candidate.track_id.get())
                .unwrap_or(usize::MAX),
        )
    });
    let selected_track_ids = selected
        .iter()
        .map(|candidate| candidate.track_id.get())
        .collect::<Vec<_>>();
    let relevant = case
        .expectations
        .relevant_track_ids
        .iter()
        .copied()
        .collect::<BTreeSet<_>>();
    let relevant_in_top = top_track_ids
        .iter()
        .filter(|track_id| relevant.contains(track_id))
        .count();
    let precision = if top_track_ids.is_empty() {
        0.0
    } else {
        relevant_in_top as f64 / top_track_ids.len() as f64
    };
    let recall = relevant_in_top as f64 / relevant.len() as f64;
    let reciprocal_rank = candidate_ids
        .iter()
        .position(|track_id| relevant.contains(track_id))
        .map_or(0.0, |index| 1.0 / (index + 1) as f64);
    let required = case
        .expectations
        .required_default_track_ids
        .iter()
        .copied()
        .collect::<BTreeSet<_>>();
    let selected_set = selected_track_ids.iter().copied().collect::<BTreeSet<_>>();
    let required_selected_recall = (!required.is_empty())
        .then(|| required.intersection(&selected_set).count() as f64 / required.len() as f64);
    let positions = selected
        .iter()
        .filter_map(|candidate| {
            candidate
                .sequence_position
                .map(|position| (candidate.track_id.get(), position))
        })
        .collect::<BTreeMap<_, _>>();
    let order_pair_accuracy = (!case.expectations.order_pairs.is_empty()).then(|| {
        case.expectations
            .order_pairs
            .iter()
            .filter(|(before, after)| {
                positions
                    .get(before)
                    .zip(positions.get(after))
                    .is_some_and(|(before, after)| before < after)
            })
            .count() as f64
            / case.expectations.order_pairs.len() as f64
    });
    let top_candidates = response
        .candidates
        .iter()
        .take(case.expectations.top_k)
        .collect::<Vec<_>>();
    let reason_coverage = if top_candidates.is_empty() {
        0.0
    } else {
        top_candidates
            .iter()
            .filter(|candidate| !candidate.reasons.is_empty())
            .count() as f64
            / top_candidates.len() as f64
    };
    let track_by_id = case
        .fixture_tracks
        .iter()
        .map(|track| (track.id, track))
        .collect::<BTreeMap<_, _>>();
    let selected_duration_s = selected_track_ids
        .iter()
        .filter_map(|track_id| track_by_id.get(track_id))
        .map(|track| track.length_s)
        .sum::<f64>();
    let target_duration_s = f64::from(case.request.target_minutes) * 60.0;
    let duration_error_ratio = (selected_duration_s - target_duration_s).abs() / target_duration_s;
    let artist_keys = selected_track_ids
        .iter()
        .filter_map(|track_id| track_by_id.get(track_id).map(|track| (*track_id, track)))
        .map(|(track_id, track)| {
            let artist = track.artist.trim().to_lowercase();
            if artist.is_empty() {
                format!("track:{track_id}")
            } else {
                artist
            }
        })
        .collect::<BTreeSet<_>>();
    let artist_diversity = if selected_track_ids.is_empty() {
        0.0
    } else {
        artist_keys.len() as f64 / selected_track_ids.len() as f64
    };
    let known = track_by_id.keys().copied().collect::<BTreeSet<_>>();
    let forbidden = case
        .expectations
        .forbidden_track_ids
        .iter()
        .copied()
        .collect::<BTreeSet<_>>();
    let excluded = case
        .request
        .exclude_track_ids
        .iter()
        .map(|track_id| track_id.get())
        .collect::<BTreeSet<_>>();
    let unknown_candidate_count = candidate_ids
        .iter()
        .filter(|track_id| !known.contains(track_id))
        .count();
    let forbidden_candidate_count = candidate_ids
        .iter()
        .filter(|track_id| forbidden.contains(track_id))
        .count();
    let excluded_candidate_count = candidate_ids
        .iter()
        .filter(|track_id| excluded.contains(track_id))
        .count();
    let source_mismatch_count = response
        .candidates
        .iter()
        .filter(|candidate| {
            track_by_id
                .get(&candidate.track_id.get())
                .is_some_and(|source| candidate_source_mismatch(candidate, source))
        })
        .count();
    let selected_positions = selected
        .iter()
        .filter_map(|candidate| candidate.sequence_position)
        .collect::<Vec<_>>();
    let sequence_valid = selected_positions.len() == selected.len()
        && {
            let mut sorted = selected_positions.clone();
            sorted.sort_unstable();
            sorted == (1..=selected.len()).collect::<Vec<_>>()
        }
        && response.plan.selected_tracks == selected.len();
    let contract_valid = response.engine == engine_id
        && response.library_tracks == case.fixture_tracks.len()
        && candidate_ids.iter().copied().collect::<BTreeSet<_>>().len() == candidate_ids.len()
        && response.candidates.len() <= usize::from(case.request.candidate_limit)
        && response.eligible_tracks <= case.fixture_tracks.len()
        && response.candidates.len() <= response.eligible_tracks
        && response.plan.energy_curve == case.request.energy_curve
        && (response.plan.selected_duration_s - selected_duration_s).abs() < 0.001
        && unknown_candidate_count == 0
        && excluded_candidate_count == 0
        && source_mismatch_count == 0
        && sequence_valid;
    let metrics = PlaylistEvaluationMetrics {
        precision_at_k: round_to(precision, 4),
        recall_at_k: round_to(recall, 4),
        reciprocal_rank: round_to(reciprocal_rank, 4),
        required_selected_recall: required_selected_recall.map(|value| round_to(value, 4)),
        order_pair_accuracy: order_pair_accuracy.map(|value| round_to(value, 4)),
        reason_coverage: round_to(reason_coverage, 4),
        selected_artist_diversity: round_to(artist_diversity, 4),
        duration_error_ratio: round_to(duration_error_ratio, 4),
        forbidden_candidate_count,
        unknown_candidate_count,
        excluded_candidate_count,
        source_mismatch_count,
        deterministic: None,
        contract_valid,
    };
    let mut failures = Vec::new();
    let thresholds = &case.thresholds;
    if metrics.precision_at_k < thresholds.min_precision_at_k {
        failures.push("precision_at_k below threshold".to_owned());
    }
    if metrics.recall_at_k < thresholds.min_recall_at_k {
        failures.push("recall_at_k below threshold".to_owned());
    }
    if metrics.reciprocal_rank < thresholds.min_reciprocal_rank {
        failures.push("reciprocal_rank below threshold".to_owned());
    }
    if metrics
        .required_selected_recall
        .is_some_and(|value| value < thresholds.min_required_selected_recall)
    {
        failures.push("required_selected_recall below threshold".to_owned());
    }
    if metrics
        .order_pair_accuracy
        .is_some_and(|value| value < thresholds.min_order_pair_accuracy)
    {
        failures.push("order_pair_accuracy below threshold".to_owned());
    }
    if metrics.reason_coverage < thresholds.min_reason_coverage {
        failures.push("reason_coverage below threshold".to_owned());
    }
    if metrics.selected_artist_diversity < thresholds.min_selected_artist_diversity {
        failures.push("selected_artist_diversity below threshold".to_owned());
    }
    if metrics.duration_error_ratio > thresholds.max_duration_error_ratio {
        failures.push("duration_error_ratio above threshold".to_owned());
    }
    if metrics.forbidden_candidate_count > thresholds.max_forbidden_candidates {
        failures.push("forbidden candidate limit exceeded".to_owned());
    }
    if !metrics.contract_valid {
        failures.push("suggestion response violates the evaluation contract".to_owned());
    }
    ResponseAssessment {
        metrics,
        failures,
        top_track_ids,
        selected_track_ids,
        response_fingerprint: response_fingerprint(response),
    }
}

fn candidate_source_mismatch(candidate: &PlaylistCandidate, source: &EvaluationTrack) -> bool {
    candidate.path != source.path
        || candidate.title != source.title
        || candidate.display_title != source.display_title
        || candidate.artist != source.artist
        || candidate.album != source.album
        || candidate.origin != source.origin
        || candidate.genre != source.genre
        || (candidate.length_s - source.length_s).abs() > f64::EPSILON
        || candidate.bpm != source.bpm
        || candidate.manual_tags.iter().collect::<BTreeSet<_>>()
            != source.manual_tags.iter().collect::<BTreeSet<_>>()
}

fn engine_error_result(
    case: &PlaylistQualityCase,
    error: Option<ModelTaskError>,
) -> PlaylistEvaluationCaseResult {
    PlaylistEvaluationCaseResult {
        candidate_recall: None,
        id: case.id.clone(),
        description: case.description.clone(),
        passed: false,
        metrics: PlaylistEvaluationMetrics {
            precision_at_k: 0.0,
            recall_at_k: 0.0,
            reciprocal_rank: 0.0,
            required_selected_recall: None,
            order_pair_accuracy: None,
            reason_coverage: 0.0,
            selected_artist_diversity: 0.0,
            duration_error_ratio: 1.0,
            forbidden_candidate_count: 0,
            unknown_candidate_count: 0,
            excluded_candidate_count: 0,
            source_mismatch_count: 0,
            deterministic: None,
            contract_valid: false,
        },
        failures: vec![error.map_or_else(
            || "playlist model error: model_execution_failed".to_owned(),
            |error| format_task_failure("playlist model error", &error),
        )],
        top_track_ids: Vec::new(),
        selected_track_ids: Vec::new(),
        response_fingerprint: String::new(),
        repeated_top_track_ids: None,
        repeated_selected_track_ids: None,
        repeated_response_fingerprint: None,
        exact_response_match: None,
    }
}

fn response_fingerprint(response: &PlaylistSuggestion) -> String {
    let payload = serde_json::to_vec(&playlist_suggestion_payload(response)).unwrap_or_default();
    format!("{:x}", Sha256::digest(payload))
}

fn mean(values: impl Iterator<Item = f64>) -> Option<f64> {
    let values = values.collect::<Vec<_>>();
    (!values.is_empty()).then(|| round_to(values.iter().sum::<f64>() / values.len() as f64, 4))
}

fn mean_or_zero(values: impl Iterator<Item = f64>) -> f64 {
    mean(values).unwrap_or(0.0)
}

fn round_to(value: f64, places: i32) -> f64 {
    let factor = 10_f64.powi(places);
    (value * factor).round() / factor
}

fn axes_valid(energy: f64, brightness: f64, tension: f64) -> bool {
    [energy, brightness, tension]
        .iter()
        .all(|value| value.is_finite() && (0.0..=1.0).contains(value))
}

fn valid_case_id(value: &str) -> bool {
    (2..=64).contains(&value.len())
        && value
            .as_bytes()
            .first()
            .is_some_and(u8::is_ascii_alphanumeric)
        && value
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
}

fn format_track_ids(track_ids: &[i64]) -> String {
    format!(
        "[{}]",
        track_ids
            .iter()
            .map(i64::to_string)
            .collect::<Vec<_>>()
            .join(", ")
    )
}

fn format_task_failure(prefix: &str, error: &ModelTaskError) -> String {
    let mut failure = format!("{prefix}: {}", error.code);
    if let Some(diagnostic) = &error.diagnostic {
        failure.push_str(&format!(" ({diagnostic})"));
    }
    failure
}

fn default_signal_analyzer() -> String {
    "evaluation-signal/v1".to_owned()
}

const fn default_track_length() -> f64 {
    180.0
}

fn default_generated_artist() -> String {
    "Synthetic Scale Fixture".to_owned()
}

fn default_generated_genre() -> String {
    "ambient".to_owned()
}

const fn default_target_minutes() -> u16 {
    60
}

const fn default_candidate_limit() -> u16 {
    40
}

const fn default_true() -> bool {
    true
}

const fn default_top_k() -> usize {
    5
}

const fn default_one() -> f64 {
    1.0
}

fn default_thresholds() -> EvaluationThresholds {
    EvaluationThresholds {
        min_precision_at_k: 0.0,
        min_recall_at_k: 0.0,
        min_reciprocal_rank: 0.0,
        min_required_selected_recall: 0.0,
        min_order_pair_accuracy: 0.0,
        min_reason_coverage: 1.0,
        min_selected_artist_diversity: 0.0,
        max_duration_error_ratio: 1.0,
        max_forbidden_candidates: 0,
        require_deterministic: false,
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::{
        LOCAL_PLAYLIST_ENGINE_ID, PLAYLIST_QUALITY_SUITE_ID, evaluate_local_playlist_suite,
        load_playlist_quality_suite, playlist_quality_suite,
    };

    #[test]
    fn bundled_model_playlist_suite_includes_the_local_safety_baseline()
    -> Result<(), Box<dyn std::error::Error>> {
        let suite = playlist_quality_suite()?;
        assert_eq!(suite.id, PLAYLIST_QUALITY_SUITE_ID);
        assert_eq!(suite.cases.len(), 14);
        assert_eq!(
            suite
                .cases
                .iter()
                .filter(|case| case.requires_repeat())
                .count(),
            3
        );
        for case in &suite.cases {
            let task = case.task()?;
            assert!(task.request().is_some() || task.immediate_result().is_some());
        }
        Ok(())
    }

    #[test]
    fn external_suite_loader_resolves_only_the_declared_sibling_baseline()
    -> Result<(), Box<dyn std::error::Error>> {
        let path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("src/assistant/evaluation_suites/playlist-model-v1.json");
        let suite = load_playlist_quality_suite(&path)?;
        assert_eq!(suite.id, PLAYLIST_QUALITY_SUITE_ID);
        assert_eq!(suite.cases.len(), 14);
        let result = evaluate_local_playlist_suite(&suite)?;
        assert_eq!(result.engine_id, LOCAL_PLAYLIST_ENGINE_ID);
        assert_eq!(result.summary.cases, 14);
        Ok(())
    }

    #[test]
    fn candidate_recall_identifies_local_omissions_even_when_the_provider_fails()
    -> Result<(), Box<dyn std::error::Error>> {
        let raw = serde_json::from_value(serde_json::json!({
            "schema_version": "playlist-evaluation/v1", "id": "candidate-recall-test",
            "description": "A relevant track competes with many equally scored distractors.",
            "cases": [{"id": "pool-omission", "description": "Relevant candidate outside the local pool",
                "request": {"prompt": "background", "target_minutes": 5, "candidate_limit": 5},
                "tracks": [{"id": 1001, "path": "Fixture/Neutral.flac", "title": "Neutral",
                    "artist": "Fixture Ensemble", "genre": "instrumental", "length_s": 300}],
                "generated_tracks": {"count": 100, "id_start": 1, "path_prefix": "Distractors",
                    "title_prefix": "Neutral", "artist": "Fixture Ensemble", "genre": "instrumental", "length_s": 300},
                "expectations": {"top_k": 1, "relevant_track_ids": [1, 1001]}
            }]
        }))?;
        let suite = super::materialize_suite(raw)?;
        let case = &suite.cases[0];
        let task = case.task()?;
        let recall = case.candidate_recall(&task);
        assert_eq!(recall.pool_tracks, 15);
        assert_eq!(recall.relevant_in_pool, 1);
        assert_eq!(recall.recall, 0.5);
        assert_eq!(recall.missing_relevant_track_ids, vec![1001]);
        let result = case.assess(
            Err(super::ModelTaskError::new("model_execution_timeout")),
            None,
        );
        assert!(!result.passed);
        assert_eq!(
            result
                .candidate_recall
                .ok_or("recall missing on failure")?
                .recall,
            0.5
        );
        assert!(
            result
                .failures
                .iter()
                .any(|failure| failure.contains("timeout"))
        );
        let local = case.assess_for_engine(
            LOCAL_PLAYLIST_ENGINE_ID,
            Err(super::ModelTaskError::new("local_evaluation_failed")),
            None,
        );
        assert!(local.candidate_recall.is_none());
        Ok(())
    }

    #[test]
    fn candidate_diagnostics_cover_the_expanded_suite_without_certifying_a_model()
    -> Result<(), Box<dyn std::error::Error>> {
        let suite = playlist_quality_suite()?;
        let report = super::evaluate_playlist_candidates(&suite)?;
        assert_eq!(report.cases.len(), 14);
        for result in &report.cases {
            assert_eq!(result.candidate_recall.recall, 1.0, "{}", result.id);
            assert!(result.candidate_recall.pool_tracks <= 100);
        }
        let document = serde_json::to_value(report)?;
        assert!(document.get("passed").is_none());
        assert_eq!(
            document["schema_version"],
            "playlist-candidate-evaluation/v1"
        );
        for case in suite
            .cases
            .iter()
            .filter(|case| case.id.starts_with("large-pool-"))
        {
            let mut request = case.request.clone();
            request.candidate_limit = request.candidate_limit.saturating_mul(3).min(100);
            let original = super::suggest_local_playlist(&case.source, &request)?;
            assert!(
                original.candidates.iter().all(|candidate| !case
                    .expectations
                    .relevant_track_ids
                    .contains(&candidate.track_id.get())),
                "{} must reproduce the old omission",
                case.id
            );
            let task = case.task()?;
            let repeated = case.task()?;
            assert_eq!(
                task.request().ok_or("request")?.user_prompt,
                repeated.request().ok_or("request")?.user_prompt
            );
            let output = serde_json::json!({"schema_version": "assistant-playlist-planner-output/v1",
                "ranked_track_ids": case.expectations.relevant_track_ids,
                "selected_track_ids": case.expectations.required_default_track_ids});
            let result = task.finish(crate::assistant::structured_harness::tests::model_result(
                output,
            ))?;
            let assessed = case.assess(Ok(result), None);
            assert!(assessed.passed, "{}: {:?}", case.id, assessed.failures);
        }
        Ok(())
    }
}
