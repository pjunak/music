//! Mode and JSON adapters produce the same inert resource bundle.
use super::*;

pub(super) async fn load_bundle(
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

fn supported_effect(effect_type: &str) -> bool {
    matches!(
        effect_type,
        "eq" | "reverb" | "lowpass" | "highpass" | "bandpass" | "delay" | "distortion" | "tremolo"
    )
}

fn plural(count: usize, singular: &str) -> String {
    format!("{count} {singular}{}", if count == 1 { "" } else { "s" })
}
