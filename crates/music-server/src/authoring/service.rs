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

mod dependencies;
mod planning;
mod source;

use dependencies::dependency_issues;
use planning::{build_mutation, select_resources};
use source::load_bundle;

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

fn counts<'a>(values: impl Iterator<Item = &'a str>) -> BTreeMap<String, usize> {
    let mut counts = BTreeMap::new();
    for value in values {
        *counts.entry(value.to_owned()).or_insert(0) += 1;
    }
    counts
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
