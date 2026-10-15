//! OpenAPI schema builders shared by the assistant wire types.

use serde_json::json;
use utoipa::openapi::RefOr;
use utoipa::openapi::schema::{
    AdditionalProperties, AnyOfBuilder, ArrayBuilder, ObjectBuilder, Schema, Type,
};

use crate::error::{openapi_integer, openapi_number};

use super::tags::{TAG_CLEANUP_APPLY_SCHEMA, TAG_CLEANUP_PREVIEW_SCHEMA, TAG_VOCABULARY_SCHEMA};

pub(super) fn string_schema() -> RefOr<Schema> {
    ObjectBuilder::new().schema_type(Type::String).into()
}

pub(super) fn nullable_string_schema() -> RefOr<Schema> {
    Schema::AnyOf(
        AnyOfBuilder::new()
            .item(string_schema())
            .item(ObjectBuilder::new().schema_type(Type::Null))
            .build(),
    )
    .into()
}

pub(super) fn nullable_integer_schema() -> RefOr<Schema> {
    Schema::AnyOf(
        AnyOfBuilder::new()
            .item(openapi_integer())
            .item(ObjectBuilder::new().schema_type(Type::Null))
            .build(),
    )
    .into()
}

pub(super) fn nullable_positive_integer_schema() -> RefOr<Schema> {
    Schema::AnyOf(
        AnyOfBuilder::new()
            .item(
                ObjectBuilder::new()
                    .schema_type(Type::Integer)
                    .minimum(Some(1)),
            )
            .item(ObjectBuilder::new().schema_type(Type::Null))
            .build(),
    )
    .into()
}

pub(super) fn nullable_bpm_schema() -> RefOr<Schema> {
    Schema::AnyOf(
        AnyOfBuilder::new()
            .item(
                ObjectBuilder::new()
                    .schema_type(Type::Integer)
                    .minimum(Some(1))
                    .maximum(Some(999)),
            )
            .item(ObjectBuilder::new().schema_type(Type::Null))
            .build(),
    )
    .into()
}

pub(super) fn excluded_track_ids_schema() -> RefOr<Schema> {
    ArrayBuilder::new()
        .items(openapi_integer())
        .max_items(Some(5_000))
        .into()
}

pub(super) fn required_track_ids_schema() -> RefOr<Schema> {
    ArrayBuilder::new()
        .items(openapi_integer())
        .min_items(Some(1))
        .max_items(Some(5_000))
        .into()
}

pub(super) fn integer_array_schema() -> RefOr<Schema> {
    ArrayBuilder::new().items(openapi_integer()).into()
}

pub(super) fn target_minutes_schema() -> RefOr<Schema> {
    ObjectBuilder::new()
        .schema_type(Type::Integer)
        .minimum(Some(5))
        .maximum(Some(600))
        .default(Some(json!(60)))
        .into()
}

pub(super) fn candidate_limit_schema() -> RefOr<Schema> {
    ObjectBuilder::new()
        .schema_type(Type::Integer)
        .minimum(Some(5))
        .maximum(Some(100))
        .default(Some(json!(40)))
        .into()
}

pub(super) fn positive_integer_schema() -> RefOr<Schema> {
    ObjectBuilder::new()
        .schema_type(Type::Integer)
        .minimum(Some(1))
        .into()
}

pub(super) fn positive_exclusive_integer_schema() -> RefOr<Schema> {
    ObjectBuilder::new()
        .schema_type(Type::Integer)
        .exclusive_minimum(Some(0))
        .into()
}

pub(super) fn nonnegative_integer_schema() -> RefOr<Schema> {
    ObjectBuilder::new()
        .schema_type(Type::Integer)
        .minimum(Some(0))
        .into()
}

pub(super) fn nonnegative_number_schema() -> RefOr<Schema> {
    ObjectBuilder::new()
        .schema_type(Type::Number)
        .minimum(Some(0))
        .into()
}

pub(super) fn unit_number_schema() -> RefOr<Schema> {
    ObjectBuilder::new()
        .schema_type(Type::Number)
        .minimum(Some(0))
        .maximum(Some(1))
        .into()
}

pub(super) fn model_tag_filter_schema() -> RefOr<Schema> {
    ObjectBuilder::new()
        .schema_type(Type::String)
        .enum_values(Some(["processed", "current", "stale", "missing"]))
        .into()
}

pub(super) fn suggestion_source_schema() -> RefOr<Schema> {
    ObjectBuilder::new()
        .schema_type(Type::String)
        .enum_values(Some(["model", "metadata", "catalog"]))
        .into()
}

pub(super) fn model_analysis_status_schema() -> RefOr<Schema> {
    ObjectBuilder::new()
        .schema_type(Type::Object)
        .additional_properties(Some(AdditionalProperties::FreeForm(false)))
        .property(
            "status",
            ObjectBuilder::new()
                .schema_type(Type::String)
                .enum_values(Some(["current", "stale", "missing"])),
        )
        .property("job_id", nullable_string_schema())
        .property(
            "suggested_tag_count",
            AnyOfBuilder::new()
                .item(openapi_integer())
                .item(ObjectBuilder::new().schema_type(Type::Null)),
        )
        .property(
            "evidence",
            ArrayBuilder::new().items(ObjectBuilder::new().schema_type(Type::String)),
        )
        .property("context_status", nullable_string_schema())
        .property(
            "input_snapshot",
            AnyOfBuilder::new()
                .item(
                    ObjectBuilder::new()
                        .schema_type(Type::Object)
                        .additional_properties(Some(AdditionalProperties::FreeForm(true))),
                )
                .item(ObjectBuilder::new().schema_type(Type::Null)),
        )
        .property(
            "updated_at_unix_seconds",
            AnyOfBuilder::new()
                .item(openapi_integer())
                .item(ObjectBuilder::new().schema_type(Type::Null)),
        )
        .required("status")
        .required("job_id")
        .required("updated_at_unix_seconds")
        .into()
}

pub(super) fn support_schema() -> RefOr<Schema> {
    ObjectBuilder::new()
        .schema_type(Type::String)
        .enum_values(Some(["supported", "tentative"]))
        .into()
}

pub(super) fn confidence_schema() -> RefOr<Schema> {
    ObjectBuilder::new()
        .schema_type(Type::String)
        .enum_values(Some(["high", "medium", "low"]))
        .into()
}

pub(super) fn review_status_schema() -> RefOr<Schema> {
    ObjectBuilder::new()
        .schema_type(Type::String)
        .enum_values(Some(["pending", "accepted", "rejected"]))
        .into()
}

pub(super) fn review_status_default_pending_schema() -> RefOr<Schema> {
    ObjectBuilder::new()
        .schema_type(Type::String)
        .enum_values(Some(["pending", "accepted", "rejected"]))
        .default(Some(json!("pending")))
        .into()
}

pub(super) fn nullable_review_status_schema() -> RefOr<Schema> {
    Schema::AnyOf(
        AnyOfBuilder::new()
            .item(review_status_schema())
            .item(ObjectBuilder::new().schema_type(Type::Null))
            .build(),
    )
    .into()
}

pub(super) fn bulk_review_decision_schema() -> RefOr<Schema> {
    ObjectBuilder::new()
        .schema_type(Type::String)
        .enum_values(Some(["accepted", "rejected"]))
        .into()
}

pub(super) fn review_failure_code_schema() -> RefOr<Schema> {
    ObjectBuilder::new()
        .schema_type(Type::String)
        .enum_values(Some(["not_found", "stale", "tag_limit"]))
        .into()
}

pub(super) fn cleanup_reason_schema() -> RefOr<Schema> {
    ObjectBuilder::new()
        .schema_type(Type::String)
        .enum_values(Some([
            "vocabulary_alias",
            "vocabulary_plural",
            "vocabulary_typo",
        ]))
        .into()
}

pub(super) fn energy_curve_schema() -> RefOr<Schema> {
    ObjectBuilder::new()
        .schema_type(Type::String)
        .enum_values(Some(["steady", "rising", "falling", "arc"]))
        .into()
}

pub(super) fn energy_curve_default_schema() -> RefOr<Schema> {
    ObjectBuilder::new()
        .schema_type(Type::String)
        .enum_values(Some(["steady", "rising", "falling", "arc"]))
        .default(Some(json!("steady")))
        .into()
}

pub(super) fn nullable_tag_schema() -> RefOr<Schema> {
    Schema::AnyOf(
        AnyOfBuilder::new()
            .item(
                ObjectBuilder::new()
                    .schema_type(Type::String)
                    .max_length(Some(64)),
            )
            .item(ObjectBuilder::new().schema_type(Type::Null))
            .build(),
    )
    .into()
}

pub(super) fn vocabulary_version_schema() -> RefOr<Schema> {
    const_string_schema(TAG_VOCABULARY_SCHEMA)
}
pub(super) fn cleanup_preview_version_schema() -> RefOr<Schema> {
    const_string_schema(TAG_CLEANUP_PREVIEW_SCHEMA)
}
pub(super) fn cleanup_apply_version_schema() -> RefOr<Schema> {
    const_string_schema(TAG_CLEANUP_APPLY_SCHEMA)
}
pub(super) fn playlist_disclosure_version_schema() -> RefOr<Schema> {
    const_string_schema("assistant-playlist-model-disclosure/v4")
}
pub(super) fn playlist_role_id_schema() -> RefOr<Schema> {
    const_string_schema("playlist_planner")
}
pub(super) fn playlist_quality_id_schema() -> RefOr<Schema> {
    const_string_schema("playlist-quality-v1")
}
pub(super) fn model_playlist_maximum_candidates_schema() -> RefOr<Schema> {
    ObjectBuilder::new()
        .schema_type(Type::Integer)
        .minimum(Some(1))
        .maximum(Some(100))
        .into()
}
pub(super) fn model_tag_cleanup_disclosure_version_schema() -> RefOr<Schema> {
    const_string_schema("assistant-model-tag-cleanup-disclosure/v3")
}
pub(super) fn model_tag_cleanup_role_schema() -> RefOr<Schema> {
    const_string_schema("tag_cleanup")
}
pub(super) fn model_tag_cleanup_quality_schema() -> RefOr<Schema> {
    const_string_schema("tag-cleanup-quality-v1")
}
pub(super) fn model_tag_cleanup_maximum_tags_schema() -> RefOr<Schema> {
    ObjectBuilder::new()
        .schema_type(Type::Integer)
        .minimum(Some(1))
        .maximum(Some(500))
        .into()
}
pub(super) fn model_tag_cleanup_request_count_schema() -> RefOr<Schema> {
    ObjectBuilder::new()
        .schema_type(Type::Integer)
        .minimum(Some(0))
        .maximum(Some(25))
        .into()
}
pub(super) fn model_tagging_disclosure_version_schema() -> RefOr<Schema> {
    const_string_schema("assistant-model-music-tagging-disclosure/v16")
}
pub(super) fn model_tagging_role_schema() -> RefOr<Schema> {
    const_string_schema("music_tagger")
}
pub(super) fn model_tagging_quality_schema() -> RefOr<Schema> {
    const_string_schema("music-tagging-quality-v1")
}
pub(super) fn model_tagging_context_policy_schema() -> RefOr<Schema> {
    ObjectBuilder::new()
        .schema_type(Type::String)
        .enum_values(Some(["include", "skip"]))
        .default(Some(json!("include")))
        .into()
}
pub(super) fn model_tagging_tracks_per_request_schema() -> RefOr<Schema> {
    ObjectBuilder::new()
        .schema_type(Type::Integer)
        .minimum(Some(1))
        .maximum(Some(20))
        .into()
}
pub(super) fn model_tagging_retry_limit_schema() -> RefOr<Schema> {
    ObjectBuilder::new()
        .schema_type(Type::Integer)
        .minimum(Some(0))
        .maximum(Some(5))
        .into()
}
pub(super) fn eq_disclosure_version_schema() -> RefOr<Schema> {
    const_string_schema("assistant-eq-draft-disclosure/v2")
}
pub(super) fn eq_role_id_schema() -> RefOr<Schema> {
    const_string_schema("eq_assistant")
}
pub(super) fn eq_quality_id_schema() -> RefOr<Schema> {
    const_string_schema("eq-quality-v1")
}

pub(super) fn true_schema() -> RefOr<Schema> {
    ObjectBuilder::new()
        .schema_type(Type::Boolean)
        .extensions(Some([("const", json!(true))].into_iter().collect()))
        .into()
}

pub(super) fn const_string_schema(value: &'static str) -> RefOr<Schema> {
    ObjectBuilder::new()
        .schema_type(Type::String)
        .extensions(Some([("const", json!(value))].into_iter().collect()))
        .into()
}

pub(super) fn context_scope_kind_schema() -> RefOr<Schema> {
    ObjectBuilder::new()
        .schema_type(Type::String)
        .enum_values(Some(["all", "folder", "tracks"]))
        .default(Some(json!("all")))
        .into()
}

pub(super) fn model_tagging_scope_ref() -> RefOr<Schema> {
    RefOr::Ref(utoipa::openapi::Ref::from_schema_name("ModelTaggingScope"))
}

pub(super) fn context_track_ids_schema() -> RefOr<Schema> {
    ArrayBuilder::new()
        .items(openapi_integer())
        .max_items(Some(5_000))
        .into()
}

pub(super) fn context_analyzer_id_schema() -> RefOr<Schema> {
    const_string_schema("local-context/v3")
}

pub(super) fn voice_analyzer_id_schema() -> RefOr<Schema> {
    const_string_schema("essentia-musicnn-voice/v2")
}

pub(super) fn voice_analyzer_status_schema() -> RefOr<Schema> {
    ObjectBuilder::new()
        .schema_type(Type::String)
        .enum_values(Some(["not_configured", "ready", "unavailable"]))
        .into()
}

pub(super) fn nullable_voice_reason_schema() -> RefOr<Schema> {
    Schema::AnyOf(
        AnyOfBuilder::new()
            .item(
                ObjectBuilder::new()
                    .schema_type(Type::String)
                    .enum_values(Some([
                        "model_missing",
                        "model_unreadable",
                        "unsupported_model",
                        "runtime_missing",
                    ])),
            )
            .item(ObjectBuilder::new().schema_type(Type::Null))
            .build(),
    )
    .into()
}

pub(super) fn context_status_schema() -> RefOr<Schema> {
    ObjectBuilder::new()
        .schema_type(Type::String)
        .enum_values(Some(["full", "partial", "missing", "stale", "failed"]))
        .into()
}

pub(super) fn free_object_schema() -> RefOr<Schema> {
    ObjectBuilder::new()
        .schema_type(Type::Object)
        .additional_properties(Some(AdditionalProperties::FreeForm(true)))
        .into()
}

pub(super) fn nullable_object_schema() -> RefOr<Schema> {
    Schema::AnyOf(
        AnyOfBuilder::new()
            .item(free_object_schema())
            .item(ObjectBuilder::new().schema_type(Type::Null))
            .build(),
    )
    .into()
}

pub(super) fn context_timeline_schema() -> RefOr<Schema> {
    ArrayBuilder::new()
        .items(
            ObjectBuilder::new()
                .schema_type(Type::Object)
                .additional_properties(Some(AdditionalProperties::RefOr(openapi_number()))),
        )
        .into()
}

pub(super) fn context_sections_schema() -> RefOr<Schema> {
    ArrayBuilder::new().items(free_object_schema()).into()
}
