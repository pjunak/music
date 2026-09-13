ALTER TABLE tracks ADD COLUMN release_date TEXT NOT NULL DEFAULT '';
ALTER TABLE tracks ADD COLUMN original_release_date TEXT NOT NULL DEFAULT '';
ALTER TABLE tracks ADD COLUMN composer TEXT NOT NULL DEFAULT '';

-- Preserve the precision present in old indexes until embedded tags are rescanned.
UPDATE tracks SET release_date = printf('%04d', year) WHERE year BETWEEN 1 AND 9999;
