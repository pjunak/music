//! Validate each rename, tag edit and reversal against current indexed/file state.
//! These helpers prepare typed mutations; they do not apply filesystem changes.
use super::*;

impl LibraryCoordinator {
    pub(super) async fn prepare_cleanup_revert(
        &self,
        batch_id: Option<i64>,
        item_index: usize,
        item: &Map<String, Value>,
    ) -> Result<PreparedCleanupRevert, CleanupPreparationError> {
        let track_id = item.get("track_id").and_then(Value::as_i64).unwrap_or(0);
        match item.get("kind").and_then(Value::as_str) {
            Some("rename") => {
                self.prepare_cleanup_rename_revert(batch_id, item_index, track_id, item)
                    .await
            }
            Some("tag") => {
                self.prepare_cleanup_tag_revert(batch_id, item_index, track_id, item)
                    .await
            }
            Some("folder_rename") => {
                self.prepare_cleanup_folder_revert(batch_id, item_index, track_id, item)
            }
            kind => {
                Err(cleanup_skip(track_id, format!("unknown journal item kind: {kind:?}")).into())
            }
        }
    }

    async fn prepare_cleanup_rename_revert(
        &self,
        batch_id: Option<i64>,
        item_index: usize,
        recorded_track_id: i64,
        item: &Map<String, Value>,
    ) -> Result<PreparedCleanupRevert, CleanupPreparationError> {
        let (path_before, path_after) = cleanup_revert_paths(item)
            .map_err(|()| cleanup_skip(recorded_track_id, "malformed journal item"))?;
        let track = self
            .resolve_cleanup_revert_track(recorded_track_id, &path_after)
            .await?
            .ok_or_else(|| {
                cleanup_skip(
                    recorded_track_id,
                    "no track at the recorded path (renamed or removed since)",
                )
            })?;
        let original_name = path_before.file_name().to_owned();
        let revert = CleanupRevertMutation::new(batch_id, item_index).map_err(|error| {
            CleanupPreparationError::Fatal(LibraryCoordinatorError::InvalidCleanupMutation(error))
        })?;
        Ok(PreparedCleanupRevert {
            track_id: recorded_track_id,
            kind: PreparedCleanupRevertKind::Rename { original_name },
            mutation: LibraryFileMutation::MoveTrack {
                track_id: track.id,
                source: path_after,
                destination: path_before,
            },
            revert,
        })
    }

    async fn prepare_cleanup_tag_revert(
        &self,
        batch_id: Option<i64>,
        item_index: usize,
        recorded_track_id: i64,
        item: &Map<String, Value>,
    ) -> Result<PreparedCleanupRevert, CleanupPreparationError> {
        let field = item
            .get("field")
            .and_then(Value::as_str)
            .and_then(cleanup_tag_field)
            .ok_or_else(|| cleanup_skip(recorded_track_id, "malformed journal item"))?;
        let path = item
            .get("path")
            .and_then(Value::as_str)
            .ok_or(())
            .and_then(|path| LibraryPath::parse(path).map_err(|_| ()))
            .map_err(|()| cleanup_skip(recorded_track_id, "malformed journal item"))?;
        let track = self
            .resolve_cleanup_revert_track(recorded_track_id, &path)
            .await?
            .ok_or_else(|| {
                cleanup_skip(
                    recorded_track_id,
                    "no track at the recorded path (moved or removed since)",
                )
            })?;
        let expected = cleanup_input_from_json(item.get("new"))
            .map_err(|()| cleanup_skip(recorded_track_id, "malformed journal item"))?;
        let current = cleanup_track_value(&track, field);
        if !cleanup_values_match(current.as_ref(), expected.as_ref()) {
            return Err(cleanup_skip(
                recorded_track_id,
                format!("{} changed since this batch was applied", field.as_str()),
            )
            .into());
        }
        let restore_value = if item.contains_key("file_old") {
            item.get("file_old")
        } else {
            item.get("old")
        };
        let restore = cleanup_input_from_json(restore_value)
            .map_err(|()| cleanup_skip(recorded_track_id, "malformed journal item"))?;
        let patch = cleanup_tag_patch(field, restore.as_ref())
            .map_err(|_| cleanup_skip(recorded_track_id, "malformed journal item"))?;
        let revert = CleanupRevertMutation::new(batch_id, item_index).map_err(|error| {
            CleanupPreparationError::Fatal(LibraryCoordinatorError::InvalidCleanupMutation(error))
        })?;
        Ok(PreparedCleanupRevert {
            track_id: recorded_track_id,
            kind: PreparedCleanupRevertKind::Tag,
            mutation: LibraryFileMutation::UpdateTrackMetadata {
                track_id: track.id,
                path,
                patch,
            },
            revert,
        })
    }

    fn prepare_cleanup_folder_revert(
        &self,
        batch_id: Option<i64>,
        item_index: usize,
        recorded_track_id: i64,
        item: &Map<String, Value>,
    ) -> Result<PreparedCleanupRevert, CleanupPreparationError> {
        let (path_before, path_after) = cleanup_revert_paths(item)
            .map_err(|()| cleanup_skip(recorded_track_id, "malformed journal item"))?;
        let original_name = path_before.file_name().to_owned();
        let revert = CleanupRevertMutation::new(batch_id, item_index).map_err(|error| {
            CleanupPreparationError::Fatal(LibraryCoordinatorError::InvalidCleanupMutation(error))
        })?;
        Ok(PreparedCleanupRevert {
            track_id: recorded_track_id,
            kind: PreparedCleanupRevertKind::FolderRename { original_name },
            mutation: LibraryFileMutation::RenameFolder {
                source: path_after,
                destination: path_before,
            },
            revert,
        })
    }

    async fn resolve_cleanup_revert_track(
        &self,
        recorded_track_id: i64,
        path: &LibraryPath,
    ) -> Result<Option<IndexedTrack>, CleanupPreparationError> {
        if let Ok(track_id) = TrackId::new(recorded_track_id) {
            let track = self.repository.track(track_id).await.map_err(|source| {
                CleanupPreparationError::Fatal(dependency(
                    "load a cleanup revert track by id",
                    source,
                ))
            })?;
            if track.as_ref().is_some_and(|track| &track.path == path) {
                return Ok(track);
            }
        }
        self.repository.track_by_path(path).await.map_err(|source| {
            CleanupPreparationError::Fatal(dependency(
                "load a cleanup revert track by path",
                source,
            ))
        })
    }

    pub(super) async fn prepare_cleanup_mutation(
        &self,
        batch_id: Option<i64>,
        scope_label: &str,
        operation: CleanupApplyOperation,
    ) -> Result<PreparedCleanupMutation, CleanupPreparationError> {
        match operation.kind {
            CleanupOperationKind::Rename => {
                self.prepare_cleanup_track_rename(batch_id, scope_label, operation)
                    .await
            }
            CleanupOperationKind::Tag => {
                self.prepare_cleanup_tag(batch_id, scope_label, operation)
                    .await
            }
            CleanupOperationKind::FolderRename => {
                self.prepare_cleanup_folder_rename(batch_id, scope_label, operation)
            }
        }
    }

    async fn prepare_cleanup_track_rename(
        &self,
        batch_id: Option<i64>,
        scope_label: &str,
        operation: CleanupApplyOperation,
    ) -> Result<PreparedCleanupMutation, CleanupPreparationError> {
        let track_id = TrackId::new(operation.track_id)
            .map_err(|_| cleanup_skip(operation.track_id, "track not found"))?;
        let track = self
            .repository
            .track(track_id)
            .await
            .map_err(|source| {
                CleanupPreparationError::Fatal(dependency(
                    "load a track for cleanup renaming",
                    source,
                ))
            })?
            .ok_or_else(|| cleanup_skip(operation.track_id, "track not found"))?;
        let current_stem = Path::new(track.path.file_name())
            .file_stem()
            .and_then(|value| value.to_str())
            .unwrap_or_default();
        if !cleanup_expected_text_matches(current_stem, operation.old.as_ref()) {
            return Err(cleanup_skip(operation.track_id, "filename changed since analysis").into());
        }
        let new_stem = cleanup_input_text(operation.new.as_ref());
        if !valid_cleanup_leaf(&new_stem, 255) {
            return Err(cleanup_skip(
                operation.track_id,
                format!("invalid target name: {new_stem:?}"),
            )
            .into());
        }
        let extension = Path::new(track.path.file_name())
            .extension()
            .and_then(|value| value.to_str());
        let file_name = extension.map_or_else(
            || new_stem.clone(),
            |extension| format!("{new_stem}.{extension}"),
        );
        let destination = track.path.parent().map_or_else(
            || LibraryPath::parse(&file_name),
            |parent| parent.join(&file_name),
        );
        let destination = destination.map_err(|error| {
            cleanup_skip(operation.track_id, format!("invalid target name: {error}"))
        })?;
        if destination == track.path {
            return Err(cleanup_skip(operation.track_id, "no change").into());
        }
        let item = serde_json::Map::from_iter([
            ("kind".to_owned(), json!("rename")),
            ("track_id".to_owned(), json!(operation.track_id)),
            ("path_before".to_owned(), json!(track.path.as_str())),
            ("path_after".to_owned(), json!(destination.as_str())),
        ]);
        let mutation = LibraryFileMutation::MoveTrack {
            track_id,
            source: track.path,
            destination,
        };
        let append =
            CleanupBatchAppend::new(batch_id, scope_label.to_owned(), item).map_err(|error| {
                CleanupPreparationError::Fatal(LibraryCoordinatorError::InvalidCleanupMutation(
                    error,
                ))
            })?;
        Ok(PreparedCleanupMutation {
            track_id: operation.track_id,
            kind: PreparedCleanupKind::Rename {
                target_name: file_name,
            },
            mutation,
            append,
        })
    }

    async fn prepare_cleanup_tag(
        &self,
        batch_id: Option<i64>,
        scope_label: &str,
        operation: CleanupApplyOperation,
    ) -> Result<PreparedCleanupMutation, CleanupPreparationError> {
        let track_id = TrackId::new(operation.track_id)
            .map_err(|_| cleanup_skip(operation.track_id, "track not found"))?;
        let track = self
            .repository
            .track(track_id)
            .await
            .map_err(|source| {
                CleanupPreparationError::Fatal(dependency(
                    "load a track for cleanup tagging",
                    source,
                ))
            })?
            .ok_or_else(|| cleanup_skip(operation.track_id, "track not found"))?;
        let field_name = operation.field.as_deref().unwrap_or_default();
        let field = cleanup_tag_field(field_name).ok_or_else(|| {
            cleanup_skip(
                operation.track_id,
                format!("unsupported tag field: {field_name:?}"),
            )
        })?;
        let current = cleanup_track_value(&track, field);
        if !cleanup_values_match(current.as_ref(), operation.old.as_ref()) {
            return Err(cleanup_skip(
                operation.track_id,
                format!("{} changed since analysis", field.as_str()),
            )
            .into());
        }
        let patch = cleanup_tag_patch(field, operation.new.as_ref()).map_err(|reason| {
            cleanup_skip(
                operation.track_id,
                format!("invalid {} value: {reason}", field.as_str()),
            )
        })?;
        let file_old = self
            .effects
            .read_file_tag(&track.path, field)
            .await
            .map_err(|failure| {
                let reason = if failure.kind() == LibraryMutationFailureKind::NotFound {
                    "source file missing on disk".to_owned()
                } else {
                    format!("tag read failed: {}", failure.code())
                };
                cleanup_skip(operation.track_id, reason)
            })?;
        let item = serde_json::Map::from_iter([
            ("kind".to_owned(), json!("tag")),
            ("track_id".to_owned(), json!(operation.track_id)),
            ("field".to_owned(), json!(field.as_str())),
            ("old".to_owned(), cleanup_input_json(operation.old.as_ref())),
            (
                "file_old".to_owned(),
                library_file_tag_json(file_old.as_ref()),
            ),
            ("new".to_owned(), cleanup_input_json(operation.new.as_ref())),
            ("path".to_owned(), json!(track.path.as_str())),
        ]);
        let mutation = LibraryFileMutation::UpdateTrackMetadata {
            track_id,
            path: track.path,
            patch,
        };
        let append =
            CleanupBatchAppend::new(batch_id, scope_label.to_owned(), item).map_err(|error| {
                CleanupPreparationError::Fatal(LibraryCoordinatorError::InvalidCleanupMutation(
                    error,
                ))
            })?;
        Ok(PreparedCleanupMutation {
            track_id: operation.track_id,
            kind: PreparedCleanupKind::Tag,
            mutation,
            append,
        })
    }

    fn prepare_cleanup_folder_rename(
        &self,
        batch_id: Option<i64>,
        scope_label: &str,
        operation: CleanupApplyOperation,
    ) -> Result<PreparedCleanupMutation, CleanupPreparationError> {
        let normalized = operation.path.trim_matches('/').replace('\\', "/");
        let normalized = normalized.trim_matches('/');
        if normalized.is_empty() {
            return Err(
                cleanup_skip(operation.track_id, "refusing to rename the music root").into(),
            );
        }
        let source = LibraryPath::parse(normalized).map_err(|error| {
            cleanup_skip(operation.track_id, format!("invalid folder name: {error}"))
        })?;
        if !cleanup_expected_text_matches(source.file_name(), operation.old.as_ref()) {
            return Err(cleanup_skip(operation.track_id, "folder changed since analysis").into());
        }
        let new_leaf = cleanup_input_text(operation.new.as_ref());
        if !valid_cleanup_leaf(&new_leaf, 200) {
            return Err(cleanup_skip(
                operation.track_id,
                format!("invalid folder name: {new_leaf:?}"),
            )
            .into());
        }
        let destination = source.parent().map_or_else(
            || LibraryPath::parse(&new_leaf),
            |parent| parent.join(&new_leaf),
        );
        let destination = destination.map_err(|error| {
            cleanup_skip(operation.track_id, format!("invalid folder name: {error}"))
        })?;
        if destination == source {
            return Err(cleanup_skip(operation.track_id, "no change").into());
        }
        let item = serde_json::Map::from_iter([
            ("kind".to_owned(), json!("folder_rename")),
            ("path_before".to_owned(), json!(source.as_str())),
            ("path_after".to_owned(), json!(destination.as_str())),
        ]);
        let mutation = LibraryFileMutation::RenameFolder {
            source,
            destination,
        };
        let append =
            CleanupBatchAppend::new(batch_id, scope_label.to_owned(), item).map_err(|error| {
                CleanupPreparationError::Fatal(LibraryCoordinatorError::InvalidCleanupMutation(
                    error,
                ))
            })?;
        Ok(PreparedCleanupMutation {
            track_id: operation.track_id,
            kind: PreparedCleanupKind::FolderRename {
                target_name: new_leaf,
            },
            mutation,
            append,
        })
    }
}

pub(super) fn cleanup_path_depth(path: &str) -> usize {
    path.bytes()
        .filter(|byte| matches!(*byte, b'/' | b'\\'))
        .count()
}

fn cleanup_skip(track_id: i64, reason: impl Into<String>) -> CleanupSkip {
    CleanupSkip {
        track_id,
        reason: reason.into(),
    }
}

fn cleanup_expected_text_matches(current: &str, expected: Option<&CleanupInputValue>) -> bool {
    match expected {
        Some(CleanupInputValue::Text(expected)) => current == expected,
        None => current.is_empty(),
        Some(CleanupInputValue::Integer(_)) => false,
    }
}

fn cleanup_input_text(value: Option<&CleanupInputValue>) -> String {
    match value {
        Some(CleanupInputValue::Text(value)) => value.clone(),
        Some(CleanupInputValue::Integer(value)) => value.to_string(),
        None => String::new(),
    }
}

fn valid_cleanup_leaf(name: &str, maximum: usize) -> bool {
    !name.is_empty()
        && name.trim() == name
        && !name.contains(['/', '\\'])
        && !name.starts_with('.')
        && name.chars().count() <= maximum
}

const fn cleanup_tag_field(field: &str) -> Option<TrackMetadataField> {
    match field.as_bytes() {
        b"title" => Some(TrackMetadataField::Title),
        b"artist" => Some(TrackMetadataField::Artist),
        b"album_artist" => Some(TrackMetadataField::AlbumArtist),
        b"album" => Some(TrackMetadataField::Album),
        b"track_no" => Some(TrackMetadataField::TrackNumber),
        b"disc_no" => Some(TrackMetadataField::DiscNumber),
        b"year" => Some(TrackMetadataField::Year),
        _ => None,
    }
}

fn cleanup_track_value(
    track: &IndexedTrack,
    field: TrackMetadataField,
) -> Option<CleanupInputValue> {
    match field {
        TrackMetadataField::Title => Some(CleanupInputValue::Text(track.metadata.title.clone())),
        TrackMetadataField::Artist => Some(CleanupInputValue::Text(track.metadata.artist.clone())),
        TrackMetadataField::AlbumArtist => {
            Some(CleanupInputValue::Text(track.metadata.album_artist.clone()))
        }
        TrackMetadataField::Album => Some(CleanupInputValue::Text(track.metadata.album.clone())),
        TrackMetadataField::TrackNumber => track
            .metadata
            .track_no
            .map(|value| CleanupInputValue::Integer(i64::from(value))),
        TrackMetadataField::DiscNumber => track
            .metadata
            .disc_no
            .map(|value| CleanupInputValue::Integer(i64::from(value))),
        TrackMetadataField::Year => track
            .metadata
            .year
            .map(|value| CleanupInputValue::Integer(i64::from(value))),
        TrackMetadataField::Genre
        | TrackMetadataField::Bpm
        | TrackMetadataField::DisplayTitle
        | TrackMetadataField::Origin => None,
    }
}

fn cleanup_values_match(
    current: Option<&CleanupInputValue>,
    expected: Option<&CleanupInputValue>,
) -> bool {
    current == expected || (cleanup_value_absent(current) && cleanup_value_absent(expected))
}

fn cleanup_value_absent(value: Option<&CleanupInputValue>) -> bool {
    value.is_none_or(|value| matches!(value, CleanupInputValue::Text(text) if text.is_empty()))
}

fn cleanup_tag_patch(
    field: TrackMetadataField,
    value: Option<&CleanupInputValue>,
) -> Result<TrackMetadataPatch, TrackMetadataPatchError> {
    let mut patch = TrackMetadataPatch::new();
    if field.is_numeric() {
        let value = match value {
            None => None,
            Some(CleanupInputValue::Text(value)) if value.is_empty() => None,
            Some(CleanupInputValue::Integer(value)) => Some(
                u32::try_from(*value)
                    .map_err(|_| TrackMetadataPatchError::NumberOutOfRange { field })?,
            ),
            Some(CleanupInputValue::Text(value)) => Some(
                value
                    .parse::<u32>()
                    .map_err(|_| TrackMetadataPatchError::WrongValueType { field })?,
            ),
        };
        patch.insert_number(field, value)?;
    } else {
        let value = cleanup_input_text(value);
        patch.insert_text(field, (!value.is_empty()).then_some(value))?;
    }
    Ok(patch)
}

fn cleanup_input_json(value: Option<&CleanupInputValue>) -> serde_json::Value {
    value.map_or(serde_json::Value::Null, CleanupInputValue::to_json)
}

fn cleanup_input_from_json(value: Option<&Value>) -> Result<Option<CleanupInputValue>, ()> {
    match value {
        Some(Value::Null) | None => Ok(None),
        Some(Value::String(value)) => Ok(Some(CleanupInputValue::Text(value.clone()))),
        Some(Value::Number(value)) => value
            .as_i64()
            .map(CleanupInputValue::Integer)
            .map(Some)
            .ok_or(()),
        Some(_) => Err(()),
    }
}

fn cleanup_revert_paths(item: &Map<String, Value>) -> Result<(LibraryPath, LibraryPath), ()> {
    let before = item
        .get("path_before")
        .and_then(Value::as_str)
        .ok_or(())
        .and_then(|path| LibraryPath::parse(path).map_err(|_| ()))?;
    let after = item
        .get("path_after")
        .and_then(Value::as_str)
        .ok_or(())
        .and_then(|path| LibraryPath::parse(path).map_err(|_| ()))?;
    Ok((before, after))
}

fn library_file_tag_json(value: Option<&LibraryFileTagValue>) -> serde_json::Value {
    match value {
        Some(LibraryFileTagValue::Text(value)) => json!(value),
        Some(LibraryFileTagValue::Number(value)) => json!(value),
        None => serde_json::Value::Null,
    }
}

pub(super) fn cleanup_mutation_failure_reason(
    kind: &PreparedCleanupKind,
    failure: &LibraryMutationFailure,
) -> String {
    match (kind, failure.kind()) {
        (PreparedCleanupKind::Rename { .. }, LibraryMutationFailureKind::NotFound)
        | (PreparedCleanupKind::Tag, LibraryMutationFailureKind::NotFound) => {
            "source file missing on disk".to_owned()
        }
        (PreparedCleanupKind::Rename { target_name }, LibraryMutationFailureKind::Conflict) => {
            format!("a file named {target_name} already exists")
        }
        (PreparedCleanupKind::FolderRename { .. }, LibraryMutationFailureKind::NotFound) => {
            "folder missing on disk".to_owned()
        }
        (
            PreparedCleanupKind::FolderRename { target_name },
            LibraryMutationFailureKind::Conflict,
        ) => format!("a folder named {target_name} already exists"),
        (PreparedCleanupKind::Tag, LibraryMutationFailureKind::Invalid) => {
            format!("unsupported format: {}", failure.code())
        }
        (PreparedCleanupKind::Tag, _) => format!("tag write failed: {}", failure.code()),
        (PreparedCleanupKind::Rename { .. }, _) => {
            format!("file rename failed: {}", failure.code())
        }
        (PreparedCleanupKind::FolderRename { .. }, _) => {
            format!("folder rename failed: {}", failure.code())
        }
    }
}

pub(super) fn cleanup_revert_failure_reason(
    kind: &PreparedCleanupRevertKind,
    failure: &LibraryMutationFailure,
) -> String {
    match (kind, failure.kind()) {
        (PreparedCleanupRevertKind::Rename { .. }, LibraryMutationFailureKind::NotFound)
        | (PreparedCleanupRevertKind::Tag, LibraryMutationFailureKind::NotFound) => {
            "file missing on disk".to_owned()
        }
        (
            PreparedCleanupRevertKind::Rename { original_name },
            LibraryMutationFailureKind::Conflict,
        ) => format!("original name {original_name} is taken"),
        (PreparedCleanupRevertKind::FolderRename { .. }, LibraryMutationFailureKind::NotFound) => {
            "no folder at the recorded path (moved or removed since)".to_owned()
        }
        (
            PreparedCleanupRevertKind::FolderRename { original_name },
            LibraryMutationFailureKind::Conflict,
        ) => format!("original folder name {original_name} is taken"),
        (PreparedCleanupRevertKind::Tag, _) => {
            format!("tag write failed: {}", failure.code())
        }
        (PreparedCleanupRevertKind::Rename { .. }, _) => {
            format!("file rename failed: {}", failure.code())
        }
        (PreparedCleanupRevertKind::FolderRename { .. }, _) => {
            format!("folder rename failed: {}", failure.code())
        }
    }
}
