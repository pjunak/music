CREATE TABLE IF NOT EXISTS cleanup_source_policies (
    source_id VARCHAR(64) NOT NULL,
    enabled BOOLEAN NOT NULL,
    updated_at DATETIME NOT NULL,
    PRIMARY KEY (source_id)
);
