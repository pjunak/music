//! Deterministic Assistant planning and review workflows.
//!
//! Local evidence remains authoritative. Generated suggestions are immutable
//! inputs to an explicit review operation; they never mutate operator-owned
//! tags, playlists, or presets by themselves.

mod local_analysis;
mod model_quality;
mod planner;
mod providers;
mod tags;
mod vocabulary;

pub use local_analysis::{
    AUDIO_ANALYSIS_JOB_KIND, AnalysisFailureState, AnalysisFailureWrite, AnalysisState,
    AnalysisWrite, ContextScope, ContextState, ContextWrite, CurrentTrackContext,
    LIBRARY_CONTEXT_JOB_KIND, LOCAL_CONTEXT_ANALYZER_ID, LOCAL_CONTEXT_IMPLEMENTATION_ID,
    LibraryAnalysisSummary, LibraryContextPassSummary, LibraryContextSummary, LocalAnalysisError,
    LocalAnalysisRepository, LocalAnalysisService, METADATA_ANALYSIS_JOB_KIND,
    MetadataAnalysisJobHandler, TrackContextDetail, VOICE_ANALYZER_ID, VOICE_MODEL_FILENAME,
    VOICE_MODEL_SHA256, VoiceAnalyzerStatus, context_source_signature, parse_context_state,
};
pub use model_quality::*;
pub use planner::{
    EnergyCurve, PlaylistAudioSignal, PlaylistCandidate, PlaylistIntent, PlaylistPlan,
    PlaylistSuggestion, PlaylistSuggestionRequest, suggest_local_playlist,
};
pub use providers::*;
pub use tags::{
    AnalysisReviewBatch, AnalysisReviewDecision, AnalysisReviewFailure, AnalysisReviewFailureCode,
    AnalysisReviewOutcome, AnalysisReviewTarget, AnalysisSuggestion, AssistantDependencyError,
    AssistantFuture, AssistantRepository, AssistantService, AssistantServiceError,
    AssistantTrackEvidence, AssistantTrackView, AudioSignalProfile, BulkTagFailure, BulkTagOutcome,
    Confidence, LOCAL_AUDIO_ANALYZER_ID, LOCAL_METADATA_ANALYZER_ID, MAX_TAGS_PER_TRACK,
    MODEL_TAG_ANALYZER_ID, ManualTagQuery, RenameTagOutcome, StoredAnalysis, StoredAnalysisReview,
    TagPage, TagUsage, audio_source_signature, metadata_source_signature, normalize_manual_tag,
    normalize_manual_tags,
};
pub use vocabulary::{
    CleanupApplyOutcome, CleanupMutation, CleanupPreview, CleanupSelection, CleanupSuggestion,
    CleanupSuggestionReason, TAG_CLEANUP_APPLY_SCHEMA, TAG_CLEANUP_PREVIEW_SCHEMA,
    TAG_VOCABULARY_SCHEMA, TagVocabularyDocument, TagVocabularyEntry, TagVocabularyGroup,
    TagVocabularyRecord, TagVocabularySnapshot, VocabularyError, build_cleanup_preview,
    catalog_signature, default_vocabulary, vocabulary_fingerprint,
};
