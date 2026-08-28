use std::sync::OnceLock;
use std::time::Duration;

use music_domain::{IndexedTrack, LibraryPath, TrackId, TrackMetadata};
use serde_json::{Value, json};

use super::{
    AssistantTrackEvidence, EnergyCurve, EqDraftTask, ModelPlaylistTask, ModelTagCleanupTask,
    ModelTaggerBatch, PlaylistSuggestionRequest, StructuredModelResult, TagUsage,
    default_vocabulary_snapshot,
};

static EQ_TASK: OnceLock<Option<EqDraftTask>> = OnceLock::new();
static PLAYLIST_TASK: OnceLock<Option<ModelPlaylistTask>> = OnceLock::new();
static TAGGER_TASK: OnceLock<Option<ModelTaggerBatch>> = OnceLock::new();
static VOCABULARY: OnceLock<Option<super::TagVocabularySnapshot>> = OnceLock::new();

/// Exercises every strict Assistant structured-output parser with the same
/// untrusted JSON value. This surface exists only for the separate fuzzing
/// workspace; normal server builds do not expose it.
pub fn exercise_structured_model_outputs(input: &[u8]) {
    let Ok(payload) = serde_json::from_slice::<Value>(input) else {
        return;
    };

    if let Some(task) = EQ_TASK
        .get_or_init(|| EqDraftTask::new("Fuzz EQ", "warm but clear").ok())
        .as_ref()
    {
        let _ = task.finish(successful_result(payload.clone()));
    }

    if let Some(task) = TAGGER_TASK.get_or_init(tagger_fixture).as_ref() {
        let _ = task.finish(successful_result(payload.clone()));
    }

    if let Some(vocabulary) = VOCABULARY
        .get_or_init(|| default_vocabulary_snapshot().ok())
        .as_ref()
    {
        let usage = [TagUsage {
            tag: "zzzzzzzz-unmapped-fixture".to_owned(),
            track_count: 1,
        }];
        if let Ok(mut task) = ModelTagCleanupTask::new(&usage, vocabulary.clone()) {
            let _ = task.accept(successful_result(payload.clone()));
        }
    }

    if let Some(task) = PLAYLIST_TASK.get_or_init(playlist_fixture).as_ref() {
        let _ = task.finish(successful_result(payload));
    }
}

fn tagger_fixture() -> Option<ModelTaggerBatch> {
    ModelTaggerBatch::new(
        vec![json!({
            "track_id": 1,
            "title": "Fuzz track",
            "display_title": "Fuzz track",
            "artist": "Fixture artist",
            "album": "Fixture album",
            "origin": "fixture",
            "genre": "ambient",
            "length_s": 300.0
        })],
        VOCABULARY
            .get_or_init(|| default_vocabulary_snapshot().ok())
            .clone()?,
    )
    .ok()
}

fn successful_result(payload: Value) -> StructuredModelResult {
    StructuredModelResult {
        succeeded: true,
        error_code: None,
        payload: Some(payload),
        provider_model_id: None,
        finish_reason: Some("stop".to_owned()),
        input_tokens: None,
        output_tokens: None,
    }
}

fn playlist_fixture() -> Option<ModelPlaylistTask> {
    let id = TrackId::new(1).ok()?;
    let path = LibraryPath::parse("Fuzz/track.flac").ok()?;
    let track = AssistantTrackEvidence {
        track: IndexedTrack {
            id,
            path,
            metadata: TrackMetadata {
                title: "Fuzz track".to_owned(),
                artist: "Fixture artist".to_owned(),
                album_artist: String::new(),
                album: "Fixture album".to_owned(),
                track_no: Some(1),
                disc_no: Some(1),
                year: Some(2026),
                genre: "ambient".to_owned(),
                bpm: None,
            },
            duration: Duration::from_secs(300),
            display_title: "Fuzz track".to_owned(),
            origin: "fixture".to_owned(),
            size_bytes: 1,
            mtime_unix_seconds: 1,
            added_at_unix_seconds: 1,
        },
        manual_tags: vec!["calm".to_owned()],
        analyses: Vec::new(),
        reviews: Vec::new(),
    };
    let request = PlaylistSuggestionRequest {
        prompt: "calm ambient".to_owned(),
        target_minutes: 5,
        candidate_limit: 5,
        min_bpm: None,
        max_bpm: None,
        include_unknown_bpm: true,
        exclude_track_ids: Vec::new(),
        energy_curve: EnergyCurve::Steady,
    };
    ModelPlaylistTask::new(&[track], &request).ok()
}
