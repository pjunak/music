-- Generated decisions are rebuilt; authored and accepted tags remain untouched.
DELETE FROM track_analysis_tag_reviews;
DELETE FROM track_analyses;
ALTER TABLE track_analyses DROP COLUMN energy;
ALTER TABLE track_analyses DROP COLUMN brightness;
ALTER TABLE track_analyses DROP COLUMN tension;
ALTER TABLE track_analyses DROP COLUMN confidence;
ALTER TABLE track_analyses ADD COLUMN decisions_json TEXT NOT NULL DEFAULT '[]';

-- Preserve authored rules and materialized tracks, but retire unreviewed heuristic tags.
UPDATE playlists SET automatic_rule_json = json_set(automatic_rule_json, '$.tag_sources', 'manual'),
    automatic_source_signature = NULL
WHERE json_valid(automatic_rule_json) AND json_extract(automatic_rule_json, '$.tag_sources') = 'manual_and_local';

UPDATE background_jobs SET status = 'cancelled', restartable = 0, execution_id = NULL,
    finished_at = CURRENT_TIMESTAMP, updated_at = CURRENT_TIMESTAMP, progress_message = 'Tag decision contract replaced; start a fresh tagging run'
WHERE kind IN ('assistant.model-music-tagging', 'assistant.model-tagging-batch-collect')
    AND status IN ('queued', 'running', 'cancel_requested');
UPDATE assistant_model_batches SET state = 'expired', updated_at = CURRENT_TIMESTAMP
WHERE state NOT IN ('completed', 'failed', 'cancelled', 'expired');
