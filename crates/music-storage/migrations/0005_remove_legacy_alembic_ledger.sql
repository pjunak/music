-- The Python runtime may leave Alembic's bookkeeping table in an otherwise
-- compatible database. Preflight validates its exact shape and creates a
-- verified backup before this explicit destructive migration is allowed.
DROP TABLE IF EXISTS alembic_version;
