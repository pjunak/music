CREATE TABLE IF NOT EXISTS library_state (
    id INTEGER NOT NULL,
    generation INTEGER NOT NULL DEFAULT 0,
    status VARCHAR(16) NOT NULL DEFAULT 'pending',
    scan_started_at DATETIME,
    last_scan_at DATETIME,
    last_error_code VARCHAR(64),
    discovered_tracks INTEGER NOT NULL DEFAULT 0,
    updated_at DATETIME NOT NULL,
    PRIMARY KEY (id),
    CONSTRAINT ck_library_state_singleton CHECK (id = 1),
    CONSTRAINT ck_library_state_generation CHECK (generation >= 0),
    CONSTRAINT ck_library_state_status CHECK (
        status IN ('pending', 'reconciling', 'current', 'failed')
    ),
    CONSTRAINT ck_library_state_discovered_tracks CHECK (discovered_tracks >= 0)
);

INSERT OR IGNORE INTO library_state (
    id,
    generation,
    status,
    discovered_tracks,
    updated_at
) VALUES (1, 0, 'pending', 0, CURRENT_TIMESTAMP);
