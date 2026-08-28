//! Deterministic Assistant planning and review workflows.
//!
//! Local evidence remains authoritative. Generated suggestions are immutable
//! inputs to an explicit review operation; they never mutate operator-owned
//! tags, playlists, or presets by themselves.

#[cfg(feature = "fuzzing")]
mod fuzzing;
mod local_analysis;
mod model_eq;
mod model_playlist;
mod model_quality;
mod model_tag_cleanup;
mod model_tagger;
mod planner;
mod playlist_evaluation;
mod provider_usage;
mod providers;
mod runtime_contract;
mod structured_harness;
mod tags;
mod vocabulary;

#[cfg(feature = "fuzzing")]
#[doc(hidden)]
pub use fuzzing::exercise_structured_model_outputs;
pub use local_analysis::{
    AUDIO_ANALYSIS_JOB_KIND, AnalysisFailureState, AnalysisFailureWrite, AnalysisState,
    AnalysisWrite, ContextScope, ContextState, ContextWrite, CurrentTrackContext,
    LIBRARY_CONTEXT_JOB_KIND, LOCAL_CONTEXT_ANALYZER_ID, LOCAL_CONTEXT_IMPLEMENTATION_ID,
    LibraryAnalysisSummary, LibraryContextPassSummary, LibraryContextSummary, LocalAnalysisError,
    LocalAnalysisRepository, LocalAnalysisService, METADATA_ANALYSIS_JOB_KIND,
    MetadataAnalysisJobHandler, ModelAnalysisWrite, TrackContextDetail, VOICE_ANALYZER_ID,
    VOICE_MODEL_FILENAME, VOICE_MODEL_SHA256, VoiceAnalyzerStatus, context_source_signature,
    parse_context_state,
};
pub use model_eq::*;
pub use model_playlist::*;
pub use model_quality::*;
pub use model_tag_cleanup::*;
pub use model_tagger::*;
pub use planner::{
    EnergyCurve, LOCAL_PLAYLIST_ENGINE_ID, PlaylistAudioSignal, PlaylistCandidate, PlaylistIntent,
    PlaylistPlan, PlaylistSuggestion, PlaylistSuggestionRequest, suggest_local_playlist,
};
pub use playlist_evaluation::*;
pub use provider_usage::*;
pub use providers::*;
pub use runtime_contract::{ASSISTANT_RUNTIME_CONTRACT_VERSION, assistant_runtime_contract_digest};
pub use structured_harness::ModelTaskError;
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
