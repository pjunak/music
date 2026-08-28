use std::collections::{BTreeMap, BTreeSet};

use music_application::modes::{
    CueDocument, CueLoopDocument, CueSfxDocument, EffectDocument, InterruptDocument, ModeBundle,
    ModeCatalog, ModeDocument, ModeImportPlaylist, ModeMutation, ModeMutationError,
    ModeMutationFailureKind, PresetDocument, SoundboardCategoryDocument, SoundboardDocument,
    SoundboardItemDocument,
};
use music_application::playlists::PlaylistFilter;
use music_domain::{LibraryPath, TrackId};

use super::model::{
    AuthoringImportDocumentV1, AuthoringImportIssue, AuthoringImportItem, AuthoringImportMode,
    AuthoringImportPreview, AuthoringImportResult, AuthoringImportSelection, AuthoringImportSource,
    AuthoringResourceKind, AuthoringSourceType, ImportIssueSeverity, ImportItemStatus,
};
use crate::error::ApiError;
use crate::http::HttpState;

const MAX_COMMIT_REPLANS: usize = 3;

#[derive(Debug, Clone)]
pub(super) enum ImportSourceSpec {
    Mode(String),
    Document {
        document: AuthoringImportDocumentV1,
        source_name: Option<String>,
    },
}

#[derive(Debug, Clone)]
struct PlaylistTrackRef {
    path: Option<String>,
    missing_label: String,
}

#[derive(Debug, Clone)]
struct PlaylistPayload {
    name: String,
    category: Option<String>,
    tracks: Vec<PlaylistTrackRef>,
}

#[derive(Debug, Clone)]
enum ResourcePayload {
    Playlist(PlaylistPayload),
    Soundboard(SoundboardDocument),
    Interrupt(InterruptDocument),
    Preset(PresetDocument),
    Cue(CueDocument),
}

#[derive(Debug, Clone)]
struct ImportResource {
    kind: AuthoringResourceKind,
    resource_id: String,
    name: String,
    summary: String,
    payload: ResourcePayload,
    issues: Vec<AuthoringImportIssue>,
}

impl ImportResource {
    fn key(&self) -> String {
        format!("{}:{}", self.kind.as_str(), self.resource_id)
    }

    fn selection(&self) -> AuthoringImportSelection {
        AuthoringImportSelection {
            kind: self.kind,
            resource_id: self.resource_id.clone(),
        }
    }
}

#[derive(Debug, Clone)]
struct ImportBundle {
    source: AuthoringImportSource,
    resources: Vec<ImportResource>,
}

struct PreviewPlan {
    preview: AuthoringImportPreview,
    target: ModeBundle,
    library_tracks: BTreeMap<String, TrackId>,
}

struct SelectionPlan {
    imported: Vec<ImportResource>,
    skipped: Vec<AuthoringImportItem>,
}

struct MutationPlan {
    mutation: ModeMutation,
    result: AuthoringImportResult,
}

pub(super) async fn preview(
    state: &HttpState,
    target_mode_id: &str,
    source: &ImportSourceSpec,
) -> Result<AuthoringImportPreview, ApiError> {
    let catalog = mode_catalog(state)?;
    let bundle = load_bundle(state, source, &catalog).await?;
    Ok(build_preview(state, target_mode_id, bundle, &catalog)
        .await?
        .preview)
}

pub(super) async fn commit(
    state: &HttpState,
    target_mode_id: &str,
    source: &ImportSourceSpec,
    selections: &[AuthoringImportSelection],
) -> Result<AuthoringImportResult, ApiError> {
    let mut last_conflict = None;
    for _ in 0..MAX_COMMIT_REPLANS {
        let catalog = mode_catalog(state)?;
        let bundle = load_bundle(state, source, &catalog).await?;
        let plan = build_preview(state, target_mode_id, bundle.clone(), &catalog).await?;
        let selected = select_resources(&plan.preview, &bundle, selections)?;
        if selected.imported.is_empty() {
            return Ok(AuthoringImportResult {
                imported: Vec::new(),
                skipped: selected.skipped,
                missing_track_paths: Vec::new(),
            });
        }
        let mutation = build_mutation(catalog.generation, target_mode_id, plan, selected);
        let coordinator = state
            .modes
            .as_ref()
            .ok_or_else(ApiError::service_unavailable)?;
        match coordinator.mutate(mutation.mutation).await {
            Ok(_) => return Ok(mutation.result),
            Err(error)
                if matches!(
                    error.kind,
                    ModeMutationFailureKind::Stale | ModeMutationFailureKind::Conflict
                ) =>
            {
                last_conflict = Some(error);
            }
            Err(error) => return Err(map_mutation_error(error)),
        }
    }
    Err(last_conflict.map_or_else(
        || ApiError::conflict("authoring import changed during commit"),
        map_mutation_error,
    ))
}

fn mode_catalog(state: &HttpState) -> Result<std::sync::Arc<ModeCatalog>, ApiError> {
    state
        .modes
        .as_ref()
        .and_then(music_application::modes::ModeCoordinatorHandle::snapshot)
        .ok_or_else(ApiError::service_unavailable)
}

async fn load_bundle(
    state: &HttpState,
    source: &ImportSourceSpec,
    catalog: &ModeCatalog,
) -> Result<ImportBundle, ApiError> {
    match source {
        ImportSourceSpec::Mode(mode_id) => bundle_from_mode(state, catalog, mode_id).await,
        ImportSourceSpec::Document {
            document,
            source_name,
        } => Ok(bundle_from_document(document, source_name.as_deref())),
    }
}

async fn bundle_from_mode(
    state: &HttpState,
    catalog: &ModeCatalog,
    mode_id: &str,
) -> Result<ImportBundle, ApiError> {
    let mode = catalog
        .modes
        .get(mode_id)
        .ok_or_else(|| ApiError::not_found_message(format!("mode '{mode_id}' not loaded")))?;
    let playlists = state
        .playlists
        .as_ref()
        .ok_or_else(ApiError::service_unavailable)?;
    let mut playlist_records = playlists
        .list(&PlaylistFilter {
            mode_id: Some(mode_id.to_owned()),
            category: None,
        })
        .await
        .map_err(|_| ApiError::service_unavailable())?;
    playlist_records.sort_by(|left, right| {
        left.name
            .cmp(&right.name)
            .then_with(|| left.id.cmp(&right.id))
    });

    let mut resources = Vec::new();
    for playlist in playlist_records {
        let items = playlists
            .items(playlist.id)
            .await
            .map_err(|_| ApiError::service_unavailable())?;
        let tracks = items
            .items
            .into_iter()
            .map(|item| {
                let path = item.track.map(|track| track.path.into_string());
                PlaylistTrackRef {
                    missing_label: path
                        .clone()
                        .unwrap_or_else(|| format!("track-id:{}", item.track_id)),
                    path,
                }
            })
            .collect::<Vec<_>>();
        let mut summary = plural(tracks.len(), "track");
        if let Some(category) = &playlist.category {
            summary.push_str(" · ");
            summary.push_str(category);
        }
        resources.push(ImportResource {
            kind: AuthoringResourceKind::Playlist,
            resource_id: playlist.id.to_string(),
            name: playlist.name.clone(),
            summary,
            payload: ResourcePayload::Playlist(PlaylistPayload {
                name: playlist.name,
                category: playlist.category,
                tracks,
            }),
            issues: Vec::new(),
        });
    }
    for (soundboard_id, soundboard) in &mode.soundboards {
        resources.push(ImportResource {
            kind: AuthoringResourceKind::Soundboard,
            resource_id: soundboard_id.clone(),
            name: soundboard
                .name
                .clone()
                .unwrap_or_else(|| soundboard_id.clone()),
            summary: plural(
                soundboard
                    .categories
                    .iter()
                    .map(|category| category.items.len())
                    .sum(),
                "sound",
            ),
            payload: ResourcePayload::Soundboard(soundboard.clone()),
            issues: Vec::new(),
        });
    }
    for (index, interrupt) in mode.manifest.interrupts.iter().enumerate() {
        resources.push(ImportResource {
            kind: AuthoringResourceKind::Interrupt,
            resource_id: index.to_string(),
            name: interrupt.name.clone(),
            summary: interrupt.playlist.as_ref().map_or_else(
                || {
                    format!(
                        "Sound · {}",
                        interrupt
                            .soundboard_item
                            .as_deref()
                            .unwrap_or("missing reference")
                    )
                },
                |playlist| format!("Playlist · {playlist}"),
            ),
            payload: ResourcePayload::Interrupt(interrupt.clone()),
            issues: Vec::new(),
        });
    }
    for (preset_id, preset) in &mode.presets {
        resources.push(ImportResource {
            kind: AuthoringResourceKind::Preset,
            resource_id: preset_id.clone(),
            name: preset.name.clone(),
            summary: plural(preset.effects.len(), "effect"),
            payload: ResourcePayload::Preset(preset.clone()),
            issues: Vec::new(),
        });
    }
    for (cue_id, cue) in &mode.cues {
        resources.push(ImportResource {
            kind: AuthoringResourceKind::Cue,
            resource_id: cue_id.clone(),
            name: cue.name.clone(),
            summary: plural(
                usize::from(cue.preset.is_some())
                    + usize::from(cue.playlist.is_some())
                    + cue.sfx.len()
                    + cue.loops.len(),
                "action",
            ),
            payload: ResourcePayload::Cue(cue.clone()),
            issues: Vec::new(),
        });
    }
    Ok(ImportBundle {
        source: AuthoringImportSource {
            source_type: AuthoringSourceType::Mode,
            id: mode.manifest.id.clone(),
            name: mode.manifest.name.clone(),
        },
        resources,
    })
}

fn bundle_from_document(
    document: &AuthoringImportDocumentV1,
    source_name: Option<&str>,
) -> ImportBundle {
    let mut resources = Vec::new();
    for (index, playlist) in document.playlists.iter().enumerate() {
        let issues = playlist
            .tracks
            .iter()
            .filter_map(|path| path_issue(path, "Playlist track path"))
            .collect();
        let mut summary = plural(playlist.tracks.len(), "track");
        if let Some(category) = &playlist.category {
            summary.push_str(" · ");
            summary.push_str(category);
        }
        resources.push(ImportResource {
            kind: AuthoringResourceKind::Playlist,
            resource_id: index.to_string(),
            name: playlist.name.clone(),
            summary,
            payload: ResourcePayload::Playlist(PlaylistPayload {
                name: playlist.name.clone(),
                category: playlist.category.clone(),
                tracks: playlist
                    .tracks
                    .iter()
                    .map(|path| PlaylistTrackRef {
                        path: Some(path.clone()),
                        missing_label: path.clone(),
                    })
                    .collect(),
            }),
            issues,
        });
    }
    for soundboard in &document.soundboards {
        let issues = soundboard
            .categories
            .iter()
            .flat_map(|category| &category.items)
            .filter_map(|item| path_issue(&item.file, "Soundboard item path"))
            .collect();
        let payload = SoundboardDocument {
            id: Some(soundboard.id.clone()),
            name: soundboard.name.clone(),
            categories: soundboard
                .categories
                .iter()
                .map(|category| SoundboardCategoryDocument {
                    id: category.id.clone(),
                    name: category.name.clone(),
                    items: category
                        .items
                        .iter()
                        .map(|item| SoundboardItemDocument {
                            file: item.file.clone(),
                            name: item.name.clone(),
                            icon: item.icon.clone(),
                            hotkey: item.hotkey.clone(),
                            extra: BTreeMap::new(),
                        })
                        .collect(),
                    extra: BTreeMap::new(),
                })
                .collect(),
            extra: BTreeMap::new(),
        };
        resources.push(ImportResource {
            kind: AuthoringResourceKind::Soundboard,
            resource_id: soundboard.id.clone(),
            name: soundboard
                .name
                .clone()
                .unwrap_or_else(|| soundboard.id.clone()),
            summary: plural(
                soundboard
                    .categories
                    .iter()
                    .map(|category| category.items.len())
                    .sum(),
                "sound",
            ),
            payload: ResourcePayload::Soundboard(payload),
            issues,
        });
    }
    for (index, interrupt) in document.interrupts.iter().enumerate() {
        let issues = interrupt
            .soundboard_item
            .as_deref()
            .and_then(|path| path_issue(path, "Interrupt sound path"))
            .into_iter()
            .collect();
        resources.push(ImportResource {
            kind: AuthoringResourceKind::Interrupt,
            resource_id: index.to_string(),
            name: interrupt.name.clone(),
            summary: interrupt.playlist.as_ref().map_or_else(
                || {
                    format!(
                        "Sound · {}",
                        interrupt.soundboard_item.as_deref().unwrap_or_default()
                    )
                },
                |playlist| format!("Playlist · {playlist}"),
            ),
            payload: ResourcePayload::Interrupt(InterruptDocument {
                name: interrupt.name.clone(),
                playlist: interrupt.playlist.clone(),
                soundboard_item: interrupt.soundboard_item.clone(),
                fade_in_ms: interrupt.fade_in_ms,
                fade_out_ms: interrupt.fade_out_ms,
                return_to_ambient: interrupt.return_to_ambient,
                duck_to: interrupt.duck_to,
                extra: BTreeMap::new(),
            }),
            issues,
        });
    }
    for preset in &document.presets {
        let mut issues = Vec::new();
        for effect in &preset.effects {
            if !supported_effect(&effect.effect_type) {
                issues.push(issue(
                    "unsupported_effect",
                    ImportIssueSeverity::Error,
                    format!("unsupported effect type '{}'.", effect.effect_type),
                    None,
                ));
            }
        }
        resources.push(ImportResource {
            kind: AuthoringResourceKind::Preset,
            resource_id: preset.id.clone(),
            name: preset.name.clone(),
            summary: plural(preset.effects.len(), "effect"),
            payload: ResourcePayload::Preset(PresetDocument {
                id: Some(preset.id.clone()),
                name: preset.name.clone(),
                description: preset.description.clone(),
                effects: preset
                    .effects
                    .iter()
                    .map(|effect| EffectDocument {
                        effect_type: effect.effect_type.clone(),
                        parameters: effect.parameters.clone(),
                    })
                    .collect(),
                crossfade_ms: preset.crossfade_ms,
                extra: BTreeMap::new(),
            }),
            issues,
        });
    }
    for cue in &document.cues {
        let issues = cue
            .sfx
            .iter()
            .map(|item| item.item.as_str())
            .chain(cue.loops.iter().map(|item| item.item.as_str()))
            .filter_map(|path| path_issue(path, "Cue sound path"))
            .collect();
        resources.push(ImportResource {
            kind: AuthoringResourceKind::Cue,
            resource_id: cue.id.clone(),
            name: cue.name.clone(),
            summary: plural(
                usize::from(cue.preset.is_some())
                    + usize::from(cue.playlist.is_some())
                    + cue.sfx.len()
                    + cue.loops.len(),
                "action",
            ),
            payload: ResourcePayload::Cue(CueDocument {
                id: Some(cue.id.clone()),
                name: cue.name.clone(),
                description: cue.description.clone(),
                preset: cue.preset.clone(),
                playlist: cue.playlist.clone(),
                start_index: cue.start_index,
                start_ms: cue.start_ms,
                sfx: cue
                    .sfx
                    .iter()
                    .map(|item| CueSfxDocument {
                        soundboard: item.soundboard.clone(),
                        item: item.item.clone(),
                        volume: item.volume,
                        extra: BTreeMap::new(),
                    })
                    .collect(),
                loops: cue
                    .loops
                    .iter()
                    .map(|item| CueLoopDocument {
                        soundboard: item.soundboard.clone(),
                        item: item.item.clone(),
                        interval_s: item.interval_s,
                        volume: item.volume,
                        extra: BTreeMap::new(),
                    })
                    .collect(),
                extra: BTreeMap::new(),
            }),
            issues,
        });
    }
    ImportBundle {
        source: AuthoringImportSource {
            source_type: AuthoringSourceType::Document,
            id: document.schema_version.clone(),
            name: document
                .name
                .clone()
                .or_else(|| source_name.map(str::to_owned))
                .unwrap_or_else(|| "JSON document".to_owned()),
        },
        resources,
    }
}

async fn build_preview(
    state: &HttpState,
    target_mode_id: &str,
    bundle: ImportBundle,
    catalog: &ModeCatalog,
) -> Result<PreviewPlan, ApiError> {
    let target = catalog.modes.get(target_mode_id).cloned().ok_or_else(|| {
        ApiError::not_found_message(format!("mode '{target_mode_id}' not loaded"))
    })?;
    if bundle.source.source_type == AuthoringSourceType::Mode && bundle.source.id == target_mode_id
    {
        return Err(ApiError::bad_request(
            "source and target modes must be different",
        ));
    }
    let playlists = state
        .playlists
        .as_ref()
        .ok_or_else(ApiError::service_unavailable)?;
    let target_playlist_names = playlists
        .list(&PlaylistFilter {
            mode_id: Some(target_mode_id.to_owned()),
            category: None,
        })
        .await
        .map_err(|_| ApiError::service_unavailable())?
        .into_iter()
        .map(|playlist| playlist.name)
        .collect::<BTreeSet<_>>();
    let library = state
        .library
        .as_ref()
        .ok_or_else(ApiError::service_unavailable)?;
    let library_tracks = library
        .service
        .all_tracks()
        .await
        .map_err(|_| ApiError::service_unavailable())?
        .into_iter()
        .map(|track| (track.path.into_string(), track.id))
        .collect::<BTreeMap<_, _>>();

    let playlist_name_counts = counts(bundle.resources.iter().filter_map(|resource| {
        if let ResourcePayload::Playlist(playlist) = &resource.payload {
            Some(playlist.name.as_str())
        } else {
            None
        }
    }));
    let interrupt_name_counts = counts(bundle.resources.iter().filter_map(|resource| {
        matches!(resource.payload, ResourcePayload::Interrupt(_)).then_some(resource.name.as_str())
    }));
    let target_interrupt_names = target
        .manifest
        .interrupts
        .iter()
        .map(|interrupt| interrupt.name.as_str())
        .collect::<BTreeSet<_>>();

    let mut items = Vec::with_capacity(bundle.resources.len());
    for resource in &bundle.resources {
        let mut issues = resource.issues.clone();
        let mut conflict_reason = None;
        match &resource.payload {
            ResourcePayload::Playlist(playlist) => {
                if playlist_name_counts
                    .get(&playlist.name)
                    .copied()
                    .unwrap_or(0)
                    > 1
                {
                    issues.push(issue(
                        "duplicate_source_name",
                        ImportIssueSeverity::Error,
                        "Another source playlist has the same name.",
                        None,
                    ));
                } else if target_playlist_names.contains(&playlist.name) {
                    conflict_reason = Some(
                        "A playlist with this name already exists in the target mode.".to_owned(),
                    );
                }
                let missing = playlist
                    .tracks
                    .iter()
                    .filter(|track| {
                        track
                            .path
                            .as_ref()
                            .is_none_or(|path| !library_tracks.contains_key(path))
                    })
                    .count();
                if missing > 0 {
                    issues.push(issue(
                        "missing_tracks",
                        ImportIssueSeverity::Warning,
                        format!(
                            "{missing} track reference(s) are unavailable and will be omitted."
                        ),
                        None,
                    ));
                }
            }
            ResourcePayload::Soundboard(_) => {
                if target.soundboards.contains_key(&resource.resource_id) {
                    conflict_reason = Some(
                        "A soundboard with this ID already exists in the target mode.".to_owned(),
                    );
                }
            }
            ResourcePayload::Interrupt(_) => {
                if interrupt_name_counts
                    .get(&resource.name)
                    .copied()
                    .unwrap_or(0)
                    > 1
                {
                    issues.push(issue(
                        "duplicate_source_name",
                        ImportIssueSeverity::Error,
                        "Another source interrupt has the same name.",
                        None,
                    ));
                } else if target_interrupt_names.contains(resource.name.as_str()) {
                    conflict_reason = Some(
                        "An interrupt with this name already exists in the target mode.".to_owned(),
                    );
                }
            }
            ResourcePayload::Preset(_) => {
                if target.presets.contains_key(&resource.resource_id) {
                    conflict_reason = Some(
                        "An EQ preset with this ID already exists in the target mode.".to_owned(),
                    );
                }
            }
            ResourcePayload::Cue(_) => {
                if target.cues.contains_key(&resource.resource_id) {
                    conflict_reason =
                        Some("A cue with this ID already exists in the target mode.".to_owned());
                }
            }
        }
        issues.extend(dependency_issues(
            resource,
            &bundle,
            &target,
            &target_playlist_names,
        ));
        let first_error = issues
            .iter()
            .find(|issue| issue.severity == ImportIssueSeverity::Error)
            .map(|issue| issue.message.clone());
        let (status, reason) = if let Some(conflict) = conflict_reason {
            issues.push(issue(
                "target_conflict",
                ImportIssueSeverity::Error,
                conflict.clone(),
                None,
            ));
            (ImportItemStatus::Conflict, Some(conflict))
        } else if let Some(error) = first_error {
            (ImportItemStatus::Invalid, Some(error))
        } else {
            (ImportItemStatus::Ready, None)
        };
        items.push(AuthoringImportItem {
            kind: resource.kind,
            resource_id: resource.resource_id.clone(),
            name: resource.name.clone(),
            summary: resource.summary.clone(),
            status,
            reason,
            issues,
        });
    }

    let source_mode =
        (bundle.source.source_type == AuthoringSourceType::Mode).then(|| AuthoringImportMode {
            id: bundle.source.id.clone(),
            name: bundle.source.name.clone(),
        });
    Ok(PreviewPlan {
        preview: AuthoringImportPreview {
            source: bundle.source,
            source_mode,
            target_mode: AuthoringImportMode {
                id: target.manifest.id.clone(),
                name: target.manifest.name.clone(),
            },
            items,
        },
        target,
        library_tracks,
    })
}

fn dependency_issues(
    resource: &ImportResource,
    bundle: &ImportBundle,
    target: &ModeBundle,
    target_playlist_names: &BTreeSet<String>,
) -> Vec<AuthoringImportIssue> {
    let mut source_playlists = BTreeMap::<&str, Vec<&ImportResource>>::new();
    let mut source_presets = BTreeMap::<&str, &ImportResource>::new();
    let mut source_soundboards = BTreeMap::<&str, &ImportResource>::new();
    for candidate in &bundle.resources {
        match &candidate.payload {
            ResourcePayload::Playlist(playlist) => source_playlists
                .entry(&playlist.name)
                .or_default()
                .push(candidate),
            ResourcePayload::Preset(_) => {
                source_presets.insert(&candidate.resource_id, candidate);
            }
            ResourcePayload::Soundboard(_) => {
                source_soundboards.insert(&candidate.resource_id, candidate);
            }
            ResourcePayload::Interrupt(_) | ResourcePayload::Cue(_) => {}
        }
    }
    let mut issues = Vec::new();
    match &resource.payload {
        ResourcePayload::Interrupt(interrupt) => {
            if let Some(playlist) = &interrupt.playlist {
                require_playlist(
                    playlist,
                    target_playlist_names,
                    &source_playlists,
                    &mut issues,
                );
            } else if let Some(item_path) = &interrupt.soundboard_item {
                require_sound_path(item_path, target, &source_soundboards, &mut issues);
            }
        }
        ResourcePayload::Cue(cue) => {
            if let Some(preset) = &cue.preset {
                require_preset(preset, target, &source_presets, &mut issues);
            }
            if let Some(playlist) = &cue.playlist {
                require_playlist(
                    playlist,
                    target_playlist_names,
                    &source_playlists,
                    &mut issues,
                );
            }
            for item in &cue.sfx {
                require_soundboard(
                    &item.soundboard,
                    &item.item,
                    target,
                    &source_soundboards,
                    &mut issues,
                );
            }
            for item in &cue.loops {
                require_soundboard(
                    &item.soundboard,
                    &item.item,
                    target,
                    &source_soundboards,
                    &mut issues,
                );
            }
        }
        ResourcePayload::Playlist(_)
        | ResourcePayload::Soundboard(_)
        | ResourcePayload::Preset(_) => {}
    }
    issues
}

fn require_playlist(
    name: &str,
    target_names: &BTreeSet<String>,
    source: &BTreeMap<&str, Vec<&ImportResource>>,
    issues: &mut Vec<AuthoringImportIssue>,
) {
    if target_names.contains(name) {
        return;
    }
    match source.get(name).map(Vec::as_slice).unwrap_or_default() {
        [candidate] => issues.push(issue(
            "dependency_selection_required",
            ImportIssueSeverity::Warning,
            format!("Also select playlist '{name}'."),
            Some(*candidate),
        )),
        [] => issues.push(issue(
            "missing_dependency",
            ImportIssueSeverity::Error,
            format!("Referenced playlist '{name}' is not in the target or import document."),
            None,
        )),
        _ => issues.push(issue(
            "ambiguous_dependency",
            ImportIssueSeverity::Error,
            format!("Playlist reference '{name}' matches multiple source playlists."),
            None,
        )),
    }
}

fn require_preset(
    preset_id: &str,
    target: &ModeBundle,
    source: &BTreeMap<&str, &ImportResource>,
    issues: &mut Vec<AuthoringImportIssue>,
) {
    if target.presets.contains_key(preset_id) {
        return;
    }
    if let Some(candidate) = source.get(preset_id) {
        issues.push(issue(
            "dependency_selection_required",
            ImportIssueSeverity::Warning,
            format!("Also select EQ preset '{preset_id}'."),
            Some(candidate),
        ));
    } else {
        issues.push(issue(
            "missing_dependency",
            ImportIssueSeverity::Error,
            format!("Referenced EQ preset '{preset_id}' is not in the target or import document."),
            None,
        ));
    }
}

fn require_soundboard(
    soundboard_id: &str,
    item_path: &str,
    target: &ModeBundle,
    source: &BTreeMap<&str, &ImportResource>,
    issues: &mut Vec<AuthoringImportIssue>,
) {
    if let Some(board) = target.soundboards.get(soundboard_id) {
        if soundboard_contains(board, item_path) {
            return;
        }
        issues.push(issue(
            "missing_dependency",
            ImportIssueSeverity::Error,
            format!(
                "Target soundboard '{soundboard_id}' does not contain sound '{item_path}', and its ID is already occupied."
            ),
            None,
        ));
        return;
    }
    if let Some(candidate) = source.get(soundboard_id)
        && let ResourcePayload::Soundboard(board) = &candidate.payload
        && soundboard_contains(board, item_path)
    {
        issues.push(issue(
            "dependency_selection_required",
            ImportIssueSeverity::Warning,
            format!("Also select soundboard '{soundboard_id}'."),
            Some(candidate),
        ));
        return;
    }
    issues.push(issue(
        "missing_dependency",
        ImportIssueSeverity::Error,
        format!("Sound '{item_path}' is not available in soundboard '{soundboard_id}'."),
        None,
    ));
}

fn require_sound_path(
    item_path: &str,
    target: &ModeBundle,
    source: &BTreeMap<&str, &ImportResource>,
    issues: &mut Vec<AuthoringImportIssue>,
) {
    if target
        .soundboards
        .values()
        .any(|board| soundboard_contains(board, item_path))
    {
        return;
    }
    let matches = source
        .values()
        .copied()
        .filter(|candidate| {
            matches!(
                &candidate.payload,
                ResourcePayload::Soundboard(board) if soundboard_contains(board, item_path)
            )
        })
        .collect::<Vec<_>>();
    match matches.as_slice() {
        [candidate] if target.soundboards.contains_key(&candidate.resource_id) => {
            issues.push(issue(
                "missing_dependency",
                ImportIssueSeverity::Error,
                format!(
                    "Target soundboard '{}' does not contain sound '{item_path}', and its ID is already occupied.",
                    candidate.resource_id
                ),
                None,
            ));
        }
        [candidate] => issues.push(issue(
            "dependency_selection_required",
            ImportIssueSeverity::Warning,
            format!("Also select soundboard '{}'.", candidate.resource_id),
            Some(candidate),
        )),
        [] => issues.push(issue(
            "missing_dependency",
            ImportIssueSeverity::Error,
            format!("Referenced sound '{item_path}' is not in the target or import document."),
            None,
        )),
        _ => issues.push(issue(
            "ambiguous_dependency",
            ImportIssueSeverity::Error,
            format!("Sound reference '{item_path}' matches multiple source soundboards."),
            None,
        )),
    }
}

fn select_resources(
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

fn build_mutation(
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
                let mut track_ids = Vec::new();
                for track in playlist.tracks {
                    let Some(track_id) = track
                        .path
                        .as_ref()
                        .and_then(|path| plan.library_tracks.get(path))
                        .copied()
                    else {
                        missing_track_paths.push(track.missing_label);
                        continue;
                    };
                    track_ids.push(track_id);
                }
                if let Some(category) = &playlist.category
                    && !category.is_empty()
                    && !manifest.playlist_categories.contains(category)
                {
                    manifest.playlist_categories.push(category.clone());
                }
                playlists.push(ModeImportPlaylist {
                    name: playlist.name,
                    category: playlist.category,
                    track_ids,
                });
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

fn issue(
    code: impl Into<String>,
    severity: ImportIssueSeverity,
    message: impl Into<String>,
    related: Option<&ImportResource>,
) -> AuthoringImportIssue {
    AuthoringImportIssue {
        code: code.into(),
        severity,
        message: message.into(),
        related_item: related.map(ImportResource::selection),
    }
}

fn path_issue(path: &str, label: &str) -> Option<AuthoringImportIssue> {
    LibraryPath::parse(path.to_owned()).err().map(|_| {
        issue(
            "invalid_path",
            ImportIssueSeverity::Error,
            format!("{label} must be a canonical relative path using forward slashes: {path}"),
            None,
        )
    })
}

fn soundboard_contains(soundboard: &SoundboardDocument, item_path: &str) -> bool {
    soundboard
        .categories
        .iter()
        .flat_map(|category| &category.items)
        .any(|item| item.file == item_path)
}

fn supported_effect(effect_type: &str) -> bool {
    matches!(
        effect_type,
        "eq" | "reverb" | "lowpass" | "highpass" | "bandpass" | "delay" | "distortion" | "tremolo"
    )
}

fn counts<'a>(values: impl Iterator<Item = &'a str>) -> BTreeMap<String, usize> {
    let mut counts = BTreeMap::new();
    for value in values {
        *counts.entry(value.to_owned()).or_insert(0) += 1;
    }
    counts
}

fn plural(count: usize, singular: &str) -> String {
    format!("{count} {singular}{}", if count == 1 { "" } else { "s" })
}

fn map_mutation_error(error: ModeMutationError) -> ApiError {
    match error.kind {
        ModeMutationFailureKind::Invalid => ApiError::bad_request(error.code),
        ModeMutationFailureKind::NotFound => ApiError::plain_not_found(error.code),
        ModeMutationFailureKind::Conflict | ModeMutationFailureKind::Stale => {
            ApiError::conflict(error.code)
        }
        ModeMutationFailureKind::Unavailable => {
            tracing::error!(error = %error, "authoring import mutation failed");
            ApiError::service_unavailable()
        }
    }
}
