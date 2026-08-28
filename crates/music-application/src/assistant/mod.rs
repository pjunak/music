//! Deterministic Assistant planning and review workflows.
//!
//! Local evidence remains authoritative. Generated suggestions are immutable
//! inputs to an explicit review operation; they never mutate operator-owned
//! tags, playlists, or presets by themselves.

mod planner;
mod tags;
mod vocabulary;

pub use planner::{
    EnergyCurve, PlaylistAudioSignal, PlaylistCandidate, PlaylistIntent, PlaylistPlan,
    PlaylistSuggestion, PlaylistSuggestionRequest, suggest_local_playlist,
};
pub use tags::{
    AnalysisReviewBatch, AnalysisReviewDecision, AnalysisReviewFailure, AnalysisReviewFailureCode,
    AnalysisReviewOutcome, AnalysisReviewTarget, AnalysisSuggestion, AssistantDependencyError,
    AssistantFuture, AssistantRepository, AssistantService, AssistantServiceError,
    AssistantTrackEvidence, AssistantTrackView, AudioSignalProfile, BulkTagFailure, BulkTagOutcome,
    Confidence, LOCAL_AUDIO_ANALYZER_ID, LOCAL_METADATA_ANALYZER_ID, MAX_TAGS_PER_TRACK,
    ManualTagQuery, RenameTagOutcome, StoredAnalysis, StoredAnalysisReview, TagPage, TagUsage,
    audio_source_signature, metadata_source_signature, normalize_manual_tag, normalize_manual_tags,
};
pub use vocabulary::{
    CleanupApplyOutcome, CleanupMutation, CleanupPreview, CleanupSelection, CleanupSuggestion,
    CleanupSuggestionReason, TAG_CLEANUP_APPLY_SCHEMA, TAG_CLEANUP_PREVIEW_SCHEMA,
    TAG_VOCABULARY_SCHEMA, TagVocabularyDocument, TagVocabularyEntry, TagVocabularyGroup,
    TagVocabularyRecord, TagVocabularySnapshot, VocabularyError, build_cleanup_preview,
    catalog_signature, default_vocabulary, vocabulary_fingerprint,
};
