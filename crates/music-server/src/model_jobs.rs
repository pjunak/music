//! Server composition re-exports; workflow orchestration belongs to the application.
pub(crate) use music_application::assistant::{
    MODEL_EQ_DRAFT_JOB_KIND, MODEL_PLAYLIST_SUGGESTION_JOB_KIND, MODEL_TAG_CLEANUP_JOB_KIND,
    MODEL_TAGGING_JOB_KIND, model_evaluation_job_handlers, model_feature_job_handlers,
};
