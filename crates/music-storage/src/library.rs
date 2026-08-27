use std::collections::{BTreeMap, BTreeSet};
use std::time::Duration;

use music_application::library::{
    LibraryDependencyError, LibraryFuture, LibraryRepository, LibrarySearch, LibrarySearchResult,
    LibrarySortKey, LibraryStatus, ReconciliationStatus, SortOrder,
};
use music_domain::{IndexedTrack, LibraryGeneration, LibraryPath, TrackId, TrackMetadata};
use sqlx::sqlite::SqliteRow;
use sqlx::{QueryBuilder, Row, Sqlite};

use crate::{SqliteStorage, StorageError};

const TRACK_COLUMNS: &str = "id, path, title, artist, album_artist, album, track_no, disc_no, \
    year, genre, length_s, bpm, display_title, origin, size_bytes, mtime, \
    CAST(strftime('%s', added_at) AS INTEGER) AS added_at_unix_seconds";

impl LibraryRepository for SqliteStorage {
    fn status(&self) -> LibraryFuture<'_, LibraryStatus> {
        Box::pin(async move { self.read_library_status().await.map_err(box_storage) })
    }

    fn catalog_track_ids(&self) -> LibraryFuture<'_, Vec<TrackId>> {
        Box::pin(async move {
            let rows = sqlx::query_scalar::<_, i64>("SELECT id FROM tracks ORDER BY path, id")
                .fetch_all(&self.pool)
                .await
                .map_err(StorageError::from)
                .map_err(box_storage)?;
            rows.into_iter()
                .map(|id| {
                    TrackId::new(id)
                        .map_err(|_| StorageError::InvalidLibraryRecord("track id is invalid"))
                        .map_err(box_storage)
                })
                .collect()
        })
    }

    fn track(&self, track_id: TrackId) -> LibraryFuture<'_, Option<IndexedTrack>> {
        Box::pin(async move {
            let mut query = QueryBuilder::<Sqlite>::new(format!(
                "SELECT {TRACK_COLUMNS} FROM tracks WHERE id = "
            ));
            query.push_bind(track_id.get());
            let row = query
                .build()
                .fetch_optional(&self.pool)
                .await
                .map_err(StorageError::from)
                .map_err(box_storage)?;
            row.map(|row| indexed_track_from_row(&row).map_err(box_storage))
                .transpose()
        })
    }

    fn tracks_by_ids<'a>(
        &'a self,
        track_ids: &'a [TrackId],
    ) -> LibraryFuture<'a, Vec<IndexedTrack>> {
        Box::pin(async move {
            if track_ids.is_empty() {
                return Ok(Vec::new());
            }
            let mut query = QueryBuilder::<Sqlite>::new(format!(
                "SELECT {TRACK_COLUMNS} FROM tracks WHERE id IN ("
            ));
            let mut separated = query.separated(", ");
            for track_id in track_ids {
                separated.push_bind(track_id.get());
            }
            separated.push_unseparated(")");
            let rows = query
                .build()
                .fetch_all(&self.pool)
                .await
                .map_err(StorageError::from)
                .map_err(box_storage)?;
            let mut by_id = BTreeMap::new();
            for row in rows {
                let track = indexed_track_from_row(&row).map_err(box_storage)?;
                by_id.insert(track.id, track);
            }
            let mut seen = BTreeSet::new();
            Ok(track_ids
                .iter()
                .filter(|track_id| seen.insert(**track_id))
                .filter_map(|track_id| by_id.remove(track_id))
                .collect())
        })
    }

    fn search<'a>(&'a self, request: &'a LibrarySearch) -> LibraryFuture<'a, LibrarySearchResult> {
        Box::pin(async move {
            let pattern = (!request.query.is_empty())
                .then(|| format!("%{}%", escape_like(&request.query.to_lowercase())));
            let mut count = QueryBuilder::<Sqlite>::new("SELECT COUNT(*) FROM tracks");
            if let Some(pattern) = pattern.as_deref() {
                push_search_filter(&mut count, pattern);
            }
            let total: i64 = count
                .build_query_scalar()
                .fetch_one(&self.pool)
                .await
                .map_err(StorageError::from)
                .map_err(box_storage)?;
            let total = u64::try_from(total)
                .map_err(|_| StorageError::InvalidLibraryRecord("track count is negative"))
                .map_err(box_storage)?;

            let mut query =
                QueryBuilder::<Sqlite>::new(format!("SELECT {TRACK_COLUMNS} FROM tracks"));
            if let Some(pattern) = pattern.as_deref() {
                push_search_filter(&mut query, pattern);
            }
            query.push(" ORDER BY ").push(sort_expression(request.sort));
            match request.order {
                SortOrder::Ascending => query.push(" ASC NULLS LAST, id ASC"),
                SortOrder::Descending => query.push(" DESC NULLS FIRST, id ASC"),
            };
            query
                .push(" LIMIT ")
                .push_bind(i64::from(request.limit))
                .push(" OFFSET ")
                .push_bind(i64::try_from(request.offset).unwrap_or(i64::MAX));
            let rows = query
                .build()
                .fetch_all(&self.pool)
                .await
                .map_err(StorageError::from)
                .map_err(box_storage)?;
            let tracks = rows
                .iter()
                .map(indexed_track_from_row)
                .collect::<Result<Vec<_>, _>>()
                .map_err(box_storage)?;
            Ok(LibrarySearchResult { tracks, total })
        })
    }

    fn tracks_in_directory<'a>(
        &'a self,
        directory: Option<&'a LibraryPath>,
    ) -> LibraryFuture<'a, Vec<IndexedTrack>> {
        Box::pin(async move {
            let mut query =
                QueryBuilder::<Sqlite>::new(format!("SELECT {TRACK_COLUMNS} FROM tracks WHERE "));
            if let Some(directory) = directory {
                let prefix = escape_like(&format!("{}/", directory.as_str()));
                query
                    .push("path LIKE ")
                    .push_bind(format!("{prefix}%"))
                    .push(" ESCAPE '\\' AND path NOT LIKE ")
                    .push_bind(format!("{prefix}%/%"))
                    .push(" ESCAPE '\\'");
            } else {
                query.push("instr(path, '/') = 0");
            }
            query.push(" ORDER BY path, id");
            let rows = query
                .build()
                .fetch_all(&self.pool)
                .await
                .map_err(StorageError::from)
                .map_err(box_storage)?;
            rows.iter()
                .map(indexed_track_from_row)
                .collect::<Result<Vec<_>, _>>()
                .map_err(box_storage)
        })
    }
}

impl SqliteStorage {
    async fn read_library_status(&self) -> Result<LibraryStatus, StorageError> {
        let row = sqlx::query(
            "SELECT generation, status, \
                    CAST(strftime('%s', scan_started_at) AS INTEGER) AS scan_started_at, \
                    CAST(strftime('%s', last_scan_at) AS INTEGER) AS last_scan_at, \
                    last_error_code, discovered_tracks \
             FROM library_state WHERE id = 1",
        )
        .fetch_one(&self.pool)
        .await?;
        let status = match row.try_get::<String, _>("status")?.as_str() {
            "pending" => ReconciliationStatus::Pending,
            "reconciling" => ReconciliationStatus::Reconciling,
            "current" => ReconciliationStatus::Current,
            "failed" => ReconciliationStatus::Failed,
            _ => {
                return Err(StorageError::InvalidLibraryState(
                    "reconciliation status is unsupported",
                ));
            }
        };
        let discovered_tracks = row.try_get::<i64, _>("discovered_tracks")?;
        Ok(LibraryStatus {
            generation: LibraryGeneration::try_from(row.try_get::<i64, _>("generation")?)
                .map_err(|_| StorageError::InvalidLibraryState("generation is invalid"))?,
            status,
            scan_started_at_unix_seconds: row.try_get("scan_started_at")?,
            last_scan_at_unix_seconds: row.try_get("last_scan_at")?,
            last_error_code: row.try_get("last_error_code")?,
            discovered_tracks: u64::try_from(discovered_tracks).map_err(|_| {
                StorageError::InvalidLibraryState("discovered track count is invalid")
            })?,
        })
    }
}

fn indexed_track_from_row(row: &SqliteRow) -> Result<IndexedTrack, StorageError> {
    let numeric = |field: &'static str, value: Option<i64>| {
        value
            .map(|value| {
                u32::try_from(value).map_err(|_| StorageError::InvalidLibraryRecord(field))
            })
            .transpose()
    };
    let duration_seconds: f64 = row.try_get("length_s")?;
    let duration = Duration::try_from_secs_f64(duration_seconds)
        .map_err(|_| StorageError::InvalidLibraryRecord("track duration is invalid"))?;
    let size_bytes: i64 = row.try_get("size_bytes")?;
    Ok(IndexedTrack {
        id: TrackId::new(row.try_get("id")?)
            .map_err(|_| StorageError::InvalidLibraryRecord("track id is invalid"))?,
        path: LibraryPath::parse(row.try_get::<String, _>("path")?)
            .map_err(StorageError::InvalidLibraryPath)?,
        metadata: TrackMetadata {
            title: row.try_get("title")?,
            artist: row.try_get("artist")?,
            album_artist: row.try_get("album_artist")?,
            album: row.try_get("album")?,
            track_no: numeric("track_no is invalid", row.try_get("track_no")?)?,
            disc_no: numeric("disc_no is invalid", row.try_get("disc_no")?)?,
            year: numeric("year is invalid", row.try_get("year")?)?,
            genre: row.try_get("genre")?,
            bpm: numeric("bpm is invalid", row.try_get("bpm")?)?,
        },
        duration,
        display_title: row.try_get("display_title")?,
        origin: row.try_get("origin")?,
        size_bytes: u64::try_from(size_bytes)
            .map_err(|_| StorageError::InvalidLibraryRecord("track size is negative"))?,
        mtime_unix_seconds: row.try_get("mtime")?,
        added_at_unix_seconds: row
            .try_get::<Option<i64>, _>("added_at_unix_seconds")?
            .ok_or(StorageError::InvalidTimestamp)?,
    })
}

fn push_search_filter(query: &mut QueryBuilder<Sqlite>, pattern: &str) {
    query.push(" WHERE (");
    for (index, field) in [
        "title",
        "display_title",
        "artist",
        "album",
        "origin",
        "path",
    ]
    .iter()
    .enumerate()
    {
        if index != 0 {
            query.push(" OR ");
        }
        query
            .push("lower(")
            .push(*field)
            .push(") LIKE ")
            .push_bind(pattern.to_owned())
            .push(" ESCAPE '\\'");
    }
    query.push(")");
}

fn sort_expression(sort: LibrarySortKey) -> &'static str {
    match sort {
        LibrarySortKey::Title => article_sort("title"),
        LibrarySortKey::Artist => article_sort("artist"),
        LibrarySortKey::Album => article_sort("album"),
        LibrarySortKey::AlbumArtist => article_sort("album_artist"),
        LibrarySortKey::Year => "year",
        LibrarySortKey::LengthSeconds => "length_s",
        LibrarySortKey::TrackNumber => "track_no",
        LibrarySortKey::AddedAt => "added_at",
        LibrarySortKey::Path => "lower(trim(path))",
    }
}

fn article_sort(field: &str) -> &'static str {
    match field {
        "title" => {
            "CASE WHEN lower(trim(title)) LIKE 'the %' THEN substr(lower(trim(title)), 5) \
             WHEN lower(trim(title)) LIKE 'an %' THEN substr(lower(trim(title)), 4) \
             WHEN lower(trim(title)) LIKE 'a %' THEN substr(lower(trim(title)), 3) \
             ELSE lower(trim(title)) END"
        }
        "artist" => {
            "CASE WHEN lower(trim(artist)) LIKE 'the %' THEN substr(lower(trim(artist)), 5) \
             WHEN lower(trim(artist)) LIKE 'an %' THEN substr(lower(trim(artist)), 4) \
             WHEN lower(trim(artist)) LIKE 'a %' THEN substr(lower(trim(artist)), 3) \
             ELSE lower(trim(artist)) END"
        }
        "album" => {
            "CASE WHEN lower(trim(album)) LIKE 'the %' THEN substr(lower(trim(album)), 5) \
             WHEN lower(trim(album)) LIKE 'an %' THEN substr(lower(trim(album)), 4) \
             WHEN lower(trim(album)) LIKE 'a %' THEN substr(lower(trim(album)), 3) \
             ELSE lower(trim(album)) END"
        }
        "album_artist" => {
            "CASE WHEN lower(trim(album_artist)) LIKE 'the %' THEN substr(lower(trim(album_artist)), 5) \
             WHEN lower(trim(album_artist)) LIKE 'an %' THEN substr(lower(trim(album_artist)), 4) \
             WHEN lower(trim(album_artist)) LIKE 'a %' THEN substr(lower(trim(album_artist)), 3) \
             ELSE lower(trim(album_artist)) END"
        }
        _ => "id",
    }
}

fn escape_like(value: &str) -> String {
    value
        .replace('\\', "\\\\")
        .replace('%', "\\%")
        .replace('_', "\\_")
}

fn box_storage(source: StorageError) -> LibraryDependencyError {
    Box::new(source)
}

#[cfg(test)]
mod tests {
    use std::error::Error;
    use std::sync::Arc;

    use music_application::library::{
        LibraryRepository, LibrarySearch, LibrarySortKey, ReconciliationStatus, SortOrder,
    };
    use music_domain::{LibraryPath, TrackId};
    use tempfile::tempdir;

    use crate::{SqliteStorage, SqliteStorageOptions};

    async fn insert_track(
        storage: &SqliteStorage,
        path: &str,
        title: &str,
        artist: &str,
    ) -> Result<i64, sqlx::Error> {
        let result = sqlx::query(
            "INSERT INTO tracks (path, title, artist, album_artist, album, track_no, disc_no, \
             year, genre, length_s, bpm, display_title, origin, size_bytes, mtime, added_at) \
             VALUES (?, ?, ?, ?, '', NULL, NULL, NULL, '', 12.5, NULL, '', '', 123, 456, \
                     '2026-08-27 12:34:56')",
        )
        .bind(path)
        .bind(title)
        .bind(artist)
        .bind(artist)
        .execute(&storage.pool)
        .await?;
        Ok(result.last_insert_rowid())
    }

    #[tokio::test]
    async fn catalog_queries_preserve_order_scope_literal_search_and_article_sorting()
    -> Result<(), Box<dyn Error + Send + Sync>> {
        let directory = tempdir()?;
        let storage = Arc::new(
            SqliteStorage::open(SqliteStorageOptions::new(directory.path().join("app.db"))).await?,
        );
        let root_id = insert_track(&storage, "root.mp3", "Root", "Solo").await?;
        let ac_id = insert_track(&storage, "Albums/AC_DC 50%.mp3", "Literal", "The Doors").await?;
        let other_id = insert_track(&storage, "Albums/ACXDC 500.mp3", "Other", "A Tribe").await?;
        insert_track(&storage, "Albums/Deep/song.mp3", "Deep", "Artist").await?;

        let status = LibraryRepository::status(storage.as_ref()).await?;
        assert_eq!(status.generation.get(), 0);
        assert_eq!(status.status, ReconciliationStatus::Pending);

        let literal = LibrarySearch::new(
            "AC_DC 50%",
            100,
            0,
            LibrarySortKey::Artist,
            SortOrder::Ascending,
        )?;
        let result = LibraryRepository::search(storage.as_ref(), &literal).await?;
        assert_eq!(result.total, 1);
        assert_eq!(result.tracks[0].id.get(), ac_id);

        let sorted = LibrarySearch::new("", 100, 0, LibrarySortKey::Artist, SortOrder::Ascending)?;
        let result = LibraryRepository::search(storage.as_ref(), &sorted).await?;
        let artist_ids = result
            .tracks
            .iter()
            .filter(|track| [ac_id, other_id].contains(&track.id.get()))
            .map(|track| track.id.get())
            .collect::<Vec<_>>();
        assert_eq!(artist_ids, vec![ac_id, other_id]);

        let requested = [
            TrackId::new(other_id)?,
            TrackId::new(ac_id)?,
            TrackId::new(other_id)?,
            TrackId::new(999_999)?,
        ];
        let batch = LibraryRepository::tracks_by_ids(storage.as_ref(), &requested).await?;
        assert_eq!(
            batch.iter().map(|track| track.id.get()).collect::<Vec<_>>(),
            vec![other_id, ac_id]
        );

        let root = LibraryRepository::tracks_in_directory(storage.as_ref(), None).await?;
        assert_eq!(root.len(), 1);
        assert_eq!(root[0].id.get(), root_id);
        let albums = LibraryPath::parse("Albums")?;
        let direct =
            LibraryRepository::tracks_in_directory(storage.as_ref(), Some(&albums)).await?;
        assert_eq!(direct.len(), 2);
        assert!(
            direct
                .iter()
                .all(|track| !track.path.as_str().contains("Deep/"))
        );
        Ok(())
    }

    #[tokio::test]
    async fn invalid_stored_paths_never_cross_the_repository_boundary()
    -> Result<(), Box<dyn Error + Send + Sync>> {
        let directory = tempdir()?;
        let storage = SqliteStorage::open(SqliteStorageOptions::new(
            directory.path().join("invalid.db"),
        ))
        .await?;
        let track_id = insert_track(&storage, "../outside.mp3", "Invalid", "Artist").await?;
        assert!(
            LibraryRepository::track(&storage, TrackId::new(track_id)?)
                .await
                .is_err()
        );
        Ok(())
    }
}
