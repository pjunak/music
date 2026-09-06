CREATE TABLE assistant_model_batches (
    id TEXT PRIMARY KEY NOT NULL,
    connection_id TEXT NOT NULL REFERENCES assistant_provider_connections(id) ON DELETE CASCADE,
    state TEXT NOT NULL,
    input_file_id TEXT,
    remote_batch_id TEXT,
    document_json TEXT NOT NULL,
    updated_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP
);
CREATE INDEX ix_assistant_model_batches_state ON assistant_model_batches(state);
