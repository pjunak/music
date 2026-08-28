UPDATE library_state
SET discovered_tracks = (SELECT COUNT(*) FROM tracks),
    updated_at = CURRENT_TIMESTAMP
WHERE id = 1;
