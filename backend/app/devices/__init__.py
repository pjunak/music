"""Persistent, operator-curated device registry.

Stored as a standalone JSON file in the data directory (`DEVICES_FILE`), NOT in
`app.db` — so the list of remembered devices and their audio-output
designations survive a reinstall *and* an `app.db` wipe, exactly like the
music/, modes/ and presets/ directories.
"""
