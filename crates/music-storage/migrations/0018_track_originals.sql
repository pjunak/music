-- A converted track's library file is a streaming copy; its original is kept
-- under the originals root at the library path with this extension. Size and
-- mtime are the original file's, which analysis uses as the track's identity.
CREATE TABLE track_originals (
    track_id INTEGER NOT NULL,
    extension VARCHAR(16) NOT NULL,
    size_bytes INTEGER NOT NULL,
    mtime INTEGER NOT NULL,
    PRIMARY KEY (track_id),
    FOREIGN KEY(track_id) REFERENCES tracks (id) ON DELETE CASCADE
);
