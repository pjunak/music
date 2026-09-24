-- Generated evidence is disposable. Authored tags and catalog facts are independent.
DELETE FROM track_analysis_tag_reviews;
DELETE FROM track_analysis_failures;
DELETE FROM track_analyses;
DELETE FROM track_contexts;
ALTER TABLE track_contexts DROP COLUMN confidence;

-- Supersede old local checkpoints before job recovery. Keep run/attempt audit data.
UPDATE background_jobs
SET status = 'cancelled', restartable = 0,
    result_json = CASE WHEN lane = 'local' THEN NULL ELSE result_json END,
    error = 'Superseded by the song evidence rebuild; start a new analysis.',
    progress_phase = 'Superseded', progress_message = 'Fresh audio analysis required',
    finished_at = CURRENT_TIMESTAMP, updated_at = CURRENT_TIMESTAMP, execution_id = NULL
WHERE kind IN ('assistant.library-analysis', 'assistant.library-audio-analysis',
               'assistant.library-context-analysis', 'assistant.model-music-tagging',
               'assistant.model-tagging-batch-collect')
  AND status IN ('queued', 'running', 'cancel_requested');

-- Remote submissions may still exist; preserve identifiers and accounting, never replay.
UPDATE assistant_model_batches SET state = 'expired', updated_at = CURRENT_TIMESTAMP
WHERE state NOT IN ('completed', 'cancelled', 'failed', 'expired');
