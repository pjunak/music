use std::collections::{BTreeMap, BTreeSet};
use std::error::Error;
use std::fmt::{self, Display, Formatter};
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use music_domain::{IndexedTrack, TrackId};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use sha2::{Digest, Sha256};
use unicode_casefold::UnicodeCaseFold;
use unicode_general_category::{GeneralCategory, get_general_category};
use unicode_normalization::UnicodeNormalization;

use super::local_analysis::ContextScope;
use super::planner::{PlaylistSuggestion, PlaylistSuggestionRequest, suggest_local_playlist};
use super::vocabulary::{
    CleanupApplyOutcome, CleanupMutation, CleanupPreview, CleanupSelection, TagVocabularyDocument,
    TagVocabularyRecord, TagVocabularySnapshot, VocabularyError, build_cleanup_preview,
    default_vocabulary, vocabulary_fingerprint,
};

pub const MAX_TAGS_PER_TRACK: usize = 32;
pub const MAX_TAG_LENGTH: usize = 64;
pub const LOCAL_METADATA_ANALYZER_ID: &str = "local-metadata/v1";
pub const LOCAL_AUDIO_ANALYZER_ID: &str = "local-audio/v1";
pub const MODEL_TAG_ANALYZER_ID: &str = "model-context-tagger/v6";
pub const CATALOG_TAG_ANALYZER_ID: &str = "catalog-tags/v1";

pub type AssistantDependencyError = Box<dyn Error + Send + Sync>;
pub type AssistantFuture<'a, T> =
    Pin<Box<dyn Future<Output = Result<T, AssistantDependencyError>> + Send + 'a>>;

#[derive(Debug, Clone, Copy, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Confidence {
    High,
    Medium,
    Low,
}

impl Confidence {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::High => "high",
            Self::Medium => "medium",
            Self::Low => "low",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "high" => Some(Self::High),
            "medium" => Some(Self::Medium),
            "low" => Some(Self::Low),
            _ => None,
        }
    }

    #[must_use]
    pub const fn weight(self) -> f64 {
        match self {
            Self::High => 0.8,
            Self::Medium => 0.55,
            Self::Low => 0.3,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct StoredAnalysis {
    pub analyzer_id: String,
    pub source_signature: String,
    pub energy: f64,
    pub brightness: f64,
    pub tension: f64,
    pub moods: Vec<String>,
    pub evidence: Vec<String>,
    pub metrics: Map<String, Value>,
    pub confidence: String,
}

#[derive(Debug, Clone, Eq, PartialEq)]
pub struct StoredAnalysisReview {
    pub analyzer_id: String,
    pub source_signature: String,
    pub tag: String,
    pub decision: AnalysisReviewDecision,
}

#[derive(Debug, Clone, PartialEq)]
pub struct AssistantTrackEvidence {
    pub track: IndexedTrack,
    pub manual_tags: Vec<String>,
    pub analyses: Vec<StoredAnalysis>,
    pub reviews: Vec<StoredAnalysisReview>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct AudioSignalProfile {
    pub analyzer_id: String,
    pub energy: f64,
    pub brightness: f64,
    pub tension: f64,
    pub tempo_bpm: Option<f64>,
    pub confidence: Confidence,
    pub evidence: Vec<String>,
    pub metrics: Map<String, Value>,
}

#[derive(Debug, Clone, Eq, PartialEq)]
pub struct AnalysisSuggestion {
    pub tag: String,
    pub analyzer_id: String,
    pub source_signature: String,
    pub confidence: Confidence,
    pub evidence: Vec<String>,
    pub status: AnalysisReviewDecision,
}

#[derive(Debug, Clone, PartialEq)]
pub struct AssistantTrackView {
    pub track: IndexedTrack,
    pub manual_tags: Vec<String>,
    pub analysis_analyzer: Option<String>,
    pub analysis_tags: Vec<String>,
    pub analysis_confidence: Option<Confidence>,
    pub analysis_suggestions: Vec<AnalysisSuggestion>,
    pub audio_signal: Option<AudioSignalProfile>,
}

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum AnalysisReviewDecision {
    Pending,
    Accepted,
    Rejected,
}

impl AnalysisReviewDecision {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Accepted => "accepted",
            Self::Rejected => "rejected",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "pending" => Some(Self::Pending),
            "accepted" => Some(Self::Accepted),
            "rejected" => Some(Self::Rejected),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Eq, PartialEq, Ord, PartialOrd)]
pub struct AnalysisReviewTarget {
    pub track_id: TrackId,
    pub tag: String,
    pub analyzer_id: String,
    pub source_signature: String,
}

#[derive(Debug, Clone, Eq, PartialEq)]
pub struct AnalysisReviewOutcome {
    pub track_id: TrackId,
    pub tag: String,
    pub analyzer_id: String,
    pub source_signature: String,
    pub decision: AnalysisReviewDecision,
}

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum AnalysisReviewFailureCode {
    NotFound,
    Stale,
    TagLimit,
}

impl AnalysisReviewFailureCode {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::NotFound => "not_found",
            Self::Stale => "stale",
            Self::TagLimit => "tag_limit",
        }
    }
}

#[derive(Debug, Clone, Eq, PartialEq)]
pub struct AnalysisReviewFailure {
    pub target: AnalysisReviewTarget,
    pub code: AnalysisReviewFailureCode,
    pub error: String,
}

#[derive(Debug, Clone, Eq, PartialEq)]
pub struct AnalysisReviewBatch {
    pub requested_items: usize,
    pub applied: Vec<AnalysisReviewOutcome>,
    pub failures: Vec<AnalysisReviewFailure>,
}

#[derive(Debug, Clone, Eq, PartialEq)]
pub struct TagUsage {
    pub tag: String,
    pub track_count: u64,
}

#[derive(Debug, Clone, Eq, PartialEq)]
pub struct BulkTagFailure {
    pub track_id: TrackId,
    pub error: String,
}

#[derive(Debug, Clone, Eq, PartialEq)]
pub struct BulkTagOutcome {
    pub requested_tracks: usize,
    pub matched_tracks: usize,
    pub changed_track_ids: Vec<TrackId>,
    pub missing_track_ids: Vec<TrackId>,
    pub failures: Vec<BulkTagFailure>,
}

#[derive(Debug, Clone, Eq, PartialEq)]
pub struct RenameTagOutcome {
    pub source: String,
    pub target: String,
    pub affected_tracks: usize,
    pub merged: bool,
}

#[derive(Debug, Clone, Eq, PartialEq)]
pub struct ManualTagQuery {
    pub search: String,
    pub tag: Option<String>,
    pub review: Option<AnalysisReviewDecision>,
    pub offset: usize,
    pub limit: usize,
    pub analyzer_ids: Option<Vec<String>>,
    pub scope: Option<ContextScope>,
}

impl Default for ManualTagQuery {
    fn default() -> Self {
        Self {
            search: String::new(),
            tag: None,
            review: None,
            offset: 0,
            limit: 50,
            analyzer_ids: None,
            scope: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct TagPage {
    pub items: Vec<AssistantTrackView>,
    pub total: usize,
    pub offset: usize,
    pub limit: usize,
}

pub trait AssistantRepository: std::fmt::Debug + Send + Sync {
    fn tracks(&self) -> AssistantFuture<'_, Vec<AssistantTrackEvidence>>;
    fn vocabulary(&self) -> AssistantFuture<'_, Option<TagVocabularyRecord>>;
    fn initialize_vocabulary<'a>(
        &'a self,
        record: &'a TagVocabularyRecord,
    ) -> AssistantFuture<'a, TagVocabularyRecord>;
    fn replace_vocabulary<'a>(
        &'a self,
        expected_revision: u32,
        document: &'a TagVocabularyDocument,
    ) -> AssistantFuture<'a, Option<TagVocabularyRecord>>;
    fn patch_tags<'a>(
        &'a self,
        track_ids: &'a [TrackId],
        add: &'a [String],
        remove: &'a [String],
    ) -> AssistantFuture<'a, BulkTagOutcome>;
    fn rename_tag<'a>(
        &'a self,
        source: &'a str,
        target: &'a str,
    ) -> AssistantFuture<'a, Option<RenameTagOutcome>>;
    fn apply_cleanup<'a>(
        &'a self,
        expected_catalog_signature: &'a str,
        expected_vocabulary_fingerprint: &'a str,
        selections: &'a [CleanupSelection],
        allowed_pairs: Option<&'a [CleanupSelection]>,
    ) -> AssistantFuture<'a, CleanupMutation>;
    fn review_analysis<'a>(
        &'a self,
        targets: &'a [AnalysisReviewTarget],
        decision: AnalysisReviewDecision,
    ) -> AssistantFuture<'a, AnalysisReviewBatch>;
}

#[derive(Debug)]
pub enum AssistantServiceError {
    Validation(String),
    NotFound(String),
    Conflict(String),
    Stale(String),
    InvalidCleanupSelection(String),
    Vocabulary(VocabularyError),
    Dependency(AssistantDependencyError),
}

impl Display for AssistantServiceError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        match self {
            Self::Validation(message)
            | Self::NotFound(message)
            | Self::Conflict(message)
            | Self::Stale(message)
            | Self::InvalidCleanupSelection(message) => formatter.write_str(message),
            Self::Vocabulary(error) => Display::fmt(error, formatter),
            Self::Dependency(_) => formatter.write_str("Assistant storage operation failed"),
        }
    }
}

impl Error for AssistantServiceError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Vocabulary(error) => Some(error),
            Self::Dependency(error) => Some(error.as_ref()),
            _ => None,
        }
    }
}

impl From<VocabularyError> for AssistantServiceError {
    fn from(error: VocabularyError) -> Self {
        Self::Vocabulary(error)
    }
}

#[derive(Debug, Clone)]
pub struct AssistantService {
    repository: Arc<dyn AssistantRepository>,
}

impl AssistantService {
    #[must_use]
    pub fn new(repository: Arc<dyn AssistantRepository>) -> Self {
        Self { repository }
    }

    pub async fn vocabulary(&self) -> Result<TagVocabularySnapshot, AssistantServiceError> {
        let record = match self
            .repository
            .vocabulary()
            .await
            .map_err(AssistantServiceError::Dependency)?
        {
            Some(record) => record,
            None => {
                let document = default_vocabulary()?;
                self.repository
                    .initialize_vocabulary(&TagVocabularyRecord {
                        revision: 1,
                        seed_version: super::vocabulary::TAG_VOCABULARY_SEED_VERSION,
                        document,
                    })
                    .await
                    .map_err(AssistantServiceError::Dependency)?
            }
        };
        snapshot(record)
    }

    pub async fn replace_vocabulary(
        &self,
        expected_revision: u32,
        document: TagVocabularyDocument,
    ) -> Result<TagVocabularySnapshot, AssistantServiceError> {
        let document = document.normalized()?;
        let record = self
            .repository
            .replace_vocabulary(expected_revision, &document)
            .await
            .map_err(AssistantServiceError::Dependency)?
            .ok_or_else(|| {
                AssistantServiceError::Conflict(
                    "The tag vocabulary changed after this page was loaded. Reload it and try again."
                        .to_owned(),
                )
            })?;
        snapshot(record)
    }

    pub async fn tracks(&self) -> Result<Vec<AssistantTrackEvidence>, AssistantServiceError> {
        self.repository
            .tracks()
            .await
            .map_err(AssistantServiceError::Dependency)
    }

    pub async fn tag_usage(&self) -> Result<Vec<TagUsage>, AssistantServiceError> {
        let tracks = self.tracks().await?;
        Ok(tag_usage(&tracks))
    }

    pub async fn tag_page(
        &self,
        mut query: ManualTagQuery,
    ) -> Result<TagPage, AssistantServiceError> {
        if !(1..=100).contains(&query.limit) {
            return Err(AssistantServiceError::Validation(
                "limit must be between 1 and 100".to_owned(),
            ));
        }
        query.search = query.search.trim().to_owned();
        if query.search.chars().count() > 128 {
            return Err(AssistantServiceError::Validation(
                "search cannot exceed 128 characters".to_owned(),
            ));
        }
        query.tag = query
            .tag
            .map(|tag| normalize_manual_tag(&tag))
            .transpose()
            .map_err(AssistantServiceError::Validation)?;
        let mut views = self
            .tracks()
            .await?
            .into_iter()
            .filter(|track| {
                query
                    .scope
                    .as_ref()
                    .is_none_or(|scope| scope.contains(&track.track))
            })
            .map(|track| view_for_track(&track, query.analyzer_ids.as_deref()))
            .filter(|view| {
                query
                    .tag
                    .as_ref()
                    .is_none_or(|tag| view.manual_tags.contains(tag))
                    && (query.search.is_empty() || view_matches_search(view, &query.search))
                    && query.review.is_none_or(|status| {
                        view.analysis_suggestions
                            .iter()
                            .any(|suggestion| suggestion.status == status)
                    })
            })
            .collect::<Vec<_>>();
        views.sort_by(|left, right| {
            display_title(&left.track)
                .to_lowercase()
                .cmp(&display_title(&right.track).to_lowercase())
                .then_with(|| left.track.id.cmp(&right.track.id))
        });
        let total = views.len();
        let items = views
            .into_iter()
            .skip(query.offset)
            .take(query.limit)
            .collect();
        Ok(TagPage {
            items,
            total,
            offset: query.offset,
            limit: query.limit,
        })
    }

    pub async fn patch_tags(
        &self,
        track_ids: &[TrackId],
        add: &[String],
        remove: &[String],
    ) -> Result<BulkTagOutcome, AssistantServiceError> {
        if track_ids.is_empty() || track_ids.len() > 5_000 {
            return Err(AssistantServiceError::Validation(
                "track_ids must contain between 1 and 5000 tracks".to_owned(),
            ));
        }
        let add = normalize_manual_tags(add).map_err(AssistantServiceError::Validation)?;
        let remove = normalize_manual_tags(remove).map_err(AssistantServiceError::Validation)?;
        if let Some(overlap) = add.iter().find(|tag| remove.contains(tag)) {
            return Err(AssistantServiceError::Validation(format!(
                "tags cannot be added and removed together: {overlap}"
            )));
        }
        self.repository
            .patch_tags(track_ids, &add, &remove)
            .await
            .map_err(AssistantServiceError::Dependency)
    }

    pub async fn patch_track(
        &self,
        track_id: TrackId,
        add: &[String],
        remove: &[String],
    ) -> Result<AssistantTrackView, AssistantServiceError> {
        let outcome = self.patch_tags(&[track_id], add, remove).await?;
        if outcome.matched_tracks == 0 {
            return Err(AssistantServiceError::NotFound(
                "Track not found".to_owned(),
            ));
        }
        if let Some(failure) = outcome.failures.first() {
            return Err(AssistantServiceError::Validation(failure.error.clone()));
        }
        self.tracks()
            .await?
            .iter()
            .find(|item| item.track.id == track_id)
            .map(|item| view_for_track(item, None))
            .ok_or_else(|| AssistantServiceError::NotFound("Track not found".to_owned()))
    }

    pub async fn rename_tag(
        &self,
        source: &str,
        target: &str,
    ) -> Result<RenameTagOutcome, AssistantServiceError> {
        let source = normalize_manual_tag(source).map_err(AssistantServiceError::Validation)?;
        let target = normalize_manual_tag(target).map_err(AssistantServiceError::Validation)?;
        if source == target {
            return Err(AssistantServiceError::Validation(
                "source and target tags must be different".to_owned(),
            ));
        }
        self.repository
            .rename_tag(&source, &target)
            .await
            .map_err(AssistantServiceError::Dependency)?
            .ok_or_else(|| {
                AssistantServiceError::NotFound(format!("manual tag not found: {source}"))
            })
    }

    pub async fn cleanup_preview(&self) -> Result<CleanupPreview, AssistantServiceError> {
        let tracks = self.tracks().await?;
        let vocabulary = self.vocabulary().await?;
        build_cleanup_preview(&tag_usage(&tracks), &vocabulary).map_err(Into::into)
    }

    pub async fn apply_cleanup(
        &self,
        expected_catalog_signature: &str,
        expected_vocabulary_fingerprint: &str,
        selections: &[CleanupSelection],
    ) -> Result<CleanupApplyOutcome, AssistantServiceError> {
        self.apply_cleanup_internal(
            expected_catalog_signature,
            expected_vocabulary_fingerprint,
            selections,
            None,
        )
        .await
    }

    pub async fn apply_reviewed_cleanup(
        &self,
        expected_catalog_signature: &str,
        expected_vocabulary_fingerprint: &str,
        selections: &[CleanupSelection],
        allowed_pairs: &[CleanupSelection],
    ) -> Result<CleanupApplyOutcome, AssistantServiceError> {
        if allowed_pairs.is_empty() || allowed_pairs.len() > 100 {
            return Err(AssistantServiceError::InvalidCleanupSelection(
                "stored cleanup proposal is invalid".to_owned(),
            ));
        }
        self.apply_cleanup_internal(
            expected_catalog_signature,
            expected_vocabulary_fingerprint,
            selections,
            Some(allowed_pairs),
        )
        .await
    }

    async fn apply_cleanup_internal(
        &self,
        expected_catalog_signature: &str,
        expected_vocabulary_fingerprint: &str,
        selections: &[CleanupSelection],
        allowed_pairs: Option<&[CleanupSelection]>,
    ) -> Result<CleanupApplyOutcome, AssistantServiceError> {
        if selections.is_empty() || selections.len() > 100 {
            return Err(AssistantServiceError::Validation(
                "cleanup must contain between 1 and 100 selections".to_owned(),
            ));
        }
        let mut normalized = Vec::new();
        let mut sources = BTreeSet::new();
        let mut targets = BTreeSet::new();
        for selection in selections {
            let source = normalize_manual_tag(&selection.source)
                .map_err(AssistantServiceError::Validation)?;
            let target = normalize_manual_tag(&selection.target)
                .map_err(AssistantServiceError::Validation)?;
            if source == target || !sources.insert(source.clone()) {
                return Err(AssistantServiceError::InvalidCleanupSelection(
                    "cleanup sources must be unique and different from their targets".to_owned(),
                ));
            }
            targets.insert(target.clone());
            normalized.push(CleanupSelection { source, target });
        }
        if !sources.is_disjoint(&targets) {
            return Err(AssistantServiceError::InvalidCleanupSelection(
                "cleanup selections cannot depend on another selected rename".to_owned(),
            ));
        }
        match self
            .repository
            .apply_cleanup(
                expected_catalog_signature,
                expected_vocabulary_fingerprint,
                &normalized,
                allowed_pairs,
            )
            .await
            .map_err(AssistantServiceError::Dependency)?
        {
            CleanupMutation::Applied(outcome) => Ok(outcome),
            CleanupMutation::StaleCatalog => Err(AssistantServiceError::Stale(
                "database mood tags changed after this cleanup preview was created".to_owned(),
            )),
            CleanupMutation::StaleVocabulary => Err(AssistantServiceError::Stale(
                "tag vocabulary changed after this cleanup preview was created".to_owned(),
            )),
            CleanupMutation::InvalidSelection => {
                Err(AssistantServiceError::InvalidCleanupSelection(
                    "cleanup suggestion is not current".to_owned(),
                ))
            }
        }
    }

    pub async fn review_analysis(
        &self,
        targets: &[AnalysisReviewTarget],
        decision: AnalysisReviewDecision,
    ) -> Result<AnalysisReviewBatch, AssistantServiceError> {
        if targets.is_empty() || targets.len() > 1_000 {
            return Err(AssistantServiceError::Validation(
                "review must contain between 1 and 1000 suggestions".to_owned(),
            ));
        }
        let mut canonical = BTreeSet::new();
        for target in targets {
            if target.analyzer_id.is_empty()
                || target.analyzer_id.len() > 128
                || target.source_signature.is_empty()
                || target.source_signature.len() > 128
            {
                return Err(AssistantServiceError::Validation(
                    "analysis review identifiers are invalid".to_owned(),
                ));
            }
            canonical.insert(AnalysisReviewTarget {
                track_id: target.track_id,
                tag: normalize_manual_tag(&target.tag)
                    .map_err(AssistantServiceError::Validation)?,
                analyzer_id: target.analyzer_id.clone(),
                source_signature: target.source_signature.clone(),
            });
        }
        self.repository
            .review_analysis(&canonical.into_iter().collect::<Vec<_>>(), decision)
            .await
            .map_err(AssistantServiceError::Dependency)
    }

    pub async fn review_one(
        &self,
        target: AnalysisReviewTarget,
        decision: AnalysisReviewDecision,
    ) -> Result<(AnalysisReviewOutcome, Vec<String>), AssistantServiceError> {
        let result = self.review_analysis(&[target], decision).await?;
        if let Some(failure) = result.failures.first() {
            return Err(match failure.code {
                AnalysisReviewFailureCode::NotFound => {
                    AssistantServiceError::NotFound(failure.error.clone())
                }
                AnalysisReviewFailureCode::Stale => {
                    AssistantServiceError::Stale(failure.error.clone())
                }
                AnalysisReviewFailureCode::TagLimit => {
                    AssistantServiceError::Validation(failure.error.clone())
                }
            });
        }
        let outcome = result.applied.into_iter().next().ok_or_else(|| {
            AssistantServiceError::Conflict("analysis review was not applied".to_owned())
        })?;
        let manual_tags = self
            .tracks()
            .await?
            .into_iter()
            .find(|track| track.track.id == outcome.track_id)
            .map(|track| track.manual_tags)
            .ok_or_else(|| AssistantServiceError::NotFound("Track not found".to_owned()))?;
        Ok((outcome, manual_tags))
    }

    pub async fn suggest_playlist(
        &self,
        request: &PlaylistSuggestionRequest,
    ) -> Result<PlaylistSuggestion, AssistantServiceError> {
        let tracks = self.tracks().await?;
        suggest_local_playlist(&tracks, request).map_err(AssistantServiceError::Validation)
    }
}

fn snapshot(record: TagVocabularyRecord) -> Result<TagVocabularySnapshot, AssistantServiceError> {
    if record.revision == 0 {
        return Err(AssistantServiceError::Validation(
            "stored tag vocabulary revision is invalid".to_owned(),
        ));
    }
    let document = record.document.normalized()?;
    let fingerprint = vocabulary_fingerprint(&document)?;
    Ok(TagVocabularySnapshot {
        revision: record.revision,
        fingerprint,
        document,
    })
}

pub fn normalize_manual_tag(value: &str) -> Result<String, String> {
    let compatibility_normalized = value.nfkc().collect::<String>();
    let normalized = compatibility_normalized
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .case_fold()
        .collect::<String>();
    if normalized.is_empty() {
        return Err("tags cannot be blank".to_owned());
    }
    if normalized.chars().count() > MAX_TAG_LENGTH {
        return Err(format!("tags cannot exceed {MAX_TAG_LENGTH} characters"));
    }
    if normalized.chars().any(is_unicode_other) {
        return Err("tags cannot contain control characters".to_owned());
    }
    Ok(normalized)
}

pub fn normalize_manual_tags(values: &[String]) -> Result<Vec<String>, String> {
    let mut seen = BTreeSet::new();
    let mut normalized = Vec::new();
    for value in values {
        let value = normalize_manual_tag(value)?;
        if seen.insert(value.clone()) {
            normalized.push(value);
        }
    }
    if normalized.len() > MAX_TAGS_PER_TRACK {
        return Err(format!(
            "a track cannot have more than {MAX_TAGS_PER_TRACK} manual tags"
        ));
    }
    Ok(normalized)
}

fn is_unicode_other(character: char) -> bool {
    matches!(
        get_general_category(character),
        GeneralCategory::Control
            | GeneralCategory::Format
            | GeneralCategory::PrivateUse
            | GeneralCategory::Surrogate
            | GeneralCategory::Unassigned
    )
}

pub fn metadata_source_signature(track: &IndexedTrack) -> Result<String, String> {
    source_signature(&serde_json::json!([
        track.path.as_str(),
        track.metadata.title,
        track.display_title,
        track.metadata.artist,
        track.metadata.album,
        track.origin,
        track.metadata.genre,
        track.metadata.bpm,
    ]))
}

pub fn audio_source_signature(track: &IndexedTrack) -> Result<String, String> {
    source_signature(&serde_json::json!([
        track.path.as_str(),
        track.size_bytes,
        track.mtime_unix_seconds,
    ]))
}

pub fn catalog_tag_source_signature(
    track: &IndexedTrack,
    evidence_revision: i64,
) -> Result<String, String> {
    source_signature(&serde_json::json!([
        CATALOG_TAG_ANALYZER_ID,
        metadata_source_signature(track)?,
        evidence_revision,
    ]))
}

fn source_signature(value: &Value) -> Result<String, String> {
    serde_json::to_vec(value)
        .map(|encoded| format!("{:x}", Sha256::digest(encoded)))
        .map_err(|_| "track source signature could not be encoded".to_owned())
}

pub(super) fn current_metadata_analysis(
    track: &AssistantTrackEvidence,
) -> Option<(&StoredAnalysis, Confidence)> {
    let signature = metadata_source_signature(&track.track).ok()?;
    track
        .analyses
        .iter()
        .find(|analysis| {
            analysis.analyzer_id == LOCAL_METADATA_ANALYZER_ID
                && analysis.source_signature == signature
                && axes_valid(analysis)
        })
        .and_then(|analysis| {
            Confidence::parse(&analysis.confidence).map(|confidence| (analysis, confidence))
        })
}

pub(super) fn current_audio_analysis(track: &AssistantTrackEvidence) -> Option<AudioSignalProfile> {
    let signature = audio_source_signature(&track.track).ok()?;
    let analysis = track.analyses.iter().find(|analysis| {
        analysis.analyzer_id == LOCAL_AUDIO_ANALYZER_ID
            && analysis.source_signature == signature
            && axes_valid(analysis)
    })?;
    if analysis.metrics.get("schema").and_then(Value::as_str) != Some(LOCAL_AUDIO_ANALYZER_ID)
        || analysis
            .metrics
            .values()
            .any(|value| !matches!(value, Value::Null | Value::String(_) | Value::Number(_)))
    {
        return None;
    }
    let tempo_bpm = analysis
        .metrics
        .get("tempo_bpm")
        .and_then(Value::as_f64)
        .filter(|tempo| tempo.is_finite() && *tempo > 0.0 && *tempo <= 999.0);
    Some(AudioSignalProfile {
        analyzer_id: analysis.analyzer_id.clone(),
        energy: analysis.energy,
        brightness: analysis.brightness,
        tension: analysis.tension,
        tempo_bpm,
        confidence: Confidence::parse(&analysis.confidence)?,
        evidence: analysis.evidence.clone(),
        metrics: analysis.metrics.clone(),
    })
}

fn axes_valid(analysis: &StoredAnalysis) -> bool {
    [analysis.energy, analysis.brightness, analysis.tension]
        .iter()
        .all(|value| value.is_finite() && (0.0..=1.0).contains(value))
}

pub(super) fn view_for_track(
    track: &AssistantTrackEvidence,
    analyzer_ids: Option<&[String]>,
) -> AssistantTrackView {
    let current = current_metadata_analysis(track);
    let mut suggestions = Vec::new();
    for analysis in &track.analyses {
        let expected_signature = if analysis.analyzer_id == CATALOG_TAG_ANALYZER_ID {
            analysis
                .metrics
                .get("evidence_revision")
                .and_then(Value::as_i64)
                .and_then(|revision| catalog_tag_source_signature(&track.track, revision).ok())
        } else {
            metadata_source_signature(&track.track).ok()
        };
        if !matches!(
            analysis.analyzer_id.as_str(),
            LOCAL_METADATA_ANALYZER_ID | CATALOG_TAG_ANALYZER_ID
        ) || expected_signature.as_deref() != Some(analysis.source_signature.as_str())
            || !axes_valid(analysis)
            || analyzer_ids.is_some_and(|ids| !ids.iter().any(|id| id == &analysis.analyzer_id))
        {
            continue;
        }
        let Some(confidence) = Confidence::parse(&analysis.confidence) else {
            continue;
        };
        let mut seen = BTreeSet::new();
        for raw_tag in &analysis.moods {
            let Ok(tag) = normalize_manual_tag(raw_tag) else {
                continue;
            };
            if !seen.insert(tag.clone()) {
                continue;
            }
            let status = track
                .reviews
                .iter()
                .find(|review| {
                    review.analyzer_id == analysis.analyzer_id
                        && review.source_signature == analysis.source_signature
                        && review.tag == tag
                })
                .map_or(AnalysisReviewDecision::Pending, |review| review.decision);
            suggestions.push(AnalysisSuggestion {
                tag,
                analyzer_id: analysis.analyzer_id.clone(),
                source_signature: analysis.source_signature.clone(),
                confidence,
                evidence: analysis.evidence.clone(),
                status,
            });
        }
    }
    let analysis_tags = suggestions
        .iter()
        .filter(|suggestion| suggestion.status != AnalysisReviewDecision::Rejected)
        .map(|suggestion| suggestion.tag.clone())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();
    let mut manual_tags = track.manual_tags.clone();
    manual_tags.sort();
    AssistantTrackView {
        track: track.track.clone(),
        manual_tags,
        analysis_analyzer: current.map(|(analysis, _)| analysis.analyzer_id.clone()),
        analysis_tags,
        analysis_confidence: current.map(|(_, confidence)| confidence),
        analysis_suggestions: suggestions,
        audio_signal: current_audio_analysis(track),
    }
}

fn tag_usage(tracks: &[AssistantTrackEvidence]) -> Vec<TagUsage> {
    let mut usage = BTreeMap::<String, u64>::new();
    for track in tracks {
        for tag in &track.manual_tags {
            *usage.entry(tag.clone()).or_default() += 1;
        }
    }
    usage
        .into_iter()
        .map(|(tag, track_count)| TagUsage { tag, track_count })
        .collect()
}

fn display_title(track: &IndexedTrack) -> &str {
    if track.display_title.trim().is_empty() {
        &track.metadata.title
    } else {
        &track.display_title
    }
}

fn view_matches_search(view: &AssistantTrackView, search: &str) -> bool {
    let needle = search.to_lowercase();
    [
        view.track.display_title.as_str(),
        view.track.metadata.title.as_str(),
        view.track.metadata.artist.as_str(),
        view.track.metadata.album.as_str(),
        view.track.path.as_str(),
    ]
    .iter()
    .any(|value| value.to_lowercase().contains(&needle))
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use music_domain::{LibraryPath, TrackMetadata};

    use super::*;

    fn track() -> Result<IndexedTrack, Box<dyn Error>> {
        Ok(IndexedTrack {
            id: TrackId::new(1)?,
            path: LibraryPath::parse("Album/Track.mp3")?,
            metadata: TrackMetadata {
                title: "Track".to_owned(),
                artist: "Artist".to_owned(),
                album_artist: String::new(),
                album: "Album".to_owned(),
                track_no: None,
                disc_no: None,
                year: None,
                genre: "Ambient".to_owned(),
                bpm: Some(80),
            },
            duration: Duration::from_secs(120),
            display_title: "Track".to_owned(),
            origin: String::new(),
            size_bytes: 10,
            mtime_unix_seconds: 20,
            added_at_unix_seconds: 30,
        })
    }

    #[test]
    fn manual_tag_normalization_matches_the_legacy_contract() {
        assert_eq!(
            normalize_manual_tag("  CÁLM\tRoom "),
            Ok("cálm room".to_owned())
        );
        assert!(normalize_manual_tag("\u{0000}").is_err());
    }

    #[test]
    fn stale_profiles_are_not_exposed_as_current_suggestions() -> Result<(), Box<dyn Error>> {
        let evidence = AssistantTrackEvidence {
            track: track()?,
            manual_tags: Vec::new(),
            analyses: vec![StoredAnalysis {
                analyzer_id: LOCAL_METADATA_ANALYZER_ID.to_owned(),
                source_signature: "stale".to_owned(),
                energy: 0.2,
                brightness: 0.5,
                tension: 0.1,
                moods: vec!["calm".to_owned()],
                evidence: vec!["metadata".to_owned()],
                metrics: Map::new(),
                confidence: "high".to_owned(),
            }],
            reviews: Vec::new(),
        };
        let view = view_for_track(&evidence, None);
        assert!(view.analysis_suggestions.is_empty());
        Ok(())
    }
}
