CREATE TABLE cleanup_rejections (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    fingerprint TEXT NOT NULL UNIQUE,
    context_signature TEXT NOT NULL,
    proposal_json TEXT NOT NULL CHECK (json_valid(proposal_json)),
    search_text TEXT NOT NULL,
    rejected_at INTEGER NOT NULL DEFAULT (unixepoch())
);
