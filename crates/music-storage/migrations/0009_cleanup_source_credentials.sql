CREATE TABLE cleanup_source_credentials (
    source_id VARCHAR(32) NOT NULL CHECK (source_id IN ('acoustid', 'lastfm')),
    encrypted_api_key TEXT NOT NULL,
    api_key_nonce VARCHAR(64) NOT NULL,
    api_key_hint VARCHAR(32) NOT NULL,
    created_at DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP,
    PRIMARY KEY (source_id)
);
