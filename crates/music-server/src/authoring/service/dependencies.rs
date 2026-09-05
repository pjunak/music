//! Resource-specific dependency checks shared by preview and commit selection.
use super::*;

pub(super) fn dependency_issues(
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

fn soundboard_contains(soundboard: &SoundboardDocument, item_path: &str) -> bool {
    soundboard
        .categories
        .iter()
        .flat_map(|category| &category.items)
        .any(|item| item.file == item_path)
}
