-- Python replaced this SQL-backed registry with devices.json before the Rust
-- cutover but left the old table behind. Preflight validates its exact
-- historical shape and creates a verified backup before removal.
DROP TABLE IF EXISTS known_devices;
