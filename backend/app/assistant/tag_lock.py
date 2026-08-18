import threading

# Manual-tag edits and generated-tag acceptance share this process-local lock
# so their read/validate/write transactions cannot jointly exceed per-track
# limits. SQLite still provides the cross-process database write lock.
tag_write_lock = threading.Lock()
