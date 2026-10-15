//! HTTP routes for the Assistant: library context, playlist and EQ
//! suggestions, tag vocabulary and review, and model-assisted tagging and
//! tag cleanup. Each feature lives in its own submodule.

use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;

use crate::http::HttpState;

mod context;
mod model_tag_cleanup;
mod model_tagging;
mod review;
mod schema;
mod shared;
mod suggestions;
mod tags;

pub(crate) use shared::ModelTaggingScopeWire;

pub(crate) fn assistant_router() -> OpenApiRouter<HttpState> {
    OpenApiRouter::default()
        .routes(routes!(context::start_library_context_analysis))
        .routes(routes!(context::library_context_summary))
        .routes(routes!(context::library_track_context))
        .routes(routes!(suggestions::suggest_playlist))
        .routes(routes!(suggestions::model_playlist_status))
        .routes(routes!(suggestions::start_model_playlist_suggestion))
        .routes(routes!(suggestions::model_eq_status))
        .routes(routes!(suggestions::start_model_eq_draft))
        .routes(routes!(tags::tag_catalog))
        .routes(routes!(tags::get_tag_vocabulary))
        .routes(routes!(tags::update_tag_vocabulary))
        .routes(routes!(model_tag_cleanup::model_tag_cleanup_status))
        .routes(routes!(model_tag_cleanup::start_model_tag_cleanup))
        .routes(routes!(model_tag_cleanup::apply_model_tag_cleanup))
        .routes(routes!(tags::preview_tag_cleanup))
        .routes(routes!(tags::apply_tag_cleanup))
        .routes(routes!(model_tagging::model_tagging_status))
        .routes(routes!(model_tagging::plan_model_tagging))
        .routes(routes!(model_tagging::start_model_tagging))
        .routes(routes!(model_tagging::model_batch_status))
        .routes(routes!(model_tagging::model_batch_action))
        .routes(routes!(tags::rename_tag))
        .routes(routes!(tags::patch_tags_bulk))
        .routes(routes!(review::review_analysis_tags_bulk))
        .routes(routes!(model_tagging::query_model_library_tags))
        .routes(routes!(review::list_library_tags))
        .routes(routes!(tags::patch_track_tags))
        .routes(routes!(review::review_analysis_tag))
}
