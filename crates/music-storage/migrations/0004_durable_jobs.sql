ALTER TABLE background_jobs
ADD COLUMN lane VARCHAR(16) NOT NULL DEFAULT 'local'
CHECK (lane IN ('local', 'provider'));

ALTER TABLE background_jobs
ADD COLUMN schema_version INTEGER NOT NULL DEFAULT 1
CHECK (schema_version > 0);

ALTER TABLE background_jobs
ADD COLUMN restartable BOOLEAN NOT NULL DEFAULT 1
CHECK (restartable IN (0, 1));

ALTER TABLE background_jobs
ADD COLUMN checkpoint_policy VARCHAR(16) NOT NULL DEFAULT 'replace'
CHECK (checkpoint_policy IN ('replace'));

ALTER TABLE background_jobs
ADD COLUMN execution_id VARCHAR(32);

UPDATE background_jobs
SET lane = 'provider', restartable = 0
WHERE kind LIKE 'assistant.model%';

CREATE INDEX IF NOT EXISTS ix_background_jobs_lane_status_created
ON background_jobs (lane, status, created_at, id);
