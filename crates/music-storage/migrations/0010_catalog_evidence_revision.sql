CREATE TABLE catalog_evidence_state (
    id INTEGER PRIMARY KEY CHECK (id = 1),
    revision INTEGER NOT NULL CHECK (revision > 0)
);
INSERT INTO catalog_evidence_state (id, revision) VALUES (1, 1);
ALTER TABLE cleanup_track_enrichments ADD COLUMN evidence_revision INTEGER NOT NULL DEFAULT 0;

-- Legacy evidence has no source-policy or vocabulary provenance. Authored tags
-- remain intact; only regenerable proposals and their review decisions expire.
DELETE FROM cleanup_track_enrichments;
DELETE FROM track_analysis_tag_reviews WHERE analyzer_id = 'catalog-tags/v1';
DELETE FROM track_analyses WHERE analyzer_id = 'catalog-tags/v1';
