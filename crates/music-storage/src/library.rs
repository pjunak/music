use std::collections::{BTreeMap, BTreeSet};
use std::time::Duration;

use music_application::cleanup::{
    CleanupBatchAppend, CleanupBatchDetail, CleanupBatchSummary, CleanupFuture,
    CleanupMutationCommit, CleanupMutationRepository, CleanupNameVerdict, CleanupRepository,
    CleanupRevertMutation, CleanupRevertMutationCommit, CleanupVerificationRepository,
    MAX_CLEANUP_BATCH_HISTORY, MAX_CLEANUP_SCOPE_LABEL_CHARS,
};
use music_application::library::{
    DiscoveredTrack, LibraryCatalogTrack, LibraryDependencyError, LibraryFileMutation,
    LibraryFileMutationOutcome, LibraryFuture, LibraryIndexMutationCommit,
    LibraryMutationRepository, LibraryRepository, LibrarySearch, LibrarySearchResult,
    LibrarySortKey, LibraryStatus, LibraryTrackMutationCommit, LibraryUploadMutationCommit,
    ReconciliationCommit, ReconciliationStatus, ReconciliationSummary, SortOrder,
    TrackMetadataField, TrackMetadataPatch, TrackMetadataPatchValue,
};
use music_application::recovery::RecoveryJournalId;
use music_domain::{
    IndexedTrack, LibraryGeneration, LibraryPath, NameVerdicts, TrackId, TrackMetadata,
};
use serde_json::{Value, json};
use sqlx::sqlite::SqliteRow;
use sqlx::{QueryBuilder, Row, Sqlite};

use crate::{SqliteStorage, StorageError};

pub(crate) const TRACK_COLUMNS: &str = "id, path, title, artist, album_artist, album, track_no, disc_no, \
    year, genre, length_s, bpm, display_title, origin, size_bytes, mtime, \
    CAST(strftime('%s', added_at) AS INTEGER) AS added_at_unix_seconds";
const MAX_RECONCILIATION_TRACKS: usize = 1_000_000;
const RECONCILIATION_BATCH_SIZE: usize = 500;
const MAX_CLEANUP_BATCH_JSON_BYTES: i64 = 16 * 1024 * 1024;
const UPDATED_COUNT_SQL: &str = "SELECT COUNT(*) FROM temp.library_scan_stage AS staged \
    JOIN tracks ON tracks.path = staged.path WHERE \
    tracks.title IS NOT staged.title OR tracks.artist IS NOT staged.artist OR \
    tracks.album_artist IS NOT staged.album_artist OR tracks.album IS NOT staged.album OR \
    tracks.track_no IS NOT staged.track_no OR tracks.disc_no IS NOT staged.disc_no OR \
    tracks.year IS NOT staged.year OR tracks.genre IS NOT staged.genre OR \
    tracks.length_s IS NOT staged.length_s OR tracks.bpm IS NOT staged.bpm OR \
    tracks.size_bytes IS NOT staged.size_bytes OR tracks.mtime IS NOT staged.mtime";
const TRACK_UPSERT_SQL: &str = "INSERT INTO tracks (\
        path, title, artist, album_artist, album, track_no, disc_no, year, genre, length_s, bpm, \
        display_title, origin, size_bytes, mtime, added_at\
    ) SELECT path, title, artist, album_artist, album, track_no, disc_no, year, genre, length_s, \
             bpm, '', '', size_bytes, mtime, CURRENT_TIMESTAMP \
      FROM temp.library_scan_stage WHERE true \
      ON CONFLICT(path) DO UPDATE SET \
        title = excluded.title, artist = excluded.artist, album_artist = excluded.album_artist, \
        album = excluded.album, track_no = excluded.track_no, disc_no = excluded.disc_no, \
        year = excluded.year, genre = excluded.genre, length_s = excluded.length_s, \
        bpm = excluded.bpm, size_bytes = excluded.size_bytes, mtime = excluded.mtime \
      WHERE tracks.title IS NOT excluded.title OR tracks.artist IS NOT excluded.artist OR \
        tracks.album_artist IS NOT excluded.album_artist OR tracks.album IS NOT excluded.album OR \
        tracks.track_no IS NOT excluded.track_no OR tracks.disc_no IS NOT excluded.disc_no OR \
        tracks.year IS NOT excluded.year OR tracks.genre IS NOT excluded.genre OR \
        tracks.length_s IS NOT excluded.length_s OR tracks.bpm IS NOT excluded.bpm OR \
        tracks.size_bytes IS NOT excluded.size_bytes OR tracks.mtime IS NOT excluded.mtime";

impl LibraryRepository for SqliteStorage {
    fn status(&self) -> LibraryFuture<'_, LibraryStatus> {
        Box::pin(async move { self.read_library_status().await.map_err(box_storage) })
    }

    fn all_tracks(&self) -> LibraryFuture<'_, Vec<IndexedTrack>> {
        Box::pin(async move {
            let mut query =
                QueryBuilder::<Sqlite>::new(format!("SELECT {TRACK_COLUMNS} FROM tracks"));
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

    fn playback_catalog(&self) -> LibraryFuture<'_, Vec<LibraryCatalogTrack>> {
        Box::pin(async move {
            let rows = sqlx::query("SELECT id, path, length_s FROM tracks ORDER BY path, id")
                .fetch_all(&self.pool)
                .await
                .map_err(StorageError::from)
                .map_err(box_storage)?;
            rows.iter()
                .map(library_catalog_track_from_row)
                .collect::<Result<Vec<_>, _>>()
                .map_err(box_storage)
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

    fn track_by_path<'a>(
        &'a self,
        path: &'a LibraryPath,
    ) -> LibraryFuture<'a, Option<IndexedTrack>> {
        Box::pin(async move {
            let mut query = QueryBuilder::<Sqlite>::new(format!(
                "SELECT {TRACK_COLUMNS} FROM tracks WHERE path = "
            ));
            query.push_bind(path.as_str());
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

    fn folder_track_counts(&self) -> LibraryFuture<'_, BTreeMap<LibraryPath, u64>> {
        Box::pin(async move {
            let paths = sqlx::query_scalar::<_, String>("SELECT path FROM tracks ORDER BY path")
                .fetch_all(&self.pool)
                .await
                .map_err(StorageError::from)
                .map_err(box_storage)?;
            let mut counts = BTreeMap::new();
            for stored in paths {
                let mut parent = LibraryPath::parse(stored)
                    .map_err(StorageError::InvalidLibraryPath)
                    .map_err(box_storage)?
                    .parent();
                while let Some(directory) = parent {
                    let count = counts.entry(directory.clone()).or_insert(0_u64);
                    *count = count.checked_add(1).ok_or_else(|| {
                        box_storage(StorageError::InvalidLibraryState(
                            "folder track count overflowed",
                        ))
                    })?;
                    parent = directory.parent();
                }
            }
            Ok(counts)
        })
    }
}

impl CleanupRepository for SqliteStorage {
    fn cleanup_name_verdicts(&self) -> CleanupFuture<'_, NameVerdicts> {
        Box::pin(async move {
            let rows = sqlx::query_as::<_, (String, i64, i64)>(
                "SELECT loose_key, artist_score, album_score FROM cleanup_name_lookups \
                 ORDER BY id",
            )
            .fetch_all(&self.pool)
            .await
            .map_err(StorageError::from)
            .map_err(box_storage)?;
            rows.into_iter()
                .map(|(key, artist, album)| {
                    let artist = i32::try_from(artist).map_err(|_| {
                        box_storage(StorageError::InvalidLibraryRecord(
                            "cleanup artist score is invalid",
                        ))
                    })?;
                    let album = i32::try_from(album).map_err(|_| {
                        box_storage(StorageError::InvalidLibraryRecord(
                            "cleanup album score is invalid",
                        ))
                    })?;
                    Ok((key, (artist, album)))
                })
                .collect()
        })
    }

    fn cleanup_batches(&self) -> CleanupFuture<'_, Vec<CleanupBatchSummary>> {
        Box::pin(async move {
            let rows = sqlx::query_as::<_, (i64, i64, String, i64, Option<i64>)>(
                "SELECT id, CAST(strftime('%s', created_at) AS INTEGER), scope_label, \
                 CASE WHEN json_valid(items_json) = 0 THEN -1 \
                      WHEN json_type(items_json) != 'array' THEN -1 \
                      ELSE json_array_length(items_json) END, \
                 CAST(strftime('%s', reverted_at) AS INTEGER) \
                 FROM cleanup_batches ORDER BY created_at DESC, id DESC LIMIT ?",
            )
            .bind(i64::try_from(MAX_CLEANUP_BATCH_HISTORY).map_err(|_| {
                box_storage(StorageError::InvalidLibraryState(
                    "cleanup batch history limit is invalid",
                ))
            })?)
            .fetch_all(&self.pool)
            .await
            .map_err(StorageError::from)
            .map_err(box_storage)?;
            rows.into_iter()
                .map(|(id, created_at, scope_label, item_count, reverted_at)| {
                    cleanup_batch_summary(id, created_at, scope_label, item_count, reverted_at)
                        .map_err(box_storage)
                })
                .collect()
        })
    }

    fn cleanup_batch(&self, batch_id: i64) -> CleanupFuture<'_, Option<CleanupBatchDetail>> {
        Box::pin(async move {
            let row = sqlx::query_as::<_, (i64, i64, String, Option<String>, Option<i64>)>(
                "SELECT id, CAST(strftime('%s', created_at) AS INTEGER), scope_label, \
                 CASE WHEN length(items_json) <= ? THEN items_json ELSE NULL END, \
                 CAST(strftime('%s', reverted_at) AS INTEGER) \
                 FROM cleanup_batches WHERE id = ?",
            )
            .bind(MAX_CLEANUP_BATCH_JSON_BYTES)
            .bind(batch_id)
            .fetch_optional(&self.pool)
            .await
            .map_err(StorageError::from)
            .map_err(box_storage)?;
            row.map(|(id, created_at, scope_label, items_json, reverted_at)| {
                let items_json = items_json.ok_or(StorageError::InvalidLibraryRecord(
                    "cleanup batch journal is too large",
                ))?;
                let items =
                    serde_json::from_str::<Vec<serde_json::Map<String, serde_json::Value>>>(
                        &items_json,
                    )
                    .map_err(|_| {
                        StorageError::InvalidLibraryRecord("cleanup batch journal is invalid")
                    })?;
                let item_count = items.len();
                cleanup_batch_summary(
                    id,
                    created_at,
                    scope_label,
                    i64::try_from(item_count).map_err(|_| {
                        StorageError::InvalidLibraryRecord("cleanup batch item count is invalid")
                    })?,
                    reverted_at,
                )
                .map(|summary| CleanupBatchDetail {
                    id: summary.id,
                    created_at_unix_seconds: summary.created_at_unix_seconds,
                    scope_label: summary.scope_label,
                    item_count: summary.item_count,
                    reverted_at_unix_seconds: summary.reverted_at_unix_seconds,
                    items,
                })
            })
            .transpose()
            .map_err(box_storage)
        })
    }
}

fn cleanup_batch_summary(
    id: i64,
    created_at_unix_seconds: i64,
    scope_label: String,
    item_count: i64,
    reverted_at_unix_seconds: Option<i64>,
) -> Result<CleanupBatchSummary, StorageError> {
    if id <= 0 {
        return Err(StorageError::InvalidLibraryRecord(
            "cleanup batch id is invalid",
        ));
    }
    let item_count = usize::try_from(item_count)
        .map_err(|_| StorageError::InvalidLibraryRecord("cleanup batch item count is invalid"))?;
    Ok(CleanupBatchSummary {
        id,
        created_at_unix_seconds,
        scope_label,
        item_count,
        reverted_at_unix_seconds,
    })
}

impl CleanupVerificationRepository for SqliteStorage {
    fn cleanup_name_verdict_exists<'a>(&'a self, loose_key: &'a str) -> CleanupFuture<'a, bool> {
        Box::pin(async move {
            sqlx::query_scalar::<_, bool>(
                "SELECT EXISTS(SELECT 1 FROM cleanup_name_lookups WHERE loose_key = ?)",
            )
            .bind(loose_key)
            .fetch_one(&self.pool)
            .await
            .map_err(StorageError::from)
            .map_err(box_storage)
        })
    }

    fn store_cleanup_name_verdict<'a>(
        &'a self,
        verdict: &'a CleanupNameVerdict,
    ) -> CleanupFuture<'a, bool> {
        Box::pin(async move {
            let _admission = self.write_gate.lock().await;
            let result = sqlx::query(
                "INSERT INTO cleanup_name_lookups \
                 (loose_key, name, artist_score, album_score, fetched_at) \
                 VALUES (?, ?, ?, ?, CURRENT_TIMESTAMP) \
                 ON CONFLICT(loose_key) DO NOTHING",
            )
            .bind(verdict.loose_key())
            .bind(verdict.name())
            .bind(i64::from(verdict.scores().artist()))
            .bind(i64::from(verdict.scores().album()))
            .execute(&self.pool)
            .await
            .map_err(StorageError::from)
            .map_err(box_storage)?;
            Ok(result.rows_affected() == 1)
        })
    }
}

impl CleanupMutationRepository for SqliteStorage {
    fn commit_cleanup_mutation<'a>(
        &'a self,
        journal_id: &'a RecoveryJournalId,
        mutation: &'a LibraryFileMutation,
        outcome: LibraryFileMutationOutcome,
        append: &'a CleanupBatchAppend,
    ) -> CleanupFuture<'a, CleanupMutationCommit> {
        Box::pin(async move {
            let (status, affected_tracks, track, batch_id) = match (mutation, outcome) {
                (
                    LibraryFileMutation::RenameFolder {
                        source,
                        destination,
                    },
                    LibraryFileMutationOutcome::Folder { path, .. },
                ) if destination == &path => {
                    let (commit, batch_id) = self
                        .commit_library_folder_rename_with_cleanup(
                            journal_id,
                            source,
                            destination,
                            MutationJournalContext::CleanupApply(append),
                        )
                        .await
                        .map_err(box_storage)?;
                    (commit.status, commit.affected_tracks, None, batch_id)
                }
                (
                    LibraryFileMutation::MoveTrack {
                        track_id,
                        source,
                        destination,
                    },
                    LibraryFileMutationOutcome::TrackMoved {
                        track_id: moved_id,
                        track: discovered,
                    },
                ) if track_id == &moved_id && destination == &discovered.path => {
                    let (commit, batch_id) = self
                        .commit_library_track_move_with_cleanup(
                            journal_id,
                            *track_id,
                            source,
                            &discovered,
                            MutationJournalContext::CleanupApply(append),
                        )
                        .await
                        .map_err(box_storage)?;
                    (commit.status, 1, Some(commit.track), batch_id)
                }
                (
                    LibraryFileMutation::UpdateTrackMetadata {
                        track_id,
                        path,
                        patch,
                    },
                    LibraryFileMutationOutcome::TrackMetadataUpdated {
                        track_id: updated_id,
                        discovered,
                    },
                ) if track_id == &updated_id
                    && discovered.as_ref().is_none_or(|track| &track.path == path) =>
                {
                    let (commit, batch_id) = self
                        .commit_library_track_metadata_with_cleanup(
                            journal_id,
                            *track_id,
                            path,
                            patch,
                            discovered.as_ref(),
                            MutationJournalContext::CleanupApply(append),
                        )
                        .await
                        .map_err(box_storage)?;
                    (commit.status, 1, Some(commit.track), batch_id)
                }
                _ => {
                    return Err(box_storage(StorageError::InvalidLibraryState(
                        "cleanup mutation outcome is invalid",
                    )));
                }
            };
            let batch_id = batch_id.ok_or_else(|| {
                box_storage(StorageError::InvalidLibraryState(
                    "cleanup mutation did not append its batch journal",
                ))
            })?;
            Ok(CleanupMutationCommit {
                status,
                affected_tracks,
                track,
                batch_id,
            })
        })
    }

    fn commit_cleanup_revert_mutation<'a>(
        &'a self,
        journal_id: &'a RecoveryJournalId,
        mutation: &'a LibraryFileMutation,
        outcome: LibraryFileMutationOutcome,
        revert: &'a CleanupRevertMutation,
    ) -> CleanupFuture<'a, CleanupRevertMutationCommit> {
        Box::pin(async move {
            let (status, affected_tracks, track, batch_id) = match (mutation, outcome) {
                (
                    LibraryFileMutation::RenameFolder {
                        source,
                        destination,
                    },
                    LibraryFileMutationOutcome::Folder { path, .. },
                ) if destination == &path => {
                    let (commit, batch_id) = self
                        .commit_library_folder_rename_with_cleanup(
                            journal_id,
                            source,
                            destination,
                            MutationJournalContext::CleanupRevert(revert),
                        )
                        .await
                        .map_err(box_storage)?;
                    (commit.status, commit.affected_tracks, None, batch_id)
                }
                (
                    LibraryFileMutation::MoveTrack {
                        track_id,
                        source,
                        destination,
                    },
                    LibraryFileMutationOutcome::TrackMoved {
                        track_id: moved_id,
                        track: discovered,
                    },
                ) if track_id == &moved_id && destination == &discovered.path => {
                    let (commit, batch_id) = self
                        .commit_library_track_move_with_cleanup(
                            journal_id,
                            *track_id,
                            source,
                            &discovered,
                            MutationJournalContext::CleanupRevert(revert),
                        )
                        .await
                        .map_err(box_storage)?;
                    (commit.status, 1, Some(commit.track), batch_id)
                }
                (
                    LibraryFileMutation::UpdateTrackMetadata {
                        track_id,
                        path,
                        patch,
                    },
                    LibraryFileMutationOutcome::TrackMetadataUpdated {
                        track_id: updated_id,
                        discovered,
                    },
                ) if track_id == &updated_id
                    && discovered.as_ref().is_none_or(|track| &track.path == path) =>
                {
                    let (commit, batch_id) = self
                        .commit_library_track_metadata_with_cleanup(
                            journal_id,
                            *track_id,
                            path,
                            patch,
                            discovered.as_ref(),
                            MutationJournalContext::CleanupRevert(revert),
                        )
                        .await
                        .map_err(box_storage)?;
                    (commit.status, 1, Some(commit.track), batch_id)
                }
                _ => {
                    return Err(box_storage(StorageError::InvalidLibraryState(
                        "cleanup revert mutation outcome is invalid",
                    )));
                }
            };
            if batch_id.is_some() {
                return Err(box_storage(StorageError::InvalidLibraryState(
                    "cleanup revert unexpectedly appended batch history",
                )));
            }
            Ok(CleanupRevertMutationCommit {
                status,
                affected_tracks,
                track,
            })
        })
    }

    fn finish_cleanup_batch_revert<'a>(
        &'a self,
        journal_id: &'a RecoveryJournalId,
        batch_id: i64,
        reverted: usize,
        skipped: usize,
    ) -> CleanupFuture<'a, ()> {
        Box::pin(async move {
            if batch_id <= 0 {
                return Err(box_storage(StorageError::InvalidLibraryRecord(
                    "cleanup batch id is invalid",
                )));
            }
            let _admission = self.write_gate.lock().await;
            let mut transaction = self.pool.begin().await.map_err(StorageError::from)?;
            let plan = read_applying_mutation_journal(
                &mut transaction,
                journal_id,
                "cleanup",
                "revert_batch",
            )
            .await
            .map_err(box_storage)?;
            let plan = plan
                .as_object()
                .filter(|plan| plan.len() == 1)
                .ok_or_else(|| box_storage(StorageError::InvalidRecoveryJournalRecord))?;
            if plan.get("batch_id").and_then(Value::as_i64) != Some(batch_id) {
                return Err(box_storage(StorageError::InvalidRecoveryJournalRecord));
            }

            let batch = sqlx::query(
                "UPDATE cleanup_batches SET reverted_at = CURRENT_TIMESTAMP \
                 WHERE id = ? AND reverted_at IS NULL",
            )
            .bind(batch_id)
            .execute(&mut *transaction)
            .await
            .map_err(StorageError::from)
            .map_err(box_storage)?;
            if batch.rows_affected() != 1 {
                return Err(box_storage(StorageError::InvalidLibraryRecord(
                    "cleanup batch changed during revert",
                )));
            }

            let progress_json = serde_json::to_string(&json!({
                "reverted": reverted,
                "skipped": skipped,
            }))
            .map_err(StorageError::RecoveryJournalSerialization)
            .map_err(box_storage)?;
            let journal = sqlx::query(
                "UPDATE recovery_journal SET state = 'committed', progress_json = ?, \
                 updated_at = CURRENT_TIMESTAMP, completed_at = CURRENT_TIMESTAMP \
                 WHERE id = ? AND domain = 'cleanup' AND operation = 'revert_batch' \
                 AND state = 'applying'",
            )
            .bind(progress_json)
            .bind(journal_id.as_str())
            .execute(&mut *transaction)
            .await
            .map_err(StorageError::from)
            .map_err(box_storage)?;
            if journal.rows_affected() != 1 {
                return Err(box_storage(StorageError::InvalidRecoveryJournalRecord));
            }
            transaction
                .commit()
                .await
                .map_err(StorageError::from)
                .map_err(box_storage)
        })
    }
}

impl LibraryMutationRepository for SqliteStorage {
    fn begin_reconciliation(&self) -> LibraryFuture<'_, LibraryStatus> {
        Box::pin(async move {
            self.begin_library_reconciliation()
                .await
                .map_err(box_storage)
        })
    }

    fn commit_reconciliation(
        &self,
        expected_generation: LibraryGeneration,
        discovered: Vec<DiscoveredTrack>,
    ) -> LibraryFuture<'_, ReconciliationCommit> {
        Box::pin(async move {
            self.commit_library_reconciliation(expected_generation, &discovered)
                .await
                .map_err(box_storage)
        })
    }

    fn fail_reconciliation<'a>(
        &'a self,
        expected_generation: LibraryGeneration,
        error_code: &'a str,
    ) -> LibraryFuture<'a, LibraryStatus> {
        Box::pin(async move {
            self.fail_library_reconciliation(expected_generation, error_code)
                .await
                .map_err(box_storage)
        })
    }

    fn commit_folder_rename<'a>(
        &'a self,
        journal_id: &'a RecoveryJournalId,
        source: &'a LibraryPath,
        destination: &'a LibraryPath,
    ) -> LibraryFuture<'a, LibraryIndexMutationCommit> {
        Box::pin(async move {
            self.commit_library_folder_rename(journal_id, source, destination)
                .await
                .map_err(box_storage)
        })
    }

    fn commit_folder_delete<'a>(
        &'a self,
        journal_id: &'a RecoveryJournalId,
        path: &'a LibraryPath,
    ) -> LibraryFuture<'a, LibraryIndexMutationCommit> {
        Box::pin(async move {
            self.commit_library_folder_delete(journal_id, path)
                .await
                .map_err(box_storage)
        })
    }

    fn commit_track_move<'a>(
        &'a self,
        journal_id: &'a RecoveryJournalId,
        track_id: TrackId,
        source: &'a LibraryPath,
        discovered: &'a DiscoveredTrack,
    ) -> LibraryFuture<'a, LibraryTrackMutationCommit> {
        Box::pin(async move {
            self.commit_library_track_move(journal_id, track_id, source, discovered)
                .await
                .map_err(box_storage)
        })
    }

    fn commit_track_delete<'a>(
        &'a self,
        journal_id: &'a RecoveryJournalId,
        track_id: TrackId,
        path: &'a LibraryPath,
    ) -> LibraryFuture<'a, LibraryIndexMutationCommit> {
        Box::pin(async move {
            self.commit_library_track_delete(journal_id, track_id, path)
                .await
                .map_err(box_storage)
        })
    }

    fn commit_track_metadata<'a>(
        &'a self,
        journal_id: &'a RecoveryJournalId,
        track_id: TrackId,
        path: &'a LibraryPath,
        patch: &'a TrackMetadataPatch,
        discovered: Option<&'a DiscoveredTrack>,
    ) -> LibraryFuture<'a, LibraryTrackMutationCommit> {
        Box::pin(async move {
            self.commit_library_track_metadata(journal_id, track_id, path, patch, discovered)
                .await
                .map_err(box_storage)
        })
    }

    fn commit_upload<'a>(
        &'a self,
        journal_id: &'a RecoveryJournalId,
        staged: &'a LibraryPath,
        destination: &'a LibraryPath,
        replace_existing: bool,
        discovered: Option<&'a DiscoveredTrack>,
    ) -> LibraryFuture<'a, LibraryUploadMutationCommit> {
        Box::pin(async move {
            self.commit_library_upload(
                journal_id,
                staged,
                destination,
                replace_existing,
                discovered,
            )
            .await
            .map_err(box_storage)
        })
    }
}

#[derive(Debug, Clone, Copy)]
enum MutationJournalContext<'a> {
    Library,
    CleanupApply(&'a CleanupBatchAppend),
    CleanupRevert(&'a CleanupRevertMutation),
}

impl<'a> MutationJournalContext<'a> {
    const fn domain(self) -> &'static str {
        match self {
            Self::Library => "library",
            Self::CleanupApply(_) | Self::CleanupRevert(_) => "cleanup",
        }
    }

    fn expected_plan(self, mutation: &LibraryFileMutation) -> Result<Value, StorageError> {
        match self {
            Self::Library => Ok(mutation.plan()),
            Self::CleanupApply(append) => append
                .journal_plan(mutation)
                .map_err(|_| StorageError::InvalidRecoveryJournalRecord),
            Self::CleanupRevert(revert) => revert
                .journal_plan(mutation)
                .map_err(|_| StorageError::InvalidRecoveryJournalRecord),
        }
    }

    const fn batch_append(self) -> Option<&'a CleanupBatchAppend> {
        match self {
            Self::CleanupApply(append) => Some(append),
            Self::Library | Self::CleanupRevert(_) => None,
        }
    }
}

impl SqliteStorage {
    async fn commit_library_track_move(
        &self,
        journal_id: &RecoveryJournalId,
        track_id: TrackId,
        source: &LibraryPath,
        discovered: &DiscoveredTrack,
    ) -> Result<LibraryTrackMutationCommit, StorageError> {
        self.commit_library_track_move_with_cleanup(
            journal_id,
            track_id,
            source,
            discovered,
            MutationJournalContext::Library,
        )
        .await
        .map(|(commit, _)| commit)
    }

    async fn commit_library_track_move_with_cleanup(
        &self,
        journal_id: &RecoveryJournalId,
        track_id: TrackId,
        source: &LibraryPath,
        discovered: &DiscoveredTrack,
        context: MutationJournalContext<'_>,
    ) -> Result<(LibraryTrackMutationCommit, Option<i64>), StorageError> {
        if discovered.size_bytes > i64::MAX as u64 {
            return Err(StorageError::InvalidLibraryRecord(
                "track size exceeds SQLite integer range",
            ));
        }
        let _admission = self.write_gate.lock().await;
        let mut transaction = self.pool.begin().await?;
        validate_library_mutation_journal(
            &mut transaction,
            journal_id,
            &LibraryFileMutation::MoveTrack {
                track_id,
                source: source.clone(),
                destination: discovered.path.clone(),
            },
            context,
        )
        .await?;
        let updated = sqlx::query(
            "UPDATE tracks SET path = ?, title = ?, artist = ?, album_artist = ?, album = ?, \
             track_no = ?, disc_no = ?, year = ?, genre = ?, length_s = ?, bpm = ?, \
             size_bytes = ?, mtime = ? WHERE id = ? AND path = ?",
        )
        .bind(discovered.path.as_str())
        .bind(&discovered.metadata.title)
        .bind(&discovered.metadata.artist)
        .bind(&discovered.metadata.album_artist)
        .bind(&discovered.metadata.album)
        .bind(discovered.metadata.track_no.map(i64::from))
        .bind(discovered.metadata.disc_no.map(i64::from))
        .bind(discovered.metadata.year.map(i64::from))
        .bind(&discovered.metadata.genre)
        .bind(discovered.duration.as_secs_f64())
        .bind(discovered.metadata.bpm.map(i64::from))
        .bind(discovered.size_bytes as i64)
        .bind(discovered.mtime_unix_seconds)
        .bind(track_id.get())
        .bind(source.as_str())
        .execute(&mut *transaction)
        .await?;
        if updated.rows_affected() != 1 {
            transaction.rollback().await?;
            return Err(StorageError::InvalidLibraryRecord(
                "track move source changed",
            ));
        }
        let batch_id = finish_library_index_mutation(
            &mut transaction,
            journal_id,
            "move_track",
            1,
            LibraryCatalogEffect::PreserveMembership,
            context,
        )
        .await?;
        let mut query =
            QueryBuilder::<Sqlite>::new(format!("SELECT {TRACK_COLUMNS} FROM tracks WHERE id = "));
        query.push_bind(track_id.get());
        let row = query.build().fetch_one(&mut *transaction).await?;
        let track = indexed_track_from_row(&row)?;
        transaction.commit().await?;
        Ok((
            LibraryTrackMutationCommit {
                status: self.read_library_status().await?,
                track,
            },
            batch_id,
        ))
    }

    async fn commit_library_track_delete(
        &self,
        journal_id: &RecoveryJournalId,
        track_id: TrackId,
        path: &LibraryPath,
    ) -> Result<LibraryIndexMutationCommit, StorageError> {
        let _admission = self.write_gate.lock().await;
        let mut transaction = self.pool.begin().await?;
        validate_library_mutation_journal(
            &mut transaction,
            journal_id,
            &LibraryFileMutation::DeleteTrack {
                track_id,
                path: path.clone(),
            },
            MutationJournalContext::Library,
        )
        .await?;
        let deleted = sqlx::query("DELETE FROM tracks WHERE id = ? AND path = ?")
            .bind(track_id.get())
            .bind(path.as_str())
            .execute(&mut *transaction)
            .await?;
        let affected_tracks = deleted.rows_affected();
        finish_library_index_mutation(
            &mut transaction,
            journal_id,
            "delete_track",
            affected_tracks,
            LibraryCatalogEffect::RemoveTracks,
            MutationJournalContext::Library,
        )
        .await?;
        transaction.commit().await?;
        Ok(LibraryIndexMutationCommit {
            status: self.read_library_status().await?,
            affected_tracks,
        })
    }

    async fn commit_library_track_metadata(
        &self,
        journal_id: &RecoveryJournalId,
        track_id: TrackId,
        path: &LibraryPath,
        patch: &TrackMetadataPatch,
        discovered: Option<&DiscoveredTrack>,
    ) -> Result<LibraryTrackMutationCommit, StorageError> {
        self.commit_library_track_metadata_with_cleanup(
            journal_id,
            track_id,
            path,
            patch,
            discovered,
            MutationJournalContext::Library,
        )
        .await
        .map(|(commit, _)| commit)
    }

    async fn commit_library_track_metadata_with_cleanup(
        &self,
        journal_id: &RecoveryJournalId,
        track_id: TrackId,
        path: &LibraryPath,
        patch: &TrackMetadataPatch,
        discovered: Option<&DiscoveredTrack>,
        context: MutationJournalContext<'_>,
    ) -> Result<(LibraryTrackMutationCommit, Option<i64>), StorageError> {
        if patch.is_empty() || patch.has_tag_changes() != discovered.is_some() {
            return Err(StorageError::InvalidLibraryState(
                "metadata mutation outcome does not match its patch",
            ));
        }
        if discovered.is_some_and(|track| track.path != *path) {
            return Err(StorageError::InvalidLibraryRecord(
                "metadata mutation changed the track path",
            ));
        }
        if discovered.is_some_and(|track| track.size_bytes > i64::MAX as u64) {
            return Err(StorageError::InvalidLibraryRecord(
                "track size exceeds SQLite integer range",
            ));
        }

        let _admission = self.write_gate.lock().await;
        let mut transaction = self.pool.begin().await?;
        validate_library_mutation_journal(
            &mut transaction,
            journal_id,
            &LibraryFileMutation::UpdateTrackMetadata {
                track_id,
                path: path.clone(),
                patch: patch.clone(),
            },
            context,
        )
        .await?;

        let mut query =
            QueryBuilder::<Sqlite>::new(format!("SELECT {TRACK_COLUMNS} FROM tracks WHERE id = "));
        query
            .push_bind(track_id.get())
            .push(" AND path = ")
            .push_bind(path.as_str());
        let row = query
            .build()
            .fetch_optional(&mut *transaction)
            .await?
            .ok_or(StorageError::InvalidLibraryRecord(
                "metadata mutation track changed",
            ))?;
        let mut track = indexed_track_from_row(&row)?;

        if let Some(discovered) = discovered {
            track.metadata = discovered.metadata.clone();
            track.duration = discovered.duration;
            track.size_bytes = discovered.size_bytes;
            track.mtime_unix_seconds = discovered.mtime_unix_seconds;
        }
        for (field, value) in patch.changes() {
            let destination = match field {
                TrackMetadataField::DisplayTitle => &mut track.display_title,
                TrackMetadataField::Origin => &mut track.origin,
                _ => continue,
            };
            *destination = match value {
                TrackMetadataPatchValue::Text(value) => value.clone(),
                TrackMetadataPatchValue::Cleared => String::new(),
                TrackMetadataPatchValue::Number(_) => {
                    return Err(StorageError::InvalidLibraryState(
                        "database-only metadata field has a numeric value",
                    ));
                }
            };
        }

        let updated = sqlx::query(
            "UPDATE tracks SET title = ?, artist = ?, album_artist = ?, album = ?, track_no = ?, \
             disc_no = ?, year = ?, genre = ?, length_s = ?, bpm = ?, display_title = ?, \
             origin = ?, size_bytes = ?, mtime = ? WHERE id = ? AND path = ?",
        )
        .bind(&track.metadata.title)
        .bind(&track.metadata.artist)
        .bind(&track.metadata.album_artist)
        .bind(&track.metadata.album)
        .bind(track.metadata.track_no.map(i64::from))
        .bind(track.metadata.disc_no.map(i64::from))
        .bind(track.metadata.year.map(i64::from))
        .bind(&track.metadata.genre)
        .bind(track.duration.as_secs_f64())
        .bind(track.metadata.bpm.map(i64::from))
        .bind(&track.display_title)
        .bind(&track.origin)
        .bind(track.size_bytes as i64)
        .bind(track.mtime_unix_seconds)
        .bind(track_id.get())
        .bind(path.as_str())
        .execute(&mut *transaction)
        .await?;
        if updated.rows_affected() != 1 {
            transaction.rollback().await?;
            return Err(StorageError::InvalidLibraryRecord(
                "metadata mutation track changed",
            ));
        }

        let batch_id = finish_library_index_mutation(
            &mut transaction,
            journal_id,
            "update_track_metadata",
            1,
            LibraryCatalogEffect::PreserveMembership,
            context,
        )
        .await?;
        let mut query =
            QueryBuilder::<Sqlite>::new(format!("SELECT {TRACK_COLUMNS} FROM tracks WHERE id = "));
        query.push_bind(track_id.get());
        let row = query.build().fetch_one(&mut *transaction).await?;
        let track = indexed_track_from_row(&row)?;
        transaction.commit().await?;
        Ok((
            LibraryTrackMutationCommit {
                status: self.read_library_status().await?,
                track,
            },
            batch_id,
        ))
    }

    async fn commit_library_upload(
        &self,
        journal_id: &RecoveryJournalId,
        staged: &LibraryPath,
        destination: &LibraryPath,
        replace_existing: bool,
        discovered: Option<&DiscoveredTrack>,
    ) -> Result<LibraryUploadMutationCommit, StorageError> {
        if discovered.is_some_and(|track| track.path != *destination) {
            return Err(StorageError::InvalidLibraryRecord(
                "uploaded track path changed",
            ));
        }
        if discovered.is_some_and(|track| track.size_bytes > i64::MAX as u64) {
            return Err(StorageError::InvalidLibraryRecord(
                "uploaded track size exceeds SQLite integer range",
            ));
        }

        let _admission = self.write_gate.lock().await;
        let mut transaction = self.pool.begin().await?;
        validate_library_mutation_journal(
            &mut transaction,
            journal_id,
            &LibraryFileMutation::PublishUpload {
                staged: staged.clone(),
                destination: destination.clone(),
                replace_existing,
            },
            MutationJournalContext::Library,
        )
        .await?;

        let existed = sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM tracks WHERE path = ?")
            .bind(destination.as_str())
            .fetch_one(&mut *transaction)
            .await?
            != 0;
        let (affected_tracks, catalog_effect, track) = if let Some(discovered) = discovered {
            sqlx::query(
                "INSERT INTO tracks (path, title, artist, album_artist, album, track_no, disc_no, \
                 year, genre, length_s, bpm, display_title, origin, size_bytes, mtime, added_at) \
                 VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, '', '', ?, ?, CURRENT_TIMESTAMP) \
                 ON CONFLICT(path) DO UPDATE SET title = excluded.title, artist = excluded.artist, \
                 album_artist = excluded.album_artist, album = excluded.album, \
                 track_no = excluded.track_no, disc_no = excluded.disc_no, year = excluded.year, \
                 genre = excluded.genre, length_s = excluded.length_s, bpm = excluded.bpm, \
                 size_bytes = excluded.size_bytes, mtime = excluded.mtime",
            )
            .bind(destination.as_str())
            .bind(&discovered.metadata.title)
            .bind(&discovered.metadata.artist)
            .bind(&discovered.metadata.album_artist)
            .bind(&discovered.metadata.album)
            .bind(discovered.metadata.track_no.map(i64::from))
            .bind(discovered.metadata.disc_no.map(i64::from))
            .bind(discovered.metadata.year.map(i64::from))
            .bind(&discovered.metadata.genre)
            .bind(discovered.duration.as_secs_f64())
            .bind(discovered.metadata.bpm.map(i64::from))
            .bind(discovered.size_bytes as i64)
            .bind(discovered.mtime_unix_seconds)
            .execute(&mut *transaction)
            .await?;
            let catalog_effect = if existed {
                LibraryCatalogEffect::PreserveMembership
            } else {
                LibraryCatalogEffect::AddTracks
            };
            let mut query = QueryBuilder::<Sqlite>::new(format!(
                "SELECT {TRACK_COLUMNS} FROM tracks WHERE path = "
            ));
            query.push_bind(destination.as_str());
            let row = query.build().fetch_one(&mut *transaction).await?;
            (1, catalog_effect, Some(indexed_track_from_row(&row)?))
        } else {
            (0, LibraryCatalogEffect::PreserveMembership, None)
        };

        finish_library_index_mutation(
            &mut transaction,
            journal_id,
            "publish_upload",
            affected_tracks,
            catalog_effect,
            MutationJournalContext::Library,
        )
        .await?;
        transaction.commit().await?;
        Ok(LibraryUploadMutationCommit {
            status: self.read_library_status().await?,
            track,
            affected_tracks,
        })
    }

    async fn commit_library_folder_rename(
        &self,
        journal_id: &RecoveryJournalId,
        source: &LibraryPath,
        destination: &LibraryPath,
    ) -> Result<LibraryIndexMutationCommit, StorageError> {
        self.commit_library_folder_rename_with_cleanup(
            journal_id,
            source,
            destination,
            MutationJournalContext::Library,
        )
        .await
        .map(|(commit, _)| commit)
    }

    async fn commit_library_folder_rename_with_cleanup(
        &self,
        journal_id: &RecoveryJournalId,
        source: &LibraryPath,
        destination: &LibraryPath,
        context: MutationJournalContext<'_>,
    ) -> Result<(LibraryIndexMutationCommit, Option<i64>), StorageError> {
        let _admission = self.write_gate.lock().await;
        let mut transaction = self.pool.begin().await?;
        validate_library_mutation_journal(
            &mut transaction,
            journal_id,
            &LibraryFileMutation::RenameFolder {
                source: source.clone(),
                destination: destination.clone(),
            },
            context,
        )
        .await?;

        let source_prefix = format!("{}/", source.as_str());
        let pattern = format!("{}%", escape_like(&source_prefix));
        let limit = i64::try_from(MAX_RECONCILIATION_TRACKS + 1)
            .map_err(|_| StorageError::InvalidLibraryState("folder mutation limit is invalid"))?;
        let rows = sqlx::query(
            "SELECT path FROM tracks WHERE path LIKE ? ESCAPE '\\' ORDER BY id LIMIT ?",
        )
        .bind(&pattern)
        .bind(limit)
        .fetch_all(&mut *transaction)
        .await?;
        if rows.len() > MAX_RECONCILIATION_TRACKS {
            transaction.rollback().await?;
            return Err(StorageError::InvalidLibraryState(
                "folder mutation contains too many tracks",
            ));
        }

        for row in &rows {
            let path: String = row.try_get("path")?;
            let suffix =
                path.strip_prefix(&source_prefix)
                    .ok_or(StorageError::InvalidLibraryRecord(
                        "folder mutation path has an invalid prefix",
                    ))?;
            let _validated_destination = destination
                .join(suffix)
                .map_err(StorageError::InvalidLibraryPath)?;
        }

        let affected_tracks = u64::try_from(rows.len())
            .map_err(|_| StorageError::InvalidLibraryState("folder mutation count is too large"))?;
        let updated = sqlx::query(
            "UPDATE tracks SET path = ? || substr(path, length(?) + 1) \
             WHERE path LIKE ? ESCAPE '\\'",
        )
        .bind(destination.as_str())
        .bind(source.as_str())
        .bind(pattern)
        .execute(&mut *transaction)
        .await?;
        if updated.rows_affected() != affected_tracks {
            transaction.rollback().await?;
            return Err(StorageError::InvalidLibraryRecord(
                "folder mutation track set changed",
            ));
        }
        let batch_id = finish_library_index_mutation(
            &mut transaction,
            journal_id,
            "rename_folder",
            affected_tracks,
            LibraryCatalogEffect::PreserveMembership,
            context,
        )
        .await?;
        transaction.commit().await?;
        Ok((
            LibraryIndexMutationCommit {
                status: self.read_library_status().await?,
                affected_tracks,
            },
            batch_id,
        ))
    }

    async fn commit_library_folder_delete(
        &self,
        journal_id: &RecoveryJournalId,
        path: &LibraryPath,
    ) -> Result<LibraryIndexMutationCommit, StorageError> {
        let _admission = self.write_gate.lock().await;
        let mut transaction = self.pool.begin().await?;
        validate_library_delete_journal(&mut transaction, journal_id, path).await?;

        let prefix = escape_like(&format!("{}/", path.as_str()));
        let result = sqlx::query("DELETE FROM tracks WHERE path LIKE ? ESCAPE '\\'")
            .bind(format!("{prefix}%"))
            .execute(&mut *transaction)
            .await?;
        let affected_tracks = result.rows_affected();
        finish_library_index_mutation(
            &mut transaction,
            journal_id,
            "delete_folder",
            affected_tracks,
            LibraryCatalogEffect::RemoveTracks,
            MutationJournalContext::Library,
        )
        .await?;
        transaction.commit().await?;
        Ok(LibraryIndexMutationCommit {
            status: self.read_library_status().await?,
            affected_tracks,
        })
    }

    async fn begin_library_reconciliation(&self) -> Result<LibraryStatus, StorageError> {
        let _admission = self.write_gate.lock().await;
        let mut transaction = self.pool.begin().await?;
        let result = sqlx::query(
            "UPDATE library_state SET status = 'reconciling', \
             scan_started_at = CURRENT_TIMESTAMP, last_error_code = NULL, \
             updated_at = CURRENT_TIMESTAMP WHERE id = 1",
        )
        .execute(&mut *transaction)
        .await?;
        if result.rows_affected() != 1 {
            transaction.rollback().await?;
            return Err(StorageError::InvalidLibraryState(
                "singleton reconciliation row is missing",
            ));
        }
        transaction.commit().await?;
        self.read_library_status().await
    }

    async fn commit_library_reconciliation(
        &self,
        expected_generation: LibraryGeneration,
        discovered: &[DiscoveredTrack],
    ) -> Result<ReconciliationCommit, StorageError> {
        if discovered.len() > MAX_RECONCILIATION_TRACKS {
            return Err(StorageError::InvalidLibraryState(
                "reconciliation contains too many tracks",
            ));
        }
        let mut paths = BTreeSet::new();
        if discovered
            .iter()
            .any(|track| !paths.insert(track.path.as_str()))
        {
            return Err(StorageError::InvalidLibraryState(
                "reconciliation contains duplicate paths",
            ));
        }
        if discovered
            .iter()
            .any(|track| track.size_bytes > i64::MAX as u64)
        {
            return Err(StorageError::InvalidLibraryState(
                "track size exceeds SQLite integer range",
            ));
        }
        let discovered_count = i64::try_from(discovered.len()).map_err(|_| {
            StorageError::InvalidLibraryState("discovered track count is too large")
        })?;

        let _admission = self.write_gate.lock().await;
        let mut transaction = self.pool.begin().await?;
        let state = sqlx::query("SELECT generation, status FROM library_state WHERE id = 1")
            .fetch_one(&mut *transaction)
            .await?;
        let current_generation =
            LibraryGeneration::try_from(state.try_get::<i64, _>("generation")?)
                .map_err(|_| StorageError::InvalidLibraryState("generation is invalid"))?;
        if current_generation != expected_generation {
            transaction.rollback().await?;
            return Ok(ReconciliationCommit::Conflict { current_generation });
        }
        if state.try_get::<String, _>("status")? != "reconciling" {
            transaction.rollback().await?;
            return Err(StorageError::InvalidLibraryState(
                "reconciliation was not started",
            ));
        }

        sqlx::raw_sql(
            "CREATE TEMP TABLE IF NOT EXISTS library_scan_stage (\
                path TEXT NOT NULL PRIMARY KEY, title TEXT NOT NULL, artist TEXT NOT NULL, \
                album_artist TEXT NOT NULL, album TEXT NOT NULL, track_no INTEGER, disc_no INTEGER, \
                year INTEGER, genre TEXT NOT NULL, length_s REAL NOT NULL, bpm INTEGER, \
                size_bytes INTEGER NOT NULL, mtime INTEGER NOT NULL\
             ); \
             DELETE FROM temp.library_scan_stage;",
        )
        .execute(&mut *transaction)
        .await?;
        for chunk in discovered.chunks(RECONCILIATION_BATCH_SIZE) {
            let mut insert = QueryBuilder::<Sqlite>::new(
                "INSERT INTO temp.library_scan_stage (path, title, artist, album_artist, album, \
                 track_no, disc_no, year, genre, length_s, bpm, size_bytes, mtime) ",
            );
            insert.push_values(chunk, |mut row, track| {
                row.push_bind(track.path.as_str().to_owned())
                    .push_bind(track.metadata.title.clone())
                    .push_bind(track.metadata.artist.clone())
                    .push_bind(track.metadata.album_artist.clone())
                    .push_bind(track.metadata.album.clone())
                    .push_bind(track.metadata.track_no.map(i64::from))
                    .push_bind(track.metadata.disc_no.map(i64::from))
                    .push_bind(track.metadata.year.map(i64::from))
                    .push_bind(track.metadata.genre.clone())
                    .push_bind(track.duration.as_secs_f64())
                    .push_bind(track.metadata.bpm.map(i64::from))
                    .push_bind(track.size_bytes as i64)
                    .push_bind(track.mtime_unix_seconds);
            });
            insert.build().execute(&mut *transaction).await?;
        }

        let added = count_reconciliation_rows(
            &mut transaction,
            "SELECT COUNT(*) FROM temp.library_scan_stage AS staged \
             LEFT JOIN tracks ON tracks.path = staged.path WHERE tracks.id IS NULL",
        )
        .await?;
        let updated = count_reconciliation_rows(&mut transaction, UPDATED_COUNT_SQL).await?;
        let matching = count_reconciliation_rows(
            &mut transaction,
            "SELECT COUNT(*) FROM temp.library_scan_stage AS staged \
             JOIN tracks ON tracks.path = staged.path",
        )
        .await?;
        let removed = count_reconciliation_rows(
            &mut transaction,
            "SELECT COUNT(*) FROM tracks WHERE NOT EXISTS (\
                 SELECT 1 FROM temp.library_scan_stage AS staged WHERE staged.path = tracks.path\
             )",
        )
        .await?;
        let unchanged = matching
            .checked_sub(updated)
            .ok_or(StorageError::InvalidLibraryState(
                "reconciliation counts are inconsistent",
            ))?;

        sqlx::raw_sql(TRACK_UPSERT_SQL)
            .execute(&mut *transaction)
            .await?;
        sqlx::query(
            "DELETE FROM tracks WHERE NOT EXISTS (\
                 SELECT 1 FROM temp.library_scan_stage AS staged WHERE staged.path = tracks.path\
             )",
        )
        .execute(&mut *transaction)
        .await?;

        let next_generation = expected_generation
            .next()
            .map_err(|_| StorageError::InvalidLibraryState("generation overflowed"))?;
        let result = sqlx::query(
            "UPDATE library_state SET generation = ?, status = 'current', scan_started_at = NULL, \
             last_scan_at = CURRENT_TIMESTAMP, last_error_code = NULL, discovered_tracks = ?, \
             updated_at = CURRENT_TIMESTAMP \
             WHERE id = 1 AND generation = ? AND status = 'reconciling'",
        )
        .bind(i64::try_from(next_generation.get()).map_err(|_| {
            StorageError::InvalidLibraryState("generation exceeds SQLite integer range")
        })?)
        .bind(discovered_count)
        .bind(i64::try_from(expected_generation.get()).map_err(|_| {
            StorageError::InvalidLibraryState("generation exceeds SQLite integer range")
        })?)
        .execute(&mut *transaction)
        .await?;
        if result.rows_affected() != 1 {
            transaction.rollback().await?;
            return Err(StorageError::InvalidLibraryState(
                "reconciliation state changed during commit",
            ));
        }
        let ids = sqlx::query_scalar::<_, i64>("SELECT id FROM tracks ORDER BY id")
            .fetch_all(&mut *transaction)
            .await?;
        let track_ids = ids
            .into_iter()
            .map(|id| {
                TrackId::new(id)
                    .map_err(|_| StorageError::InvalidLibraryRecord("track id is invalid"))
            })
            .collect::<Result<BTreeSet<_>, _>>()?;
        transaction.commit().await?;
        let status = self.read_library_status().await?;
        Ok(ReconciliationCommit::Applied {
            status,
            summary: ReconciliationSummary {
                added,
                updated,
                removed,
                unchanged,
            },
            track_ids,
        })
    }

    async fn fail_library_reconciliation(
        &self,
        expected_generation: LibraryGeneration,
        error_code: &str,
    ) -> Result<LibraryStatus, StorageError> {
        if !(1..=64).contains(&error_code.len())
            || !error_code
                .bytes()
                .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'_')
        {
            return Err(StorageError::InvalidLibraryState(
                "reconciliation error code is invalid",
            ));
        }
        let expected_generation = i64::try_from(expected_generation.get()).map_err(|_| {
            StorageError::InvalidLibraryState("generation exceeds SQLite integer range")
        })?;
        let _admission = self.write_gate.lock().await;
        sqlx::query(
            "UPDATE library_state SET status = 'failed', last_error_code = ?, \
             updated_at = CURRENT_TIMESTAMP \
             WHERE id = 1 AND generation = ? AND status = 'reconciling'",
        )
        .bind(error_code)
        .bind(expected_generation)
        .execute(&self.pool)
        .await?;
        self.read_library_status().await
    }

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

async fn validate_library_mutation_journal(
    transaction: &mut sqlx::Transaction<'_, Sqlite>,
    journal_id: &RecoveryJournalId,
    mutation: &LibraryFileMutation,
    context: MutationJournalContext<'_>,
) -> Result<(), StorageError> {
    let operation = mutation
        .operation()
        .map_err(|_| StorageError::InvalidRecoveryJournalRecord)?;
    let plan = read_applying_mutation_journal(
        transaction,
        journal_id,
        context.domain(),
        operation.as_str(),
    )
    .await?;
    let expected = context.expected_plan(mutation)?;
    if plan != expected {
        return Err(StorageError::InvalidRecoveryJournalRecord);
    }
    Ok(())
}

async fn validate_library_delete_journal(
    transaction: &mut sqlx::Transaction<'_, Sqlite>,
    journal_id: &RecoveryJournalId,
    path: &LibraryPath,
) -> Result<(), StorageError> {
    let plan =
        read_applying_mutation_journal(transaction, journal_id, "library", "delete_folder").await?;
    let object = plan
        .as_object()
        .ok_or(StorageError::InvalidRecoveryJournalRecord)?;
    if object.len() != 2
        || object.get("path").and_then(Value::as_str) != Some(path.as_str())
        || object.get("recursive").and_then(Value::as_bool).is_none()
    {
        return Err(StorageError::InvalidRecoveryJournalRecord);
    }
    Ok(())
}

async fn read_applying_mutation_journal(
    transaction: &mut sqlx::Transaction<'_, Sqlite>,
    journal_id: &RecoveryJournalId,
    domain: &str,
    operation: &str,
) -> Result<Value, StorageError> {
    let row = sqlx::query(
        "SELECT domain, operation, state, plan_json FROM recovery_journal WHERE id = ?",
    )
    .bind(journal_id.as_str())
    .fetch_optional(&mut **transaction)
    .await?
    .ok_or(StorageError::InvalidRecoveryJournalRecord)?;
    if row.try_get::<String, _>("domain")? != domain
        || row.try_get::<String, _>("operation")? != operation
        || row.try_get::<String, _>("state")? != "applying"
    {
        return Err(StorageError::InvalidRecoveryJournalRecord);
    }
    serde_json::from_str(&row.try_get::<String, _>("plan_json")?)
        .map_err(StorageError::RecoveryJournalSerialization)
}

async fn finish_library_index_mutation(
    transaction: &mut sqlx::Transaction<'_, Sqlite>,
    journal_id: &RecoveryJournalId,
    operation: &'static str,
    affected_tracks: u64,
    catalog_effect: LibraryCatalogEffect,
    context: MutationJournalContext<'_>,
) -> Result<Option<i64>, StorageError> {
    if affected_tracks > 0 {
        let state =
            sqlx::query("SELECT generation, discovered_tracks FROM library_state WHERE id = 1")
                .fetch_one(&mut **transaction)
                .await?;
        let current_generation =
            LibraryGeneration::try_from(state.try_get::<i64, _>("generation")?)
                .map_err(|_| StorageError::InvalidLibraryState("generation is invalid"))?;
        let next_generation = current_generation
            .next()
            .map_err(|_| StorageError::InvalidLibraryState("generation overflowed"))?;
        let current_track_count = u64::try_from(state.try_get::<i64, _>("discovered_tracks")?)
            .map_err(|_| StorageError::InvalidLibraryState("track count is invalid"))?;
        let track_count = match catalog_effect {
            LibraryCatalogEffect::PreserveMembership => current_track_count,
            LibraryCatalogEffect::AddTracks => current_track_count
                .checked_add(affected_tracks)
                .ok_or(StorageError::InvalidLibraryState(
                    "catalog count overflowed during track addition",
                ))?,
            LibraryCatalogEffect::RemoveTracks => {
                current_track_count.checked_sub(affected_tracks).ok_or(
                    StorageError::InvalidLibraryState("track removal exceeds the catalog count"),
                )?
            }
        };
        let result = sqlx::query(
            "UPDATE library_state SET generation = ?, status = 'current', \
             scan_started_at = NULL, last_error_code = NULL, discovered_tracks = ?, \
             updated_at = CURRENT_TIMESTAMP WHERE id = 1 AND generation = ?",
        )
        .bind(i64::try_from(next_generation.get()).map_err(|_| {
            StorageError::InvalidLibraryState("generation exceeds SQLite integer range")
        })?)
        .bind(i64::try_from(track_count).map_err(|_| {
            StorageError::InvalidLibraryState("track count exceeds SQLite integer range")
        })?)
        .bind(i64::try_from(current_generation.get()).map_err(|_| {
            StorageError::InvalidLibraryState("generation exceeds SQLite integer range")
        })?)
        .execute(&mut **transaction)
        .await?;
        if result.rows_affected() != 1 {
            return Err(StorageError::InvalidLibraryState(
                "library state changed during mutation",
            ));
        }
    }

    let batch_id = match context.batch_append() {
        Some(append) => Some(append_cleanup_batch(transaction, append).await?),
        None => None,
    };
    let progress = batch_id.map_or_else(
        || json!({"affected_tracks": affected_tracks}),
        |batch_id| json!({"affected_tracks": affected_tracks, "batch_id": batch_id}),
    );
    let progress_json =
        serde_json::to_string(&progress).map_err(StorageError::RecoveryJournalSerialization)?;
    let journal = sqlx::query(
        "UPDATE recovery_journal SET state = 'committed', progress_json = ?, \
         updated_at = CURRENT_TIMESTAMP, completed_at = CURRENT_TIMESTAMP \
         WHERE id = ? AND domain = ? AND operation = ? AND state = 'applying'",
    )
    .bind(progress_json)
    .bind(journal_id.as_str())
    .bind(context.domain())
    .bind(operation)
    .execute(&mut **transaction)
    .await?;
    if journal.rows_affected() != 1 {
        return Err(StorageError::InvalidRecoveryJournalRecord);
    }

    Ok(batch_id)
}

async fn append_cleanup_batch(
    transaction: &mut sqlx::Transaction<'_, Sqlite>,
    append: &CleanupBatchAppend,
) -> Result<i64, StorageError> {
    if append.scope_label().chars().count() > MAX_CLEANUP_SCOPE_LABEL_CHARS
        || append.item().is_empty()
    {
        return Err(StorageError::InvalidLibraryRecord(
            "cleanup batch append is invalid",
        ));
    }
    let mut items = if let Some(batch_id) = append.batch_id() {
        let row = sqlx::query_as::<_, (Option<String>, bool)>(
            "SELECT CASE WHEN length(items_json) <= ? THEN items_json ELSE NULL END, \
                    reverted_at IS NOT NULL \
             FROM cleanup_batches WHERE id = ?",
        )
        .bind(MAX_CLEANUP_BATCH_JSON_BYTES)
        .bind(batch_id)
        .fetch_optional(&mut **transaction)
        .await?
        .ok_or(StorageError::InvalidLibraryRecord(
            "cleanup batch does not exist",
        ))?;
        if row.1 {
            return Err(StorageError::InvalidLibraryRecord(
                "cleanup batch was already reverted",
            ));
        }
        let encoded = row.0.ok_or(StorageError::InvalidLibraryRecord(
            "cleanup batch journal is too large",
        ))?;
        serde_json::from_str::<Vec<serde_json::Map<String, Value>>>(&encoded)
            .map_err(|_| StorageError::InvalidLibraryRecord("cleanup batch journal is invalid"))?
    } else {
        Vec::new()
    };
    items.push(append.item().clone());
    let items_json =
        serde_json::to_string(&items).map_err(StorageError::RecoveryJournalSerialization)?;
    if items_json.len()
        > usize::try_from(MAX_CLEANUP_BATCH_JSON_BYTES)
            .map_err(|_| StorageError::InvalidLibraryState("cleanup batch size limit is invalid"))?
    {
        return Err(StorageError::InvalidLibraryRecord(
            "cleanup batch journal is too large",
        ));
    }
    if let Some(batch_id) = append.batch_id() {
        let updated = sqlx::query(
            "UPDATE cleanup_batches SET items_json = ? \
             WHERE id = ? AND reverted_at IS NULL",
        )
        .bind(items_json)
        .bind(batch_id)
        .execute(&mut **transaction)
        .await?;
        if updated.rows_affected() != 1 {
            return Err(StorageError::InvalidLibraryRecord(
                "cleanup batch changed during append",
            ));
        }
        Ok(batch_id)
    } else {
        let inserted = sqlx::query(
            "INSERT INTO cleanup_batches (created_at, scope_label, items_json, reverted_at) \
             VALUES (CURRENT_TIMESTAMP, ?, ?, NULL)",
        )
        .bind(append.scope_label())
        .bind(items_json)
        .execute(&mut **transaction)
        .await?;
        let batch_id = inserted.last_insert_rowid();
        if batch_id <= 0 {
            return Err(StorageError::InvalidLibraryRecord(
                "cleanup batch id is invalid",
            ));
        }
        Ok(batch_id)
    }
}

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
enum LibraryCatalogEffect {
    PreserveMembership,
    AddTracks,
    RemoveTracks,
}

async fn count_reconciliation_rows(
    transaction: &mut sqlx::Transaction<'_, Sqlite>,
    statement: &'static str,
) -> Result<u64, StorageError> {
    let count: i64 = sqlx::query_scalar(statement)
        .fetch_one(&mut **transaction)
        .await?;
    u64::try_from(count)
        .map_err(|_| StorageError::InvalidLibraryState("reconciliation count is negative"))
}

pub(crate) fn indexed_track_from_row(row: &SqliteRow) -> Result<IndexedTrack, StorageError> {
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

fn library_catalog_track_from_row(row: &SqliteRow) -> Result<LibraryCatalogTrack, StorageError> {
    let duration_seconds: f64 = row.try_get("length_s")?;
    Ok(LibraryCatalogTrack {
        id: TrackId::new(row.try_get("id")?)
            .map_err(|_| StorageError::InvalidLibraryRecord("track id is invalid"))?,
        path: LibraryPath::parse(row.try_get::<String, _>("path")?)
            .map_err(StorageError::InvalidLibraryPath)?,
        duration: Duration::try_from_secs_f64(duration_seconds)
            .map_err(|_| StorageError::InvalidLibraryRecord("track duration is invalid"))?,
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
    use std::time::Duration;

    use music_application::cleanup::{
        CleanupNameScores, CleanupNameVerdict, CleanupRepository, CleanupVerificationRepository,
    };
    use music_application::library::{
        DiscoveredTrack, LibraryFileMutation, LibraryMutationRepository, LibraryRepository,
        LibrarySearch, LibrarySortKey, ReconciliationCommit, ReconciliationStatus,
        ReconciliationSummary, SortOrder, TrackMetadataField, TrackMetadataPatch,
    };
    use music_application::recovery::{
        RecoveryDomain, RecoveryJournalDraft, RecoveryJournalId, RecoveryJournalRepository,
        RecoveryState, RecoveryTransition,
    };
    use music_domain::{LibraryGeneration, LibraryPath, TrackId, TrackMetadata, cleanup_loose_key};
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
        sqlx::query(
            "UPDATE library_state SET discovered_tracks = (SELECT COUNT(*) FROM tracks) WHERE id = 1",
        )
        .execute(&storage.pool)
        .await?;
        Ok(result.last_insert_rowid())
    }

    fn discovered(
        path: &str,
        title: &str,
        artist: &str,
        size_bytes: u64,
        mtime_unix_seconds: i64,
    ) -> Result<DiscoveredTrack, music_domain::MediaPathError> {
        Ok(DiscoveredTrack {
            path: LibraryPath::parse(path)?,
            metadata: TrackMetadata {
                title: title.to_owned(),
                artist: artist.to_owned(),
                album_artist: artist.to_owned(),
                album: String::new(),
                track_no: None,
                disc_no: None,
                year: None,
                genre: String::new(),
                bpm: None,
            },
            duration: Duration::from_secs_f64(12.5),
            size_bytes,
            mtime_unix_seconds,
        })
    }

    async fn applying_journal(
        storage: &SqliteStorage,
        mutation: &LibraryFileMutation,
    ) -> Result<RecoveryJournalId, Box<dyn Error + Send + Sync>> {
        let draft = RecoveryJournalDraft::new(
            RecoveryDomain::Library,
            mutation.operation()?,
            mutation.plan(),
        )?;
        let id = draft.id.clone();
        RecoveryJournalRepository::create_recovery_journal(storage, draft).await?;
        let transition = RecoveryJournalRepository::transition_recovery_journal(
            storage,
            &id,
            RecoveryState::Planned,
            RecoveryState::Applying,
            serde_json::json!({}),
        )
        .await?;
        assert!(matches!(transition, RecoveryTransition::Applied(_)));
        Ok(id)
    }

    #[tokio::test]
    async fn cleanup_reads_the_complete_ordered_catalog_and_bounded_verdict_cache()
    -> Result<(), Box<dyn Error + Send + Sync>> {
        let directory = tempdir()?;
        let storage = SqliteStorage::open(SqliteStorageOptions::new(
            directory.path().join("cleanup-read-model.db"),
        ))
        .await?;
        let later_id = insert_track(&storage, "Zulu/later.mp3", "Later", "Artist").await?;
        let earlier_id = insert_track(&storage, "Alpha/earlier.mp3", "Earlier", "Artist").await?;
        sqlx::query(
            "INSERT INTO cleanup_name_lookups \
             (id, loose_key, name, artist_score, album_score, fetched_at) \
             VALUES (1, ?, ?, 100, 25, '2026-08-28 12:00:00')",
        )
        .bind(cleanup_loose_key("Andrey Vinogradov"))
        .bind("Andrey Vinogradov")
        .execute(&storage.pool)
        .await?;

        let tracks = LibraryRepository::all_tracks(&storage).await?;
        assert_eq!(tracks.len(), 2);
        assert_eq!(tracks[0].id.get(), earlier_id);
        assert_eq!(tracks[0].path.as_str(), "Alpha/earlier.mp3");
        assert_eq!(tracks[1].id.get(), later_id);
        let playback_catalog = LibraryRepository::playback_catalog(&storage).await?;
        assert_eq!(playback_catalog.len(), 2);
        assert_eq!(playback_catalog[0].id.get(), earlier_id);
        assert_eq!(playback_catalog[0].path.as_str(), "Alpha/earlier.mp3");
        assert_eq!(playback_catalog[1].id.get(), later_id);

        let verdicts = CleanupRepository::cleanup_name_verdicts(&storage).await?;
        assert_eq!(
            verdicts.get(&cleanup_loose_key("Andrey Vinogradov")),
            Some(&(100, 25))
        );

        let cached = CleanupNameVerdict::new(
            cleanup_loose_key("Abbey Road"),
            "Abbey Road",
            CleanupNameScores::new(10, 100)?,
        );
        assert!(
            CleanupVerificationRepository::store_cleanup_name_verdict(&storage, &cached).await?
        );
        assert!(
            CleanupVerificationRepository::cleanup_name_verdict_exists(
                &storage,
                cached.loose_key()
            )
            .await?
        );
        assert!(
            !CleanupVerificationRepository::store_cleanup_name_verdict(&storage, &cached).await?
        );
        let refreshed = CleanupRepository::cleanup_name_verdicts(&storage).await?;
        assert_eq!(refreshed.get(cached.loose_key()), Some(&(10, 100)));
        Ok(())
    }

    #[tokio::test]
    async fn cleanup_batch_history_is_newest_first_bounded_and_decoded()
    -> Result<(), Box<dyn Error + Send + Sync>> {
        let directory = tempdir()?;
        let storage = SqliteStorage::open(SqliteStorageOptions::new(
            directory.path().join("cleanup-history.db"),
        ))
        .await?;
        for id in 1..=105_i64 {
            sqlx::query(
                "INSERT INTO cleanup_batches \
                 (id, created_at, scope_label, items_json, reverted_at) \
                 VALUES (?, datetime('2026-08-28 12:00:00', ?), ?, ?, ?)",
            )
            .bind(id)
            .bind(format!("+{id} seconds"))
            .bind(format!("batch-{id}"))
            .bind(
                serde_json::json!([{
                    "kind": "rename",
                    "track_id": id,
                    "before": format!("old-{id}.mp3"),
                    "after": format!("new-{id}.mp3")
                }])
                .to_string(),
            )
            .bind((id == 105).then_some("2026-08-28 13:00:00"))
            .execute(&storage.pool)
            .await?;
        }

        let batches = CleanupRepository::cleanup_batches(&storage).await?;
        assert_eq!(batches.len(), 100);
        assert_eq!(batches[0].id, 105);
        assert_eq!(batches[0].scope_label, "batch-105");
        assert_eq!(batches[0].item_count, 1);
        assert!(batches[0].reverted_at_unix_seconds.is_some());
        assert_eq!(batches[99].id, 6);

        let detail = CleanupRepository::cleanup_batch(&storage, 42)
            .await?
            .ok_or("cleanup batch was missing")?;
        assert_eq!(detail.id, 42);
        assert_eq!(detail.item_count, 1);
        assert_eq!(detail.items[0]["kind"], "rename");
        assert_eq!(detail.items[0]["track_id"], 42);
        assert!(
            CleanupRepository::cleanup_batch(&storage, 999)
                .await?
                .is_none()
        );
        Ok(())
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

    #[tokio::test]
    async fn reconciliation_is_generation_checked_and_preserves_human_owned_fields()
    -> Result<(), Box<dyn Error + Send + Sync>> {
        let directory = tempdir()?;
        let storage = SqliteStorage::open(SqliteStorageOptions::new(
            directory.path().join("reconcile.db"),
        ))
        .await?;
        let unchanged_id = insert_track(&storage, "unchanged.mp3", "Same", "Artist").await?;
        let updated_id = insert_track(&storage, "updated.mp3", "Old", "Artist").await?;
        let removed_id = insert_track(&storage, "removed.mp3", "Removed", "Artist").await?;
        sqlx::query(
            "UPDATE tracks SET display_title = 'Human title', origin = 'Human origin' WHERE id = ?",
        )
        .bind(updated_id)
        .execute(&storage.pool)
        .await?;

        let started = LibraryMutationRepository::begin_reconciliation(&storage).await?;
        assert_eq!(started.generation, LibraryGeneration::new(0));
        assert_eq!(started.status, ReconciliationStatus::Reconciling);
        let commit = LibraryMutationRepository::commit_reconciliation(
            &storage,
            started.generation,
            vec![
                discovered("unchanged.mp3", "Same", "Artist", 123, 456)?,
                discovered("updated.mp3", "New", "Artist", 124, 457)?,
                discovered("new.mp3", "New track", "Artist", 10, 20)?,
            ],
        )
        .await?;
        let ReconciliationCommit::Applied {
            status,
            summary,
            track_ids,
        } = commit
        else {
            return Err("unexpected reconciliation conflict".into());
        };
        assert_eq!(status.generation, LibraryGeneration::new(1));
        assert_eq!(status.status, ReconciliationStatus::Current);
        assert_eq!(status.discovered_tracks, 3);
        assert_eq!(
            summary,
            ReconciliationSummary {
                added: 1,
                updated: 1,
                removed: 1,
                unchanged: 1,
            }
        );
        assert!(track_ids.contains(&TrackId::new(unchanged_id)?));
        assert!(track_ids.contains(&TrackId::new(updated_id)?));
        assert!(!track_ids.contains(&TrackId::new(removed_id)?));
        let updated = LibraryRepository::track(&storage, TrackId::new(updated_id)?)
            .await?
            .ok_or("updated track disappeared")?;
        assert_eq!(updated.metadata.title, "New");
        assert_eq!(updated.display_title, "Human title");
        assert_eq!(updated.origin, "Human origin");

        let second = LibraryMutationRepository::begin_reconciliation(&storage).await?;
        assert_eq!(second.generation, LibraryGeneration::new(1));
        assert_eq!(
            LibraryMutationRepository::commit_reconciliation(
                &storage,
                LibraryGeneration::new(0),
                Vec::new(),
            )
            .await?,
            ReconciliationCommit::Conflict {
                current_generation: LibraryGeneration::new(1)
            }
        );
        let failed = LibraryMutationRepository::fail_reconciliation(
            &storage,
            second.generation,
            "fixture_scan_failed",
        )
        .await?;
        assert_eq!(failed.status, ReconciliationStatus::Failed);
        assert_eq!(failed.generation, LibraryGeneration::new(1));
        assert_eq!(
            failed.last_error_code.as_deref(),
            Some("fixture_scan_failed")
        );
        assert_eq!(
            LibraryRepository::catalog_track_ids(&storage).await?.len(),
            3
        );
        Ok(())
    }

    #[tokio::test]
    async fn journaled_folder_mutations_preserve_ids_and_update_the_index_atomically()
    -> Result<(), Box<dyn Error + Send + Sync>> {
        let directory = tempdir()?;
        let storage = SqliteStorage::open(SqliteStorageOptions::new(
            directory.path().join("folders.db"),
        ))
        .await?;
        let first_id = insert_track(&storage, "Öld_%/one.mp3", "One", "Artist").await?;
        let nested_id = insert_track(&storage, "Öld_%/Disc/two.mp3", "Two", "Artist").await?;
        let unrelated_id =
            insert_track(&storage, "Old_AX/untouched.mp3", "Other", "Artist").await?;

        let rename = LibraryFileMutation::RenameFolder {
            source: LibraryPath::parse("Öld_%")?,
            destination: LibraryPath::parse("Néw_%")?,
        };
        let rename_journal = applying_journal(&storage, &rename).await?;
        let renamed = LibraryMutationRepository::commit_folder_rename(
            &storage,
            &rename_journal,
            &LibraryPath::parse("Öld_%")?,
            &LibraryPath::parse("Néw_%")?,
        )
        .await?;
        assert_eq!(renamed.affected_tracks, 2);
        assert_eq!(renamed.status.generation, LibraryGeneration::new(1));
        assert_eq!(renamed.status.discovered_tracks, 3);
        assert_eq!(
            LibraryRepository::track(&storage, TrackId::new(first_id)?)
                .await?
                .ok_or("renamed track disappeared")?
                .path
                .as_str(),
            "Néw_%/one.mp3"
        );
        assert_eq!(
            LibraryRepository::track(&storage, TrackId::new(nested_id)?)
                .await?
                .ok_or("nested track disappeared")?
                .path
                .as_str(),
            "Néw_%/Disc/two.mp3"
        );
        assert_eq!(
            LibraryRepository::track(&storage, TrackId::new(unrelated_id)?)
                .await?
                .ok_or("unrelated track disappeared")?
                .path
                .as_str(),
            "Old_AX/untouched.mp3"
        );

        let mismatched_delete = LibraryFileMutation::DeleteFolder {
            path: LibraryPath::parse("Old_AX")?,
            recursive: true,
        };
        let mismatched_journal = applying_journal(&storage, &mismatched_delete).await?;
        assert!(
            LibraryMutationRepository::commit_folder_delete(
                &storage,
                &mismatched_journal,
                &LibraryPath::parse("Néw_%")?,
            )
            .await
            .is_err()
        );
        assert!(
            LibraryRepository::track(&storage, TrackId::new(first_id)?)
                .await?
                .is_some()
        );
        assert!(matches!(
            RecoveryJournalRepository::transition_recovery_journal(
                &storage,
                &mismatched_journal,
                RecoveryState::Applying,
                RecoveryState::Failed,
                serde_json::json!({"error_code": "fixture_mismatch"}),
            )
            .await?,
            RecoveryTransition::Applied(_)
        ));

        let delete = LibraryFileMutation::DeleteFolder {
            path: LibraryPath::parse("Néw_%")?,
            recursive: true,
        };
        let delete_journal = applying_journal(&storage, &delete).await?;
        let deleted = LibraryMutationRepository::commit_folder_delete(
            &storage,
            &delete_journal,
            &LibraryPath::parse("Néw_%")?,
        )
        .await?;
        assert_eq!(deleted.affected_tracks, 2);
        assert_eq!(deleted.status.generation, LibraryGeneration::new(2));
        assert_eq!(deleted.status.discovered_tracks, 1);
        assert!(
            LibraryRepository::track(&storage, TrackId::new(first_id)?)
                .await?
                .is_none()
        );
        assert!(
            LibraryRepository::catalog_track_ids(&storage)
                .await?
                .contains(&TrackId::new(unrelated_id)?)
        );
        assert!(
            RecoveryJournalRepository::unfinished_recovery_journals(
                &storage,
                RecoveryDomain::Library,
            )
            .await?
            .is_empty()
        );
        Ok(())
    }

    #[tokio::test]
    async fn journaled_track_move_and_delete_preserve_then_remove_the_same_identity()
    -> Result<(), Box<dyn Error + Send + Sync>> {
        let directory = tempdir()?;
        let storage = SqliteStorage::open(SqliteStorageOptions::new(
            directory.path().join("track-mutations.db"),
        ))
        .await?;
        let raw_id = insert_track(&storage, "Source/song.mp3", "Song", "Artist").await?;
        let track_id = TrackId::new(raw_id)?;
        let move_mutation = LibraryFileMutation::MoveTrack {
            track_id,
            source: LibraryPath::parse("Source/song.mp3")?,
            destination: LibraryPath::parse("Archive/renamed.mp3")?,
        };
        let move_journal = applying_journal(&storage, &move_mutation).await?;
        let moved_discovery =
            discovered("Archive/renamed.mp3", "Renamed song", "Artist", 321, 654)?;
        let moved = LibraryMutationRepository::commit_track_move(
            &storage,
            &move_journal,
            track_id,
            &LibraryPath::parse("Source/song.mp3")?,
            &moved_discovery,
        )
        .await?;
        assert_eq!(moved.track.id, track_id);
        assert_eq!(moved.track.path.as_str(), "Archive/renamed.mp3");
        assert_eq!(moved.track.metadata.title, "Renamed song");
        assert_eq!(moved.status.generation, LibraryGeneration::new(1));
        assert!(
            LibraryRepository::catalog_track_ids(&storage)
                .await?
                .contains(&track_id)
        );

        let delete_mutation = LibraryFileMutation::DeleteTrack {
            track_id,
            path: LibraryPath::parse("Archive/renamed.mp3")?,
        };
        let delete_journal = applying_journal(&storage, &delete_mutation).await?;
        let deleted = LibraryMutationRepository::commit_track_delete(
            &storage,
            &delete_journal,
            track_id,
            &LibraryPath::parse("Archive/renamed.mp3")?,
        )
        .await?;
        assert_eq!(deleted.affected_tracks, 1);
        assert_eq!(deleted.status.generation, LibraryGeneration::new(2));
        assert!(
            !LibraryRepository::catalog_track_ids(&storage)
                .await?
                .contains(&track_id)
        );
        assert!(
            LibraryRepository::track(&storage, track_id)
                .await?
                .is_none()
        );
        Ok(())
    }

    #[tokio::test]
    async fn journaled_metadata_updates_commit_file_and_database_fields_atomically()
    -> Result<(), Box<dyn Error + Send + Sync>> {
        let directory = tempdir()?;
        let storage = SqliteStorage::open(SqliteStorageOptions::new(
            directory.path().join("track-metadata.db"),
        ))
        .await?;
        let raw_id = insert_track(&storage, "Album/song.wav", "Song", "Artist").await?;
        let track_id = TrackId::new(raw_id)?;

        let mut mixed_patch = TrackMetadataPatch::new();
        mixed_patch.insert_text(TrackMetadataField::Title, Some("Retagged song".to_owned()))?;
        mixed_patch.insert_text(
            TrackMetadataField::DisplayTitle,
            Some("Battle cue".to_owned()),
        )?;
        mixed_patch.insert_text(TrackMetadataField::Origin, None)?;
        let mixed_mutation = LibraryFileMutation::UpdateTrackMetadata {
            track_id,
            path: LibraryPath::parse("Album/song.wav")?,
            patch: mixed_patch.clone(),
        };
        let mixed_journal = applying_journal(&storage, &mixed_mutation).await?;
        let retagged = discovered("Album/song.wav", "Retagged song", "Artist", 987, 765)?;
        let mixed = LibraryMutationRepository::commit_track_metadata(
            &storage,
            &mixed_journal,
            track_id,
            &LibraryPath::parse("Album/song.wav")?,
            &mixed_patch,
            Some(&retagged),
        )
        .await?;
        assert_eq!(mixed.track.id, track_id);
        assert_eq!(mixed.track.metadata.title, "Retagged song");
        assert_eq!(mixed.track.display_title, "Battle cue");
        assert_eq!(mixed.track.origin, "");
        assert_eq!(mixed.track.size_bytes, 987);
        assert_eq!(mixed.status.generation, LibraryGeneration::new(1));
        assert_eq!(mixed.status.discovered_tracks, 1);

        let mut database_patch = TrackMetadataPatch::new();
        database_patch.insert_text(TrackMetadataField::DisplayTitle, None)?;
        let database_mutation = LibraryFileMutation::UpdateTrackMetadata {
            track_id,
            path: LibraryPath::parse("Album/song.wav")?,
            patch: database_patch.clone(),
        };
        let database_journal = applying_journal(&storage, &database_mutation).await?;
        let cleared = LibraryMutationRepository::commit_track_metadata(
            &storage,
            &database_journal,
            track_id,
            &LibraryPath::parse("Album/song.wav")?,
            &database_patch,
            None,
        )
        .await?;
        assert_eq!(cleared.track.metadata.title, "Retagged song");
        assert_eq!(cleared.track.display_title, "");
        assert_eq!(cleared.status.generation, LibraryGeneration::new(2));
        assert_eq!(cleared.status.discovered_tracks, 1);
        assert!(
            RecoveryJournalRepository::unfinished_recovery_journals(
                &storage,
                RecoveryDomain::Library,
            )
            .await?
            .is_empty()
        );
        Ok(())
    }

    #[tokio::test]
    async fn journaled_uploads_insert_replace_and_ignore_non_audio_membership()
    -> Result<(), Box<dyn Error + Send + Sync>> {
        let directory = tempdir()?;
        let storage = SqliteStorage::open(SqliteStorageOptions::new(
            directory.path().join("uploads.db"),
        ))
        .await?;
        let staged = LibraryPath::parse("Uploads/.upload-first.partial")?;
        let destination = LibraryPath::parse("Uploads/song.wav")?;
        let first_mutation = LibraryFileMutation::PublishUpload {
            staged: staged.clone(),
            destination: destination.clone(),
            replace_existing: false,
        };
        let first_journal = applying_journal(&storage, &first_mutation).await?;
        let first_discovered = discovered("Uploads/song.wav", "First", "Artist", 100, 200)?;
        let first = LibraryMutationRepository::commit_upload(
            &storage,
            &first_journal,
            &staged,
            &destination,
            false,
            Some(&first_discovered),
        )
        .await?;
        let track_id = first
            .track
            .as_ref()
            .ok_or("uploaded track was not indexed")?
            .id;
        assert_eq!(first.affected_tracks, 1);
        assert_eq!(first.status.discovered_tracks, 1);
        assert_eq!(first.status.generation, LibraryGeneration::new(1));

        let replacement_stage = LibraryPath::parse("Uploads/.upload-second.partial")?;
        let replacement_mutation = LibraryFileMutation::PublishUpload {
            staged: replacement_stage.clone(),
            destination: destination.clone(),
            replace_existing: true,
        };
        let replacement_journal = applying_journal(&storage, &replacement_mutation).await?;
        let replacement = discovered("Uploads/song.wav", "Replacement", "Artist", 300, 400)?;
        let replaced = LibraryMutationRepository::commit_upload(
            &storage,
            &replacement_journal,
            &replacement_stage,
            &destination,
            true,
            Some(&replacement),
        )
        .await?;
        let replaced_track = replaced
            .track
            .as_ref()
            .ok_or("replacement track was not indexed")?;
        assert_eq!(replaced_track.id, track_id);
        assert_eq!(replaced_track.metadata.title, "Replacement");
        assert_eq!(replaced.status.discovered_tracks, 1);
        assert_eq!(replaced.status.generation, LibraryGeneration::new(2));

        let text_stage = LibraryPath::parse("Uploads/.upload-text.partial")?;
        let text_destination = LibraryPath::parse("Uploads/notes.txt")?;
        let text_mutation = LibraryFileMutation::PublishUpload {
            staged: text_stage.clone(),
            destination: text_destination.clone(),
            replace_existing: false,
        };
        let text_journal = applying_journal(&storage, &text_mutation).await?;
        let text = LibraryMutationRepository::commit_upload(
            &storage,
            &text_journal,
            &text_stage,
            &text_destination,
            false,
            None,
        )
        .await?;
        assert!(text.track.is_none());
        assert_eq!(text.affected_tracks, 0);
        assert_eq!(text.status.discovered_tracks, 1);
        assert_eq!(text.status.generation, LibraryGeneration::new(2));
        Ok(())
    }
}
