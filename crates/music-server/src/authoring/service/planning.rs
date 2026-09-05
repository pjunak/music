//! Pure selection and create-only mutation planning; the coordinator owns commit/recovery.
use super::*;

pub(super) fn select_resources(
    preview: &AuthoringImportPreview,
    bundle: &ImportBundle,
    selections: &[AuthoringImportSelection],
) -> Result<SelectionPlan, ApiError> {
    let preview_by_key = preview
        .items
        .iter()
        .map(|item| (format!("{}:{}", item.kind.as_str(), item.resource_id), item))
        .collect::<BTreeMap<_, _>>();
    let resources_by_key = bundle
        .resources
        .iter()
        .map(|resource| (resource.key(), resource))
        .collect::<BTreeMap<_, _>>();
    let mut requested = Vec::new();
    let mut seen = BTreeSet::new();
    for selection in selections {
        let key = selection.key();
        if !seen.insert(key.clone()) {
            continue;
        }
        if !preview_by_key.contains_key(&key) || !resources_by_key.contains_key(&key) {
            return Err(ApiError::bad_request_message(format!(
                "source resource '{key}' is no longer available"
            )));
        }
        requested.push(key);
    }
    let selected_ready = requested
        .iter()
        .filter(|key| {
            preview_by_key
                .get(*key)
                .is_some_and(|item| item.status == ImportItemStatus::Ready)
        })
        .cloned()
        .collect::<BTreeSet<_>>();
    for key in &selected_ready {
        let item = preview_by_key.get(key).ok_or_else(ApiError::internal)?;
        for dependency in item.issues.iter().filter(|issue| {
            issue.code == "dependency_selection_required" && issue.related_item.is_some()
        }) {
            let related = dependency
                .related_item
                .as_ref()
                .ok_or_else(ApiError::internal)?;
            if !selected_ready.contains(&related.key()) {
                return Err(ApiError::bad_request_message(format!(
                    "'{}' requires {} '{}' to be selected and ready",
                    item.name,
                    related.kind.as_str(),
                    related.resource_id
                )));
            }
        }
    }
    Ok(SelectionPlan {
        imported: requested
            .iter()
            .filter(|key| selected_ready.contains(*key))
            .filter_map(|key| {
                resources_by_key
                    .get(key)
                    .map(|resource| (*resource).clone())
            })
            .collect(),
        skipped: requested
            .iter()
            .filter(|key| !selected_ready.contains(*key))
            .filter_map(|key| preview_by_key.get(key).map(|item| (*item).clone()))
            .collect(),
    })
}

pub(super) fn build_mutation(
    generation: u64,
    target_mode_id: &str,
    plan: PreviewPlan,
    selected: SelectionPlan,
) -> MutationPlan {
    let mut manifest: ModeDocument = plan.target.manifest;
    let mut soundboards = BTreeMap::new();
    let mut cues = BTreeMap::new();
    let mut presets = BTreeMap::new();
    let mut playlists = Vec::new();
    let mut missing_track_paths = Vec::new();
    let preview_by_key = plan
        .preview
        .items
        .iter()
        .map(|item| {
            (
                format!("{}:{}", item.kind.as_str(), item.resource_id),
                item.clone(),
            )
        })
        .collect::<BTreeMap<_, _>>();
    let mut imported_items = Vec::with_capacity(selected.imported.len());
    for resource in selected.imported {
        if let Some(item) = preview_by_key.get(&resource.key()) {
            imported_items.push(item.clone());
        }
        match resource.payload {
            ResourcePayload::Playlist(playlist) => {
                playlists.push(plan_playlist(
                    playlist,
                    &plan.library_tracks,
                    &mut manifest.playlist_categories,
                    &mut missing_track_paths,
                ));
            }
            ResourcePayload::Soundboard(document) => {
                soundboards.insert(resource.resource_id, document);
            }
            ResourcePayload::Interrupt(document) => manifest.interrupts.push(document),
            ResourcePayload::Preset(document) => {
                presets.insert(resource.resource_id, document);
            }
            ResourcePayload::Cue(document) => {
                cues.insert(resource.resource_id, document);
            }
        }
    }
    MutationPlan {
        mutation: ModeMutation::ImportResources {
            expected_generation: generation,
            mode_id: target_mode_id.to_owned(),
            manifest,
            soundboards,
            cues,
            presets,
            playlists,
        },
        result: AuthoringImportResult {
            imported: imported_items,
            skipped: selected.skipped,
            missing_track_paths,
        },
    }
}

fn plan_playlist(
    playlist: PlaylistPayload,
    library_tracks: &BTreeMap<String, TrackId>,
    categories: &mut Vec<String>,
    missing_track_paths: &mut Vec<String>,
) -> ModeImportPlaylist {
    let mut track_ids = Vec::new();
    for track in playlist.tracks {
        if let Some(track_id) = track
            .path
            .as_ref()
            .and_then(|path| library_tracks.get(path))
        {
            track_ids.push(*track_id);
        } else {
            missing_track_paths.push(track.missing_label);
        }
    }
    if let Some(category) = &playlist.category
        && !category.is_empty()
        && !categories.contains(category)
    {
        categories.push(category.clone());
    }
    ModeImportPlaylist {
        name: playlist.name,
        category: playlist.category,
        track_ids,
    }
}
