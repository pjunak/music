-- The one-time devices.json import ran at the Rust cutover. Remembered devices
-- live in remembered_devices; the import ledger has no remaining reader.
DROP TABLE IF EXISTS legacy_device_imports;
