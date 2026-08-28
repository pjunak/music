use std::collections::{BTreeMap, BTreeSet};

use music_application::playlists::{
    AutomaticMaterialization, AutomaticPlaylistRule, AutomaticPlaylistSource, AutomaticSourceTrack,
    AutomaticTagSources, MAX_PLAYLIST_ITEMS, PatchValue, PlaylistCreate, PlaylistDependencyError,
    PlaylistFilter, PlaylistFuture, PlaylistItemRecord, PlaylistItems, PlaylistMutation,
    PlaylistPatch, PlaylistRecord, PlaylistRepository, resolve_automatic_playlist,
};
use music_domain::{IndexedTrack, TrackId};
use serde_json::Value;
use sha2::{Digest, Sha256};
use sqlx::sqlite::SqliteRow;
use sqlx::{QueryBuilder, Row, Sqlite, Transaction};

use crate::library::{TRACK_COLUMNS, indexed_track_from_row};
use crate::{SqliteStorage, StorageError};

const PLAYLIST_COLUMNS: &str = "id, name, mode_id, category, automatic_rule_json, \
    automatic_source_signature, CAST(strftime('%s', automatic_refreshed_at) AS INTEGER) \
    AS automatic_refreshed_at_unix_seconds, \
    CAST(strftime('%s', created_at) AS INTEGER) AS created_at_unix_seconds, \
    CAST(strftime('%s', updated_at) AS INTEGER) AS updated_at_unix_seconds";
const LOCAL_METADATA_ANALYZER_ID: &str = "local-metadata/v1";

impl PlaylistRepository for SqliteStorage {
    fn create<'a>(&'a self, request: &'a PlaylistCreate) -> PlaylistFuture<'a, PlaylistRecord> {
        Box::pin(async move {
            let _admission = self.write_gate.lock().await;
            let mut transaction = self
                .pool
                .begin()
                .await
                .map_err(StorageError::from)
                .map_err(box_storage)?;
            let result = sqlx::query(
                "INSERT INTO playlists (name, mode_id, category, automatic_rule_json, created_at, updated_at) \
                 VALUES (?, ?, ?, '', CURRENT_TIMESTAMP, CURRENT_TIMESTAMP)",
            )
            .bind(&request.name)
            .bind(&request.mode_id)
            .bind(&request.category)
            .execute(&mut *transaction)
            .await
            .map_err(StorageError::from)
            .map_err(box_storage)?;
            let playlist = load_playlist(&mut transaction, result.last_insert_rowid())
                .await
                .map_err(box_storage)?
                .ok_or_else(|| {
                    box_storage(StorageError::InvalidLibraryState(
                        "created playlist disappeared",
                    ))
                })?;
            transaction
                .commit()
                .await
                .map_err(StorageError::from)
                .map_err(box_storage)?;
            Ok(playlist)
        })
    }

    fn list<'a>(&'a self, filter: &'a PlaylistFilter) -> PlaylistFuture<'a, Vec<PlaylistRecord>> {
        Box::pin(async move {
            let mut query =
                QueryBuilder::<Sqlite>::new(format!("SELECT {PLAYLIST_COLUMNS} FROM playlists"));
            let mut has_filter = false;
            if let Some(mode_id) = &filter.mode_id {
                query.push(" WHERE mode_id = ").push_bind(mode_id);
                has_filter = true;
            }
            if let Some(category) = &filter.category {
                query
                    .push(if has_filter {
                        " AND category = "
                    } else {
                        " WHERE category = "
                    })
                    .push_bind(category);
            }
            query.push(" ORDER BY created_at DESC, id DESC");
            let rows = query
                .build()
                .fetch_all(&self.pool)
                .await
                .map_err(StorageError::from)
                .map_err(box_storage)?;
            rows.iter()
                .map(playlist_from_row)
                .collect::<Result<Vec<_>, _>>()
                .map_err(box_storage)
        })
    }

    fn get(&self, playlist_id: i64) -> PlaylistFuture<'_, Option<PlaylistRecord>> {
        Box::pin(async move {
            load_playlist_pool(self, playlist_id)
                .await
                .map_err(box_storage)
        })
    }

    fn update<'a>(
        &'a self,
        playlist_id: i64,
        patch: &'a PlaylistPatch,
    ) -> PlaylistFuture<'a, Option<PlaylistRecord>> {
        Box::pin(async move {
            let _admission = self.write_gate.lock().await;
            let mut transaction = self
                .pool
                .begin()
                .await
                .map_err(StorageError::from)
                .map_err(box_storage)?;
            let Some(mut playlist) = load_playlist(&mut transaction, playlist_id)
                .await
                .map_err(box_storage)?
            else {
                transaction
                    .rollback()
                    .await
                    .map_err(StorageError::from)
                    .map_err(box_storage)?;
                return Ok(None);
            };
            let before = (
                playlist.name.clone(),
                playlist.mode_id.clone(),
                playlist.category.clone(),
            );
            if let PatchValue::Set(name) = &patch.name {
                playlist.name.clone_from(name);
            }
            if let PatchValue::Set(mode_id) = &patch.mode_id {
                playlist.mode_id = Some(mode_id.clone());
            }
            if let PatchValue::Set(category) = &patch.category {
                playlist.category.clone_from(category);
            }
            if before
                != (
                    playlist.name.clone(),
                    playlist.mode_id.clone(),
                    playlist.category.clone(),
                )
            {
                sqlx::query("UPDATE playlists SET name = ?, mode_id = ?, category = ?, updated_at = CURRENT_TIMESTAMP WHERE id = ?")
                    .bind(&playlist.name)
                    .bind(&playlist.mode_id)
                    .bind(&playlist.category)
                    .bind(playlist_id)
                    .execute(&mut *transaction)
                    .await
                    .map_err(StorageError::from)
                    .map_err(box_storage)?;
                playlist = load_playlist(&mut transaction, playlist_id)
                    .await
                    .map_err(box_storage)?
                    .ok_or_else(|| {
                        box_storage(StorageError::InvalidLibraryState(
                            "updated playlist disappeared",
                        ))
                    })?;
            }
            transaction
                .commit()
                .await
                .map_err(StorageError::from)
                .map_err(box_storage)?;
            Ok(Some(playlist))
        })
    }

    fn delete(&self, playlist_id: i64) -> PlaylistFuture<'_, bool> {
        Box::pin(async move {
            let _admission = self.write_gate.lock().await;
            let result = sqlx::query("DELETE FROM playlists WHERE id = ?")
                .bind(playlist_id)
                .execute(&self.pool)
                .await
                .map_err(StorageError::from)
                .map_err(box_storage)?;
            Ok(result.rows_affected() == 1)
        })
    }

    fn items(&self, playlist_id: i64) -> PlaylistFuture<'_, Option<PlaylistItems>> {
        Box::pin(async move {
            let _admission = self.write_gate.lock().await;
            let mut transaction = self
                .pool
                .begin()
                .await
                .map_err(StorageError::from)
                .map_err(box_storage)?;
            let Some(playlist) = load_playlist(&mut transaction, playlist_id)
                .await
                .map_err(box_storage)?
            else {
                transaction
                    .rollback()
                    .await
                    .map_err(StorageError::from)
                    .map_err(box_storage)?;
                return Ok(None);
            };
            let track_ids = ordered_track_ids(&mut transaction, playlist_id)
                .await
                .map_err(box_storage)?;
            repack_items_if_needed(&mut transaction, playlist_id, &track_ids)
                .await
                .map_err(box_storage)?;
            let tracks = load_tracks(&mut transaction, &track_ids)
                .await
                .map_err(box_storage)?;
            let items = track_ids
                .into_iter()
                .enumerate()
                .map(|(position, track_id)| PlaylistItemRecord {
                    position: i64::try_from(position).unwrap_or(i64::MAX),
                    track_id,
                    track: tracks.get(&track_id).cloned(),
                })
                .collect();
            transaction
                .commit()
                .await
                .map_err(StorageError::from)
                .map_err(box_storage)?;
            Ok(Some(PlaylistItems { playlist, items }))
        })
    }

    fn add_track(
        &self,
        playlist_id: i64,
        track_id: TrackId,
        position: Option<usize>,
    ) -> PlaylistFuture<'_, PlaylistMutation<PlaylistItemRecord>> {
        Box::pin(async move {
            let _admission = self.write_gate.lock().await;
            let mut transaction = self
                .pool
                .begin()
                .await
                .map_err(StorageError::from)
                .map_err(box_storage)?;
            let Some(playlist) = load_playlist(&mut transaction, playlist_id)
                .await
                .map_err(box_storage)?
            else {
                transaction
                    .rollback()
                    .await
                    .map_err(StorageError::from)
                    .map_err(box_storage)?;
                return Ok(PlaylistMutation::PlaylistNotFound);
            };
            if playlist.is_automatic() {
                transaction
                    .rollback()
                    .await
                    .map_err(StorageError::from)
                    .map_err(box_storage)?;
                return Ok(PlaylistMutation::AutomaticItemsManaged);
            }
            let Some(track) = load_track(&mut transaction, track_id.get())
                .await
                .map_err(box_storage)?
            else {
                transaction
                    .rollback()
                    .await
                    .map_err(StorageError::from)
                    .map_err(box_storage)?;
                return Ok(PlaylistMutation::TrackNotFound);
            };
            let mut track_ids = ordered_track_ids(&mut transaction, playlist_id)
                .await
                .map_err(box_storage)?;
            if track_ids.len() >= MAX_PLAYLIST_ITEMS {
                transaction
                    .rollback()
                    .await
                    .map_err(StorageError::from)
                    .map_err(box_storage)?;
                return Ok(PlaylistMutation::CapacityExceeded);
            }
            let target = position.unwrap_or(track_ids.len());
            if target > track_ids.len() {
                transaction
                    .rollback()
                    .await
                    .map_err(StorageError::from)
                    .map_err(box_storage)?;
                return Ok(PlaylistMutation::PositionOutOfRange);
            }
            track_ids.insert(target, track_id.get());
            replace_items(&mut transaction, playlist_id, &track_ids)
                .await
                .map_err(box_storage)?;
            transaction
                .commit()
                .await
                .map_err(StorageError::from)
                .map_err(box_storage)?;
            Ok(PlaylistMutation::Applied(PlaylistItemRecord {
                position: i64::try_from(target).unwrap_or(i64::MAX),
                track_id: track_id.get(),
                track: Some(track),
            }))
        })
    }

    fn remove_track(
        &self,
        playlist_id: i64,
        position: usize,
    ) -> PlaylistFuture<'_, PlaylistMutation<()>> {
        Box::pin(async move {
            mutate_items(self, playlist_id, move |track_ids| {
                if position >= track_ids.len() {
                    return Err(PlaylistMutation::PositionOutOfRange);
                }
                track_ids.remove(position);
                Ok(())
            })
            .await
        })
    }

    fn move_track(
        &self,
        playlist_id: i64,
        from_position: usize,
        to_position: usize,
    ) -> PlaylistFuture<'_, PlaylistMutation<()>> {
        Box::pin(async move {
            mutate_items(self, playlist_id, move |track_ids| {
                if from_position >= track_ids.len() || to_position >= track_ids.len() {
                    return Err(PlaylistMutation::PositionOutOfRange);
                }
                if from_position != to_position {
                    let track_id = track_ids.remove(from_position);
                    track_ids.insert(to_position, track_id);
                }
                Ok(())
            })
            .await
        })
    }

    fn automatic_source(
        &self,
        tag_sources: AutomaticTagSources,
    ) -> PlaylistFuture<'_, AutomaticPlaylistSource> {
        Box::pin(async move {
            let mut transaction = self
                .pool
                .begin()
                .await
                .map_err(StorageError::from)
                .map_err(box_storage)?;
            let source = load_automatic_source(&mut transaction, tag_sources)
                .await
                .map_err(box_storage)?;
            transaction
                .commit()
                .await
                .map_err(StorageError::from)
                .map_err(box_storage)?;
            Ok(source)
        })
    }

    fn materialize_automatic<'a>(
        &'a self,
        playlist_id: i64,
        rule: &'a AutomaticPlaylistRule,
        expected_source_signature: Option<&'a str>,
        force: bool,
    ) -> PlaylistFuture<'a, AutomaticMaterialization> {
        Box::pin(async move {
            let _admission = self.write_gate.lock().await;
            let mut transaction = self
                .pool
                .begin()
                .await
                .map_err(StorageError::from)
                .map_err(box_storage)?;
            let Some(current) = load_playlist(&mut transaction, playlist_id)
                .await
                .map_err(box_storage)?
            else {
                transaction
                    .rollback()
                    .await
                    .map_err(StorageError::from)
                    .map_err(box_storage)?;
                return Ok(AutomaticMaterialization::PlaylistNotFound);
            };
            if expected_source_signature.is_none()
                && current.automatic_rule().ok().flatten().as_ref() != Some(rule)
            {
                transaction
                    .rollback()
                    .await
                    .map_err(StorageError::from)
                    .map_err(box_storage)?;
                return Ok(AutomaticMaterialization::RuleChanged);
            }
            let source = load_automatic_source(&mut transaction, rule.tag_sources)
                .await
                .map_err(box_storage)?;
            let resolution = resolve_automatic_playlist(rule, source).map_err(|_| {
                box_storage(StorageError::InvalidLibraryState(
                    "automatic playlist rule is invalid",
                ))
            })?;
            if expected_source_signature
                .is_some_and(|expected| expected != resolution.source_signature)
            {
                transaction
                    .rollback()
                    .await
                    .map_err(StorageError::from)
                    .map_err(box_storage)?;
                return Ok(AutomaticMaterialization::StalePreview);
            }
            if !force
                && current.automatic_source_signature.as_deref()
                    == Some(&resolution.source_signature)
            {
                transaction
                    .commit()
                    .await
                    .map_err(StorageError::from)
                    .map_err(box_storage)?;
                return Ok(AutomaticMaterialization::Unchanged {
                    playlist: current,
                    resolution,
                });
            }
            let track_ids = resolution
                .tracks
                .iter()
                .map(|track| track.id.get())
                .collect::<Vec<_>>();
            replace_items(&mut transaction, playlist_id, &track_ids)
                .await
                .map_err(box_storage)?;
            let rule_json = serde_json::to_string(rule)
                .map_err(|error| box_storage(StorageError::ManifestSerialization(error)))?;
            sqlx::query(
                "UPDATE playlists SET automatic_rule_json = ?, automatic_source_signature = ?, \
                 automatic_refreshed_at = CURRENT_TIMESTAMP, updated_at = CURRENT_TIMESTAMP WHERE id = ?",
            )
            .bind(rule_json)
            .bind(&resolution.source_signature)
            .bind(playlist_id)
            .execute(&mut *transaction)
            .await
            .map_err(StorageError::from)
            .map_err(box_storage)?;
            let playlist = load_playlist(&mut transaction, playlist_id)
                .await
                .map_err(box_storage)?
                .ok_or_else(|| {
                    box_storage(StorageError::InvalidLibraryState(
                        "materialized playlist disappeared",
                    ))
                })?;
            transaction
                .commit()
                .await
                .map_err(StorageError::from)
                .map_err(box_storage)?;
            Ok(AutomaticMaterialization::Applied {
                playlist,
                resolution,
            })
        })
    }

    fn disable_automatic(&self, playlist_id: i64) -> PlaylistFuture<'_, Option<PlaylistRecord>> {
        Box::pin(async move {
            let _admission = self.write_gate.lock().await;
            let mut transaction = self
                .pool
                .begin()
                .await
                .map_err(StorageError::from)
                .map_err(box_storage)?;
            let updated = sqlx::query(
                "UPDATE playlists SET automatic_rule_json = '', automatic_source_signature = NULL, \
                 automatic_refreshed_at = NULL, updated_at = CURRENT_TIMESTAMP WHERE id = ?",
            )
            .bind(playlist_id)
            .execute(&mut *transaction)
            .await
            .map_err(StorageError::from)
            .map_err(box_storage)?;
            let playlist = if updated.rows_affected() == 1 {
                load_playlist(&mut transaction, playlist_id)
                    .await
                    .map_err(box_storage)?
            } else {
                None
            };
            transaction
                .commit()
                .await
                .map_err(StorageError::from)
                .map_err(box_storage)?;
            Ok(playlist)
        })
    }
}

async fn mutate_items(
    storage: &SqliteStorage,
    playlist_id: i64,
    mutate: impl FnOnce(&mut Vec<i64>) -> Result<(), PlaylistMutation<()>> + Send,
) -> Result<PlaylistMutation<()>, PlaylistDependencyError> {
    let _admission = storage.write_gate.lock().await;
    let mut transaction = storage
        .pool
        .begin()
        .await
        .map_err(StorageError::from)
        .map_err(box_storage)?;
    let Some(playlist) = load_playlist(&mut transaction, playlist_id)
        .await
        .map_err(box_storage)?
    else {
        transaction
            .rollback()
            .await
            .map_err(StorageError::from)
            .map_err(box_storage)?;
        return Ok(PlaylistMutation::PlaylistNotFound);
    };
    if playlist.is_automatic() {
        transaction
            .rollback()
            .await
            .map_err(StorageError::from)
            .map_err(box_storage)?;
        return Ok(PlaylistMutation::AutomaticItemsManaged);
    }
    let mut track_ids = ordered_track_ids(&mut transaction, playlist_id)
        .await
        .map_err(box_storage)?;
    if let Err(outcome) = mutate(&mut track_ids) {
        transaction
            .rollback()
            .await
            .map_err(StorageError::from)
            .map_err(box_storage)?;
        return Ok(outcome);
    }
    replace_items(&mut transaction, playlist_id, &track_ids)
        .await
        .map_err(box_storage)?;
    transaction
        .commit()
        .await
        .map_err(StorageError::from)
        .map_err(box_storage)?;
    Ok(PlaylistMutation::Applied(()))
}

async fn load_playlist_pool(
    storage: &SqliteStorage,
    playlist_id: i64,
) -> Result<Option<PlaylistRecord>, StorageError> {
    let mut query = QueryBuilder::<Sqlite>::new(format!(
        "SELECT {PLAYLIST_COLUMNS} FROM playlists WHERE id = "
    ));
    query.push_bind(playlist_id);
    let row = query.build().fetch_optional(&storage.pool).await?;
    row.as_ref().map(playlist_from_row).transpose()
}

async fn load_playlist(
    transaction: &mut Transaction<'_, Sqlite>,
    playlist_id: i64,
) -> Result<Option<PlaylistRecord>, StorageError> {
    let mut query = QueryBuilder::<Sqlite>::new(format!(
        "SELECT {PLAYLIST_COLUMNS} FROM playlists WHERE id = "
    ));
    query.push_bind(playlist_id);
    let row = query.build().fetch_optional(&mut **transaction).await?;
    row.as_ref().map(playlist_from_row).transpose()
}

fn playlist_from_row(row: &SqliteRow) -> Result<PlaylistRecord, StorageError> {
    Ok(PlaylistRecord {
        id: row.try_get("id")?,
        name: row.try_get("name")?,
        mode_id: row.try_get("mode_id")?,
        category: row.try_get("category")?,
        automatic_rule_json: row.try_get("automatic_rule_json")?,
        automatic_source_signature: row.try_get("automatic_source_signature")?,
        automatic_refreshed_at_unix_seconds: row.try_get("automatic_refreshed_at_unix_seconds")?,
        created_at_unix_seconds: row
            .try_get::<Option<i64>, _>("created_at_unix_seconds")?
            .ok_or(StorageError::InvalidTimestamp)?,
        updated_at_unix_seconds: row
            .try_get::<Option<i64>, _>("updated_at_unix_seconds")?
            .ok_or(StorageError::InvalidTimestamp)?,
    })
}

async fn ordered_track_ids(
    transaction: &mut Transaction<'_, Sqlite>,
    playlist_id: i64,
) -> Result<Vec<i64>, StorageError> {
    let rows = sqlx::query(
        "SELECT position, track_id FROM playlist_items WHERE playlist_id = ? ORDER BY position",
    )
    .bind(playlist_id)
    .fetch_all(&mut **transaction)
    .await?;
    if rows.len() > MAX_PLAYLIST_ITEMS {
        return Err(StorageError::InvalidLibraryState(
            "playlist exceeds the supported item capacity",
        ));
    }
    rows.iter()
        .map(|row| row.try_get("track_id").map_err(StorageError::from))
        .collect()
}

async fn repack_items_if_needed(
    transaction: &mut Transaction<'_, Sqlite>,
    playlist_id: i64,
    track_ids: &[i64],
) -> Result<(), StorageError> {
    let positions = sqlx::query_scalar::<_, i64>(
        "SELECT position FROM playlist_items WHERE playlist_id = ? ORDER BY position",
    )
    .bind(playlist_id)
    .fetch_all(&mut **transaction)
    .await?;
    let contiguous = positions
        .iter()
        .enumerate()
        .all(|(expected, actual)| i64::try_from(expected).ok() == Some(*actual));
    if !contiguous {
        replace_items(transaction, playlist_id, track_ids).await?;
    }
    Ok(())
}

async fn replace_items(
    transaction: &mut Transaction<'_, Sqlite>,
    playlist_id: i64,
    track_ids: &[i64],
) -> Result<(), StorageError> {
    sqlx::query("DELETE FROM playlist_items WHERE playlist_id = ?")
        .bind(playlist_id)
        .execute(&mut **transaction)
        .await?;
    for (position, track_id) in track_ids.iter().enumerate() {
        let position = i64::try_from(position).map_err(|_| {
            StorageError::InvalidLibraryState("playlist position exceeds SQLite range")
        })?;
        sqlx::query("INSERT INTO playlist_items (playlist_id, position, track_id, added_at) VALUES (?, ?, ?, CURRENT_TIMESTAMP)")
            .bind(playlist_id)
            .bind(position)
            .bind(track_id)
            .execute(&mut **transaction)
            .await?;
    }
    Ok(())
}

async fn load_tracks(
    transaction: &mut Transaction<'_, Sqlite>,
    track_ids: &[i64],
) -> Result<BTreeMap<i64, IndexedTrack>, StorageError> {
    if track_ids.is_empty() {
        return Ok(BTreeMap::new());
    }
    let unique = track_ids.iter().copied().collect::<BTreeSet<_>>();
    let mut query =
        QueryBuilder::<Sqlite>::new(format!("SELECT {TRACK_COLUMNS} FROM tracks WHERE id IN ("));
    let mut separated = query.separated(", ");
    for track_id in unique {
        separated.push_bind(track_id);
    }
    query.push(")");
    let rows = query.build().fetch_all(&mut **transaction).await?;
    rows.iter()
        .map(|row| indexed_track_from_row(row).map(|track| (track.id.get(), track)))
        .collect()
}

async fn load_track(
    transaction: &mut Transaction<'_, Sqlite>,
    track_id: i64,
) -> Result<Option<IndexedTrack>, StorageError> {
    let mut query =
        QueryBuilder::<Sqlite>::new(format!("SELECT {TRACK_COLUMNS} FROM tracks WHERE id = "));
    query.push_bind(track_id);
    let row = query.build().fetch_optional(&mut **transaction).await?;
    row.as_ref().map(indexed_track_from_row).transpose()
}

async fn load_automatic_source(
    transaction: &mut Transaction<'_, Sqlite>,
    tag_sources: AutomaticTagSources,
) -> Result<AutomaticPlaylistSource, StorageError> {
    let mut query =
        QueryBuilder::<Sqlite>::new(format!("SELECT {TRACK_COLUMNS} FROM tracks ORDER BY id"));
    let rows = query.build().fetch_all(&mut **transaction).await?;
    let tracks = rows
        .iter()
        .map(indexed_track_from_row)
        .collect::<Result<Vec<_>, _>>()?;
    let mut tags = BTreeMap::<i64, BTreeSet<String>>::new();
    let manual_rows =
        sqlx::query("SELECT track_id, tag FROM track_user_tags ORDER BY track_id, tag")
            .fetch_all(&mut **transaction)
            .await?;
    for row in manual_rows {
        tags.entry(row.try_get("track_id")?)
            .or_default()
            .insert(row.try_get("tag")?);
    }
    if tag_sources == AutomaticTagSources::ManualAndLocal {
        add_current_local_tags(transaction, &tracks, &mut tags).await?;
    }
    Ok(AutomaticPlaylistSource {
        tracks: tracks
            .into_iter()
            .map(|track| {
                let effective = tags.remove(&track.id.get()).unwrap_or_default();
                AutomaticSourceTrack {
                    track,
                    tags: effective,
                }
            })
            .collect(),
    })
}

async fn add_current_local_tags(
    transaction: &mut Transaction<'_, Sqlite>,
    tracks: &[IndexedTrack],
    tags: &mut BTreeMap<i64, BTreeSet<String>>,
) -> Result<(), StorageError> {
    let analysis_rows = sqlx::query(
        "SELECT track_id, source_signature, moods_json, evidence_json, confidence \
         FROM track_analyses WHERE analyzer_id = ? ORDER BY track_id",
    )
    .bind(LOCAL_METADATA_ANALYZER_ID)
    .fetch_all(&mut **transaction)
    .await?;
    let review_rows = sqlx::query(
        "SELECT track_id, source_signature, tag FROM track_analysis_tag_reviews \
         WHERE analyzer_id = ? AND decision = 'rejected'",
    )
    .bind(LOCAL_METADATA_ANALYZER_ID)
    .fetch_all(&mut **transaction)
    .await?;
    let rejected = review_rows
        .iter()
        .map(|row| {
            Ok((
                row.try_get::<i64, _>("track_id")?,
                row.try_get::<String, _>("source_signature")?,
                row.try_get::<String, _>("tag")?,
            ))
        })
        .collect::<Result<BTreeSet<_>, sqlx::Error>>()?;
    let by_id = tracks
        .iter()
        .map(|track| (track.id.get(), track))
        .collect::<BTreeMap<_, _>>();
    for row in analysis_rows {
        let track_id: i64 = row.try_get("track_id")?;
        let Some(track) = by_id.get(&track_id) else {
            continue;
        };
        let signature: String = row.try_get("source_signature")?;
        if signature != metadata_source_signature(track)? {
            continue;
        }
        let confidence: String = row.try_get("confidence")?;
        if !matches!(confidence.as_str(), "high" | "medium" | "low") {
            continue;
        }
        let moods = string_array(row.try_get("moods_json")?);
        let evidence = string_array(row.try_get("evidence_json")?);
        let (Some(moods), Some(_)) = (moods, evidence) else {
            continue;
        };
        let effective = tags.entry(track_id).or_default();
        for mood in moods {
            if !rejected.contains(&(track_id, signature.clone(), mood.clone())) {
                effective.insert(mood);
            }
        }
    }
    Ok(())
}

fn string_array(encoded: String) -> Option<Vec<String>> {
    let value = serde_json::from_str::<Value>(&encoded).ok()?;
    value
        .as_array()?
        .iter()
        .map(|item| item.as_str().map(ToOwned::to_owned))
        .collect()
}

fn metadata_source_signature(track: &IndexedTrack) -> Result<String, StorageError> {
    let payload = serde_json::json!([
        track.path.as_str(),
        track.metadata.title,
        track.display_title,
        track.metadata.artist,
        track.metadata.album,
        track.origin,
        track.metadata.genre,
        track.metadata.bpm,
    ]);
    let encoded = serde_json::to_vec(&payload).map_err(StorageError::ManifestSerialization)?;
    Ok(format!("{:x}", Sha256::digest(encoded)))
}

fn box_storage(source: StorageError) -> PlaylistDependencyError {
    Box::new(source)
}

#[cfg(test)]
mod tests {
    use std::error::Error;
    use std::sync::Arc;

    use music_application::playlists::{
        AUTOMATIC_RULE_SCHEMA, AutomaticMatch, AutomaticOrder, PlaylistService,
    };
    use tempfile::TempDir;

    use super::*;
    use crate::SqliteStorageOptions;

    async fn storage() -> Result<(TempDir, SqliteStorage), Box<dyn Error + Send + Sync>> {
        let directory = tempfile::tempdir()?;
        let storage =
            SqliteStorage::open(SqliteStorageOptions::new(directory.path().join("music.db")))
                .await?;
        sqlx::query("INSERT INTO tracks (path, title, artist, album_artist, album, track_no, disc_no, year, genre, length_s, bpm, display_title, origin, size_bytes, mtime, added_at) VALUES ('a.mp3', 'A', '', '', '', NULL, NULL, NULL, '', 60.0, 90, 'A', '', 1, 1, CURRENT_TIMESTAMP), ('b.mp3', 'B', '', '', '', NULL, NULL, NULL, '', 60.0, 120, 'B', '', 1, 1, CURRENT_TIMESTAMP), ('c.mp3', 'C', '', '', '', NULL, NULL, NULL, '', 60.0, NULL, 'C', '', 1, 1, CURRENT_TIMESTAMP)")
            .execute(&storage.pool).await?;
        Ok((directory, storage))
    }

    #[tokio::test]
    async fn playlist_filters_bind_each_clause_without_corrupting_sql()
    -> Result<(), Box<dyn Error + Send + Sync>> {
        let (_directory, storage) = storage().await?;
        for (name, mode_id, category) in [
            ("Table ambience", Some("table"), Some("ambient")),
            ("Table combat", Some("table"), Some("combat")),
            ("Other ambience", Some("other"), Some("ambient")),
        ] {
            storage
                .create(&PlaylistCreate {
                    name: name.to_owned(),
                    mode_id: mode_id.map(str::to_owned),
                    category: category.map(str::to_owned),
                })
                .await?;
        }

        assert_eq!(
            storage
                .list(&PlaylistFilter {
                    mode_id: Some("table".to_owned()),
                    category: None,
                })
                .await?
                .len(),
            2
        );
        assert_eq!(
            storage
                .list(&PlaylistFilter {
                    mode_id: None,
                    category: Some("ambient".to_owned()),
                })
                .await?
                .len(),
            2
        );
        assert_eq!(
            storage
                .list(&PlaylistFilter {
                    mode_id: Some("table".to_owned()),
                    category: Some("ambient".to_owned()),
                })
                .await?
                .len(),
            1
        );
        assert_eq!(storage.list(&PlaylistFilter::default()).await?.len(), 3);
        Ok(())
    }

    #[tokio::test]
    async fn item_mutations_are_contiguous_and_automatic_materialization_is_atomic()
    -> Result<(), Box<dyn Error + Send + Sync>> {
        let (_directory, storage) = storage().await?;
        let playlist = storage
            .create(&PlaylistCreate {
                name: "Test".to_owned(),
                mode_id: None,
                category: None,
            })
            .await?;
        storage
            .add_track(playlist.id, TrackId::new(1)?, None)
            .await?;
        storage
            .add_track(playlist.id, TrackId::new(2)?, Some(0))
            .await?;
        storage.move_track(playlist.id, 0, 1).await?;
        let items = storage
            .items(playlist.id)
            .await?
            .ok_or("playlist missing")?;
        assert_eq!(
            items
                .items
                .iter()
                .map(|item| item.track_id)
                .collect::<Vec<_>>(),
            [1, 2]
        );

        storage
            .add_track(playlist.id, TrackId::new(1)?, None)
            .await?;
        sqlx::query("DELETE FROM tracks WHERE id = 2")
            .execute(&storage.pool)
            .await?;
        storage
            .add_track(playlist.id, TrackId::new(3)?, Some(1))
            .await?;
        let items = storage
            .items(playlist.id)
            .await?
            .ok_or("playlist missing")?;
        assert_eq!(
            items
                .items
                .iter()
                .map(|item| item.track_id)
                .collect::<Vec<_>>(),
            [1, 3, 1]
        );
        assert_eq!(
            items
                .items
                .iter()
                .map(|item| item.position)
                .collect::<Vec<_>>(),
            [0, 1, 2]
        );

        sqlx::query("INSERT INTO track_user_tags (track_id, tag, created_at) VALUES (1, 'calm', CURRENT_TIMESTAMP)")
            .execute(&storage.pool).await?;
        let rule = AutomaticPlaylistRule {
            schema_version: AUTOMATIC_RULE_SCHEMA.to_owned(),
            include_tags: vec!["calm".to_owned()],
            r#match: AutomaticMatch::Any,
            exclude_tags: Vec::new(),
            tag_sources: AutomaticTagSources::Manual,
            min_bpm: None,
            max_bpm: None,
            include_unknown_bpm: true,
            maximum_tracks: 200,
            order_by: AutomaticOrder::Title,
        };
        let preview =
            resolve_automatic_playlist(&rule, storage.automatic_source(rule.tag_sources).await?)?;
        let materialized = storage
            .materialize_automatic(playlist.id, &rule, Some(&preview.source_signature), true)
            .await?;
        assert!(matches!(
            materialized,
            AutomaticMaterialization::Applied { .. }
        ));
        assert_eq!(
            storage
                .items(playlist.id)
                .await?
                .ok_or("playlist missing")?
                .items
                .len(),
            1
        );
        assert_eq!(
            storage
                .add_track(playlist.id, TrackId::new(2)?, None)
                .await?,
            PlaylistMutation::AutomaticItemsManaged
        );
        Ok(())
    }

    #[tokio::test]
    async fn local_analysis_tags_require_current_metadata_and_respect_rejections()
    -> Result<(), Box<dyn Error + Send + Sync>> {
        let (_directory, storage) = storage().await?;
        let manual = storage
            .automatic_source(AutomaticTagSources::Manual)
            .await?;
        let track = &manual
            .tracks
            .iter()
            .find(|candidate| candidate.track.id.get() == 1)
            .ok_or("track missing")?
            .track;
        let signature = metadata_source_signature(track)?;
        sqlx::query(
            "INSERT INTO track_analyses (track_id, analyzer_id, source_signature, job_id, energy, brightness, tension, moods_json, evidence_json, metrics_json, confidence, updated_at) \
             VALUES (?, ?, ?, 'playlist-test', 0.4, 0.3, 0.2, '[\"dreamy\"]', '[\"synthetic fixture\"]', '{}', 'high', CURRENT_TIMESTAMP)",
        )
        .bind(1_i64)
        .bind(LOCAL_METADATA_ANALYZER_ID)
        .bind(&signature)
        .execute(&storage.pool)
        .await?;

        let manual = storage
            .automatic_source(AutomaticTagSources::Manual)
            .await?;
        assert!(manual.tracks[0].tags.is_empty());
        let local = storage
            .automatic_source(AutomaticTagSources::ManualAndLocal)
            .await?;
        assert!(local.tracks[0].tags.contains("dreamy"));

        sqlx::query(
            "INSERT INTO track_analysis_tag_reviews (track_id, analyzer_id, tag, source_signature, decision, reviewed_at) \
             VALUES (?, ?, 'dreamy', ?, 'rejected', CURRENT_TIMESTAMP)",
        )
        .bind(1_i64)
        .bind(LOCAL_METADATA_ANALYZER_ID)
        .bind(&signature)
        .execute(&storage.pool)
        .await?;
        let rejected = storage
            .automatic_source(AutomaticTagSources::ManualAndLocal)
            .await?;
        assert!(!rejected.tracks[0].tags.contains("dreamy"));

        sqlx::query(
            "UPDATE track_analysis_tag_reviews SET decision = 'accepted' WHERE track_id = 1",
        )
        .execute(&storage.pool)
        .await?;
        sqlx::query("UPDATE tracks SET title = 'Changed' WHERE id = 1")
            .execute(&storage.pool)
            .await?;
        let stale = storage
            .automatic_source(AutomaticTagSources::ManualAndLocal)
            .await?;
        assert!(!stale.tracks[0].tags.contains("dreamy"));
        Ok(())
    }

    #[tokio::test]
    async fn invalid_stored_rule_keeps_last_good_materialized_rows()
    -> Result<(), Box<dyn Error + Send + Sync>> {
        let (_directory, storage) = storage().await?;
        let playlist = storage
            .create(&PlaylistCreate {
                name: "Damaged automatic rule".to_owned(),
                mode_id: None,
                category: None,
            })
            .await?;
        storage
            .add_track(playlist.id, TrackId::new(1)?, None)
            .await?;
        sqlx::query(
            "UPDATE playlists SET automatic_rule_json = ?, automatic_source_signature = ? WHERE id = ?",
        )
        .bind(r#"{"schema":"automatic-playlist/unsupported"}"#)
        .bind("0".repeat(64))
        .bind(playlist.id)
        .execute(&storage.pool)
        .await?;

        let service = PlaylistService::new(Arc::new(storage));
        let items = service.items(playlist.id).await?;
        assert_eq!(items.items.len(), 1);
        assert_eq!(items.items[0].track_id, 1);
        assert!(items.playlist.automatic_rule().is_err());
        Ok(())
    }

    #[tokio::test]
    async fn stale_preview_does_not_replace_last_good_items()
    -> Result<(), Box<dyn Error + Send + Sync>> {
        let (_directory, storage) = storage().await?;
        let playlist = storage
            .create(&PlaylistCreate {
                name: "Test".to_owned(),
                mode_id: None,
                category: None,
            })
            .await?;
        storage
            .add_track(playlist.id, TrackId::new(1)?, None)
            .await?;
        let rule = AutomaticPlaylistRule {
            schema_version: AUTOMATIC_RULE_SCHEMA.to_owned(),
            include_tags: Vec::new(),
            r#match: AutomaticMatch::Any,
            exclude_tags: Vec::new(),
            tag_sources: AutomaticTagSources::Manual,
            min_bpm: None,
            max_bpm: None,
            include_unknown_bpm: true,
            maximum_tracks: 200,
            order_by: AutomaticOrder::Title,
        };
        let outcome = storage
            .materialize_automatic(playlist.id, &rule, Some(&"0".repeat(64)), true)
            .await?;
        assert_eq!(outcome, AutomaticMaterialization::StalePreview);
        assert_eq!(
            storage
                .items(playlist.id)
                .await?
                .ok_or("playlist missing")?
                .items[0]
                .track_id,
            1
        );
        Ok(())
    }

    #[tokio::test]
    async fn stale_refresh_cannot_overwrite_a_newer_rule()
    -> Result<(), Box<dyn Error + Send + Sync>> {
        let (_directory, storage) = storage().await?;
        let playlist = storage
            .create(&PlaylistCreate {
                name: "Concurrent rule".to_owned(),
                mode_id: None,
                category: None,
            })
            .await?;
        let old_rule = AutomaticPlaylistRule {
            schema_version: AUTOMATIC_RULE_SCHEMA.to_owned(),
            include_tags: Vec::new(),
            r#match: AutomaticMatch::Any,
            exclude_tags: Vec::new(),
            tag_sources: AutomaticTagSources::Manual,
            min_bpm: None,
            max_bpm: None,
            include_unknown_bpm: true,
            maximum_tracks: 200,
            order_by: AutomaticOrder::Title,
        };
        let old_preview = resolve_automatic_playlist(
            &old_rule,
            storage.automatic_source(old_rule.tag_sources).await?,
        )?;
        storage
            .materialize_automatic(
                playlist.id,
                &old_rule,
                Some(&old_preview.source_signature),
                true,
            )
            .await?;

        let mut new_rule = old_rule.clone();
        new_rule.maximum_tracks = 1;
        let new_preview = resolve_automatic_playlist(
            &new_rule,
            storage.automatic_source(new_rule.tag_sources).await?,
        )?;
        storage
            .materialize_automatic(
                playlist.id,
                &new_rule,
                Some(&new_preview.source_signature),
                true,
            )
            .await?;

        assert_eq!(
            storage
                .materialize_automatic(playlist.id, &old_rule, None, false)
                .await?,
            AutomaticMaterialization::RuleChanged
        );
        assert_eq!(
            storage
                .get(playlist.id)
                .await?
                .ok_or("playlist missing")?
                .automatic_rule()?,
            Some(new_rule)
        );
        Ok(())
    }
}
