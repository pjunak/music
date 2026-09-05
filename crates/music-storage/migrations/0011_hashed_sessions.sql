-- Revoke legacy bearer tokens, including copies in preserved migration backups.
-- Accounts and authored data are retained. Clients sign in once after upgrading.
DROP TABLE auth_sessions;
CREATE TABLE auth_sessions (
    session_id VARCHAR(32) NOT NULL,
    token_hash VARCHAR(64) NOT NULL,
    user_id INTEGER NOT NULL,
    created_at DATETIME NOT NULL,
    expires_at DATETIME NOT NULL,
    last_seen DATETIME NOT NULL,
    PRIMARY KEY (session_id),
    UNIQUE (token_hash),
    FOREIGN KEY(user_id) REFERENCES users (id) ON DELETE CASCADE
);
CREATE INDEX ix_auth_sessions_user_id ON auth_sessions (user_id);
