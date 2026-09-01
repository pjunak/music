CREATE TABLE IF NOT EXISTS cleanup_track_enrichments (
    track_id BIGINT NOT NULL,
    source_signature VARCHAR(64) NOT NULL,
    result_json TEXT NOT NULL,
    updated_at DATETIME NOT NULL,
    PRIMARY KEY (track_id),
    FOREIGN KEY(track_id) REFERENCES tracks (id) ON DELETE CASCADE
);
