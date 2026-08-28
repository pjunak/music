use std::collections::BTreeMap;
use std::error::Error;
use std::fmt::{self, Display, Formatter};
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Component, Path, PathBuf};

use music_application::modes::{
    CueDocument, ModeDocument, ModeImportPlaylist, ModeMutation, ModeMutationEffects,
    ModeMutationError, ModeMutationFailureKind, ModeMutationFuture, PreparedModeMutation,
    PresetDocument, SoundboardDocument,
};
use music_application::recovery::{
    RecoveryDomain, RecoveryJournalEntry, RecoveryJournalId, RecoveryOperation,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::yaml::serialize_document;

const PLAN_VERSION: u8 = 1;
const STAGING_DIRECTORY: &str = ".music-mode-journal";
const MODE_MARKER: &str = ".music-journal-id";
const MAX_PLAN_PATH_CHARS: usize = 512;

#[derive(Debug, Clone)]
pub struct FilesystemModeMutations {
    root: PathBuf,
}

impl FilesystemModeMutations {
    pub fn open(path: impl AsRef<Path>) -> Result<Self, ModeFilesystemMutationError> {
        let root = fs::canonicalize(path.as_ref())
            .map_err(|_| ModeFilesystemMutationError::new("modes root could not be resolved"))?;
        let metadata = fs::metadata(&root)
            .map_err(|_| ModeFilesystemMutationError::new("modes root could not be inspected"))?;
        if !metadata.is_dir() {
            return Err(ModeFilesystemMutationError::new(
                "modes root is not a directory",
            ));
        }
        Ok(Self { root })
    }
}

impl ModeMutationEffects for FilesystemModeMutations {
    fn prepare<'a>(
        &'a self,
        journal_id: &'a RecoveryJournalId,
        mutation: &'a ModeMutation,
    ) -> ModeMutationFuture<'a, PreparedModeMutation> {
        let root = self.root.clone();
        let journal_id = journal_id.clone();
        let mutation = mutation.clone();
        Box::pin(async move {
            tokio::task::spawn_blocking(move || prepare(&root, &journal_id, &mutation))
                .await
                .map_err(|_| dependency("mode staging worker did not complete"))?
                .map_err(mode_dependency)
        })
    }

    fn apply<'a>(&'a self, journal: &'a RecoveryJournalEntry) -> ModeMutationFuture<'a, ()> {
        let root = self.root.clone();
        let journal = journal.clone();
        Box::pin(async move {
            tokio::task::spawn_blocking(move || apply(&root, &journal))
                .await
                .map_err(|_| dependency("mode apply worker did not complete"))?
                .map_err(mode_dependency)
        })
    }

    fn rollback<'a>(&'a self, journal: &'a RecoveryJournalEntry) -> ModeMutationFuture<'a, ()> {
        let root = self.root.clone();
        let journal = journal.clone();
        Box::pin(async move {
            tokio::task::spawn_blocking(move || rollback(&root, &journal))
                .await
                .map_err(|_| dependency("mode rollback worker did not complete"))?
                .map_err(|error| Box::new(error) as _)
        })
    }

    fn finish<'a>(&'a self, journal: &'a RecoveryJournalEntry) -> ModeMutationFuture<'a, ()> {
        let root = self.root.clone();
        let journal = journal.clone();
        Box::pin(async move {
            tokio::task::spawn_blocking(move || finish(&root, &journal))
                .await
                .map_err(|_| dependency("mode cleanup worker did not complete"))?
                .map_err(|error| Box::new(error) as _)
        })
    }

    fn cleanup_orphans(&self) -> ModeMutationFuture<'_, ()> {
        let root = self.root.clone();
        Box::pin(async move {
            tokio::task::spawn_blocking(move || cleanup_orphans(&root))
                .await
                .map_err(|_| dependency("mode orphan cleanup worker did not complete"))?
                .map_err(|error| Box::new(error) as _)
        })
    }
}

#[derive(Debug, Clone, Eq, PartialEq)]
pub struct ModeFilesystemMutationError {
    detail: &'static str,
}

impl ModeFilesystemMutationError {
    const fn new(detail: &'static str) -> Self {
        Self { detail }
    }
}

impl Display for ModeFilesystemMutationError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.detail)
    }
}

impl Error for ModeFilesystemMutationError {}

#[derive(Debug, Clone, Copy, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum FilesystemMutationKind {
    WriteFile,
    DeleteFile,
    CreateMode,
    DeleteMode,
    ImportResources,
}

#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
struct FilesystemWritePlan {
    target: String,
    candidate: String,
    backup: Option<String>,
    target_existed: bool,
    candidate_sha256: String,
}

#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
struct ImportPlaylistPlan {
    name: String,
    category: Option<String>,
    track_ids: Vec<i64>,
}

#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
struct FilesystemMutationPlan {
    version: u8,
    journal_id: String,
    kind: FilesystemMutationKind,
    target: String,
    stage_directory: String,
    candidate: Option<String>,
    backup: Option<String>,
    target_existed: bool,
    candidate_sha256: Option<String>,
    #[serde(default)]
    writes: Vec<FilesystemWritePlan>,
    #[serde(default)]
    playlists: Vec<ImportPlaylistPlan>,
}

fn dependency(detail: &'static str) -> Box<dyn Error + Send + Sync> {
    Box::new(ModeFilesystemMutationError::new(detail))
}

fn mode_dependency(error: ModeFilesystemMutationError) -> Box<dyn Error + Send + Sync> {
    if matches!(
        error.detail,
        "mode import target already exists"
            | "mode document already exists"
            | "mode create target is occupied"
            | "mode replacement target is occupied"
    ) {
        Box::new(ModeMutationError::new(
            ModeMutationFailureKind::Conflict,
            "target resource appeared during import",
        ))
    } else {
        Box::new(error)
    }
}

fn prepare(
    root: &Path,
    journal_id: &RecoveryJournalId,
    mutation: &ModeMutation,
) -> Result<PreparedModeMutation, ModeFilesystemMutationError> {
    let stage_relative = format!("{STAGING_DIRECTORY}/{}", journal_id.as_str());
    let stage = rooted_artifact(root, &stage_relative, journal_id.as_str())?;
    prepare_stage_directory(root, &stage)?;
    let result = prepare_inner(root, &stage_relative, &stage, journal_id, mutation);
    if result.is_err() {
        let _ = fs::remove_dir_all(&stage);
        let _ = fs::remove_dir(root.join(STAGING_DIRECTORY));
    }
    result
}

fn prepare_inner(
    root: &Path,
    stage_relative: &str,
    stage: &Path,
    journal_id: &RecoveryJournalId,
    mutation: &ModeMutation,
) -> Result<PreparedModeMutation, ModeFilesystemMutationError> {
    let (operation, plan) = match mutation {
        ModeMutation::CreateMode { manifest, .. } => {
            let target = manifest.id.clone();
            ensure_mode_target_absent(root, &target)?;
            let candidate_relative = format!("{stage_relative}/candidate");
            let candidate = stage.join("candidate");
            fs::create_dir(&candidate).map_err(|_| {
                ModeFilesystemMutationError::new("mode candidate could not be created")
            })?;
            fs::create_dir(candidate.join("soundboards")).map_err(|_| {
                ModeFilesystemMutationError::new("soundboard directory could not be staged")
            })?;
            fs::create_dir(candidate.join("cues")).map_err(|_| {
                ModeFilesystemMutationError::new("cue directory could not be staged")
            })?;
            fs::create_dir(candidate.join("presets")).map_err(|_| {
                ModeFilesystemMutationError::new("preset directory could not be staged")
            })?;
            write_synced(
                &candidate.join("manifest.yaml"),
                serialize_document(manifest)
                    .map_err(|_| {
                        ModeFilesystemMutationError::new("mode manifest could not be serialized")
                    })?
                    .as_bytes(),
            )?;
            write_synced(&candidate.join(MODE_MARKER), journal_id.as_str().as_bytes())?;
            (
                "create_mode",
                FilesystemMutationPlan {
                    version: PLAN_VERSION,
                    journal_id: journal_id.as_str().to_owned(),
                    kind: FilesystemMutationKind::CreateMode,
                    target,
                    stage_directory: stage_relative.to_owned(),
                    candidate: Some(candidate_relative),
                    backup: None,
                    target_existed: false,
                    candidate_sha256: None,
                    writes: Vec::new(),
                    playlists: Vec::new(),
                },
            )
        }
        ModeMutation::DeleteMode { mode_id, .. } => {
            ensure_mode_target(root, mode_id)?;
            (
                "delete_mode",
                delete_plan(
                    journal_id,
                    FilesystemMutationKind::DeleteMode,
                    mode_id.clone(),
                    stage_relative,
                ),
            )
        }
        ModeMutation::PutManifest {
            mode_id, manifest, ..
        } => prepare_file_write(
            root,
            stage,
            stage_relative,
            journal_id,
            "write_manifest",
            format!("{mode_id}/manifest.yaml"),
            serialize_document(manifest).map_err(|_| {
                ModeFilesystemMutationError::new("mode manifest could not be serialized")
            })?,
            false,
        )?,
        ModeMutation::PutSoundboard {
            mode_id,
            soundboard_id,
            document,
            create_only,
            ..
        } => prepare_file_write(
            root,
            stage,
            stage_relative,
            journal_id,
            "write_soundboard",
            format!("{mode_id}/soundboards/{soundboard_id}.yaml"),
            serialize_document(document).map_err(|_| {
                ModeFilesystemMutationError::new("soundboard could not be serialized")
            })?,
            *create_only,
        )?,
        ModeMutation::DeleteSoundboard {
            mode_id,
            soundboard_id,
            ..
        } => prepare_file_delete(
            root,
            journal_id,
            "delete_soundboard",
            format!("{mode_id}/soundboards/{soundboard_id}.yaml"),
            stage_relative,
        )?,
        ModeMutation::PutCue {
            mode_id,
            cue_id,
            document,
            create_only,
            ..
        } => prepare_file_write(
            root,
            stage,
            stage_relative,
            journal_id,
            "write_cue",
            format!("{mode_id}/cues/{cue_id}.yaml"),
            serialize_document(document)
                .map_err(|_| ModeFilesystemMutationError::new("cue could not be serialized"))?,
            *create_only,
        )?,
        ModeMutation::DeleteCue {
            mode_id, cue_id, ..
        } => prepare_file_delete(
            root,
            journal_id,
            "delete_cue",
            format!("{mode_id}/cues/{cue_id}.yaml"),
            stage_relative,
        )?,
        ModeMutation::PutPreset {
            mode_id,
            preset_id,
            document,
            create_only,
            ..
        } => prepare_file_write(
            root,
            stage,
            stage_relative,
            journal_id,
            "write_preset",
            format!("{mode_id}/presets/{preset_id}.yaml"),
            serialize_document(document)
                .map_err(|_| ModeFilesystemMutationError::new("preset could not be serialized"))?,
            *create_only,
        )?,
        ModeMutation::DeletePreset {
            mode_id, preset_id, ..
        } => prepare_file_delete(
            root,
            journal_id,
            "delete_preset",
            format!("{mode_id}/presets/{preset_id}.yaml"),
            stage_relative,
        )?,
        ModeMutation::ImportResources {
            mode_id,
            manifest,
            soundboards,
            cues,
            presets,
            playlists,
            ..
        } => prepare_import_resources(
            root,
            stage,
            stage_relative,
            journal_id,
            mode_id,
            manifest,
            soundboards,
            cues,
            presets,
            playlists,
        )?,
    };
    Ok(PreparedModeMutation {
        operation: RecoveryOperation::parse(operation)
            .map_err(|_| ModeFilesystemMutationError::new("mode operation is invalid"))?,
        plan: serde_json::to_value(plan)
            .map_err(|_| ModeFilesystemMutationError::new("mode plan could not be encoded"))?,
    })
}

#[allow(clippy::too_many_arguments)]
fn prepare_import_resources(
    root: &Path,
    stage: &Path,
    stage_relative: &str,
    journal_id: &RecoveryJournalId,
    mode_id: &str,
    manifest: &ModeDocument,
    soundboards: &BTreeMap<String, SoundboardDocument>,
    cues: &BTreeMap<String, CueDocument>,
    presets: &BTreeMap<String, PresetDocument>,
    playlists: &[ModeImportPlaylist],
) -> Result<(&'static str, FilesystemMutationPlan), ModeFilesystemMutationError> {
    ensure_mode_target(root, mode_id)?;
    let mut documents = vec![(
        format!("{mode_id}/manifest.yaml"),
        serialize_document(manifest).map_err(|_| {
            ModeFilesystemMutationError::new("mode manifest could not be serialized")
        })?,
        false,
    )];
    for (soundboard_id, document) in soundboards {
        documents.push((
            format!("{mode_id}/soundboards/{soundboard_id}.yaml"),
            serialize_document(document).map_err(|_| {
                ModeFilesystemMutationError::new("soundboard could not be serialized")
            })?,
            true,
        ));
    }
    for (cue_id, document) in cues {
        documents.push((
            format!("{mode_id}/cues/{cue_id}.yaml"),
            serialize_document(document)
                .map_err(|_| ModeFilesystemMutationError::new("cue could not be serialized"))?,
            true,
        ));
    }
    for (preset_id, document) in presets {
        documents.push((
            format!("{mode_id}/presets/{preset_id}.yaml"),
            serialize_document(document)
                .map_err(|_| ModeFilesystemMutationError::new("preset could not be serialized"))?,
            true,
        ));
    }

    let mut writes = Vec::with_capacity(documents.len());
    for (index, (target, content, create_only)) in documents.into_iter().enumerate() {
        writes.push(stage_import_write(
            root,
            stage,
            stage_relative,
            index,
            target,
            &content,
            create_only,
        )?);
    }
    Ok((
        "import_mode_resources",
        FilesystemMutationPlan {
            version: PLAN_VERSION,
            journal_id: journal_id.as_str().to_owned(),
            kind: FilesystemMutationKind::ImportResources,
            target: mode_id.to_owned(),
            stage_directory: stage_relative.to_owned(),
            candidate: None,
            backup: None,
            target_existed: true,
            candidate_sha256: None,
            writes,
            playlists: playlists
                .iter()
                .map(|playlist| ImportPlaylistPlan {
                    name: playlist.name.clone(),
                    category: playlist.category.clone(),
                    track_ids: playlist
                        .track_ids
                        .iter()
                        .map(|track_id| track_id.get())
                        .collect(),
                })
                .collect(),
        },
    ))
}

#[allow(clippy::too_many_arguments)]
fn stage_import_write(
    root: &Path,
    stage: &Path,
    stage_relative: &str,
    index: usize,
    target: String,
    content: &str,
    create_only: bool,
) -> Result<FilesystemWritePlan, ModeFilesystemMutationError> {
    let target_path = rooted_target(root, &target)?;
    let target_existed = regular_file_exists(&target_path)?;
    if create_only && target_existed {
        return Err(ModeFilesystemMutationError::new(
            "mode import target already exists",
        ));
    }
    if !create_only && !target_existed {
        return Err(ModeFilesystemMutationError::new(
            "mode import target does not exist",
        ));
    }
    let candidate_name = format!("candidate-{index}.yaml");
    let candidate = stage.join(&candidate_name);
    write_synced(&candidate, content.as_bytes())?;
    let candidate_sha256 = sha256_file(&candidate)?;
    Ok(FilesystemWritePlan {
        target,
        candidate: format!("{stage_relative}/{candidate_name}"),
        backup: target_existed.then(|| format!("{stage_relative}/backup-{index}.yaml")),
        target_existed,
        candidate_sha256,
    })
}

#[allow(clippy::too_many_arguments)]
fn prepare_file_write(
    root: &Path,
    stage: &Path,
    stage_relative: &str,
    journal_id: &RecoveryJournalId,
    operation: &'static str,
    target: String,
    content: String,
    create_only: bool,
) -> Result<(&'static str, FilesystemMutationPlan), ModeFilesystemMutationError> {
    let target_path = rooted_target(root, &target)?;
    let target_existed = regular_file_exists(&target_path)?;
    if create_only && target_existed {
        return Err(ModeFilesystemMutationError::new(
            "mode document already exists",
        ));
    }
    if !create_only && !target_existed {
        return Err(ModeFilesystemMutationError::new(
            "mode document does not exist",
        ));
    }
    let candidate = stage.join("candidate.yaml");
    write_synced(&candidate, content.as_bytes())?;
    let candidate_sha256 = sha256_file(&candidate)?;
    Ok((
        operation,
        FilesystemMutationPlan {
            version: PLAN_VERSION,
            journal_id: journal_id.as_str().to_owned(),
            kind: FilesystemMutationKind::WriteFile,
            target,
            stage_directory: stage_relative.to_owned(),
            candidate: Some(format!("{stage_relative}/candidate.yaml")),
            backup: target_existed.then(|| format!("{stage_relative}/backup.yaml")),
            target_existed,
            candidate_sha256: Some(candidate_sha256),
            writes: Vec::new(),
            playlists: Vec::new(),
        },
    ))
}

fn prepare_file_delete(
    root: &Path,
    journal_id: &RecoveryJournalId,
    operation: &'static str,
    target: String,
    stage_relative: &str,
) -> Result<(&'static str, FilesystemMutationPlan), ModeFilesystemMutationError> {
    if !regular_file_exists(&rooted_target(root, &target)?)? {
        return Err(ModeFilesystemMutationError::new(
            "mode document does not exist",
        ));
    }
    Ok((
        operation,
        delete_plan(
            journal_id,
            FilesystemMutationKind::DeleteFile,
            target,
            stage_relative,
        ),
    ))
}

fn delete_plan(
    journal_id: &RecoveryJournalId,
    kind: FilesystemMutationKind,
    target: String,
    stage_relative: &str,
) -> FilesystemMutationPlan {
    FilesystemMutationPlan {
        version: PLAN_VERSION,
        journal_id: journal_id.as_str().to_owned(),
        kind,
        target,
        stage_directory: stage_relative.to_owned(),
        candidate: None,
        backup: Some(format!("{stage_relative}/backup")),
        target_existed: true,
        candidate_sha256: None,
        writes: Vec::new(),
        playlists: Vec::new(),
    }
}

fn apply(root: &Path, journal: &RecoveryJournalEntry) -> Result<(), ModeFilesystemMutationError> {
    let plan = decode_plan(journal)?;
    let target = rooted_target(root, &plan.target)?;
    rooted_artifact(root, &plan.stage_directory, journal.id.as_str())?;
    match plan.kind {
        FilesystemMutationKind::WriteFile => apply_file_write(root, &plan, &target)?,
        FilesystemMutationKind::DeleteFile => {
            let backup = plan_backup(root, &plan, journal)?;
            if backup.exists() {
                if target.exists() {
                    return Err(ModeFilesystemMutationError::new(
                        "mode delete has both target and backup",
                    ));
                }
            } else {
                fs::rename(&target, &backup).map_err(|_| {
                    ModeFilesystemMutationError::new(
                        "mode document could not be staged for deletion",
                    )
                })?;
            }
        }
        FilesystemMutationKind::CreateMode => {
            let candidate = plan_candidate(root, &plan, journal)?;
            if target.exists() {
                if mode_marker_matches(&target, journal.id.as_str())? {
                    return Ok(());
                }
                return Err(ModeFilesystemMutationError::new(
                    "mode create target is occupied",
                ));
            }
            fs::rename(candidate, &target)
                .map_err(|_| ModeFilesystemMutationError::new("mode could not be published"))?;
        }
        FilesystemMutationKind::DeleteMode => {
            let backup = plan_backup(root, &plan, journal)?;
            if backup.exists() {
                if target.exists() {
                    return Err(ModeFilesystemMutationError::new(
                        "mode delete has both target and backup",
                    ));
                }
            } else {
                fs::rename(&target, &backup).map_err(|_| {
                    ModeFilesystemMutationError::new("mode could not be staged for deletion")
                })?;
            }
        }
        FilesystemMutationKind::ImportResources => {
            for write in &plan.writes {
                let write_plan = import_write_plan(&plan, write);
                let target = rooted_target(root, &write.target)?;
                apply_file_write(root, &write_plan, &target)?;
            }
        }
    }
    Ok(())
}

fn apply_file_write(
    root: &Path,
    plan: &FilesystemMutationPlan,
    target: &Path,
) -> Result<(), ModeFilesystemMutationError> {
    let journal_id = &plan.journal_id;
    let candidate = rooted_artifact(
        root,
        plan.candidate.as_deref().ok_or_else(|| {
            ModeFilesystemMutationError::new("mode candidate is missing from the plan")
        })?,
        journal_id,
    )?;
    let expected_hash = plan.candidate_sha256.as_deref().ok_or_else(|| {
        ModeFilesystemMutationError::new("mode candidate hash is missing from the plan")
    })?;
    if plan.target_existed {
        let backup = rooted_artifact(
            root,
            plan.backup.as_deref().ok_or_else(|| {
                ModeFilesystemMutationError::new("mode backup is missing from the plan")
            })?,
            journal_id,
        )?;
        if !backup.exists() {
            if !target.is_file() {
                return Err(ModeFilesystemMutationError::new(
                    "mode replacement target disappeared",
                ));
            }
            fs::rename(target, &backup).map_err(|_| {
                ModeFilesystemMutationError::new("mode replacement backup could not be created")
            })?;
        }
        if candidate.exists() {
            if target.exists() {
                return Err(ModeFilesystemMutationError::new(
                    "mode replacement target is occupied",
                ));
            }
            fs::rename(&candidate, target).map_err(|_| {
                ModeFilesystemMutationError::new("mode replacement could not be published")
            })?;
        }
        if sha256_file(target)? != expected_hash {
            return Err(ModeFilesystemMutationError::new(
                "published mode document hash differs",
            ));
        }
    } else {
        ensure_document_parent(root, target)?;
        if target.exists() {
            if sha256_file(target)? != expected_hash {
                return Err(ModeFilesystemMutationError::new(
                    "mode create target is occupied",
                ));
            }
            if candidate.exists() {
                fs::remove_file(&candidate).map_err(|_| {
                    ModeFilesystemMutationError::new("mode candidate link could not be released")
                })?;
            }
        } else {
            fs::hard_link(&candidate, target).map_err(|_| {
                ModeFilesystemMutationError::new(
                    "mode document could not be published without clobbering",
                )
            })?;
            fs::remove_file(&candidate).map_err(|_| {
                ModeFilesystemMutationError::new("mode candidate link could not be released")
            })?;
        }
    }
    Ok(())
}

fn rollback(
    root: &Path,
    journal: &RecoveryJournalEntry,
) -> Result<(), ModeFilesystemMutationError> {
    let plan = decode_plan(journal)?;
    let target = rooted_target(root, &plan.target)?;
    match plan.kind {
        FilesystemMutationKind::WriteFile => rollback_file_write(root, &plan, &target)?,
        FilesystemMutationKind::DeleteFile => {
            let backup = plan_backup(root, &plan, journal)?;
            restore_backup(&backup, &target)?;
        }
        FilesystemMutationKind::CreateMode => {
            if target.exists() {
                if !mode_marker_matches(&target, journal.id.as_str())? {
                    return Err(ModeFilesystemMutationError::new(
                        "created mode changed before rollback",
                    ));
                }
                fs::remove_dir_all(&target).map_err(|_| {
                    ModeFilesystemMutationError::new("created mode could not be rolled back")
                })?;
            }
        }
        FilesystemMutationKind::DeleteMode => {
            let backup = plan_backup(root, &plan, journal)?;
            restore_backup(&backup, &target)?;
        }
        FilesystemMutationKind::ImportResources => {
            for write in plan.writes.iter().rev() {
                let write_plan = import_write_plan(&plan, write);
                let target = rooted_target(root, &write.target)?;
                rollback_file_write(root, &write_plan, &target)?;
            }
        }
    }
    remove_stage(root, &plan, journal.id.as_str())
}

fn rollback_file_write(
    root: &Path,
    plan: &FilesystemMutationPlan,
    target: &Path,
) -> Result<(), ModeFilesystemMutationError> {
    if plan.target_existed {
        let backup = rooted_artifact(
            root,
            plan.backup.as_deref().ok_or_else(|| {
                ModeFilesystemMutationError::new("mode backup is missing from the plan")
            })?,
            &plan.journal_id,
        )?;
        if backup.exists() {
            if target.exists() {
                let expected_hash = plan.candidate_sha256.as_deref().ok_or_else(|| {
                    ModeFilesystemMutationError::new("mode candidate hash is missing from the plan")
                })?;
                if sha256_file(target)? != expected_hash {
                    return Err(ModeFilesystemMutationError::new(
                        "mode document changed before rollback",
                    ));
                }
                fs::remove_file(target).map_err(|_| {
                    ModeFilesystemMutationError::new("mode replacement could not be removed")
                })?;
            }
            fs::rename(backup, target).map_err(|_| {
                ModeFilesystemMutationError::new("mode backup could not be restored")
            })?;
        }
    } else if target.exists() {
        let candidate = plan_candidate_from_plan(root, plan)?;
        if candidate.exists() {
            return Ok(());
        }
        let expected_hash = plan.candidate_sha256.as_deref().ok_or_else(|| {
            ModeFilesystemMutationError::new("mode candidate hash is missing from the plan")
        })?;
        if sha256_file(target)? != expected_hash {
            return Err(ModeFilesystemMutationError::new(
                "created mode document changed before rollback",
            ));
        }
        fs::remove_file(target).map_err(|_| {
            ModeFilesystemMutationError::new("created mode document could not be rolled back")
        })?;
    }
    Ok(())
}

fn import_write_plan(
    batch: &FilesystemMutationPlan,
    write: &FilesystemWritePlan,
) -> FilesystemMutationPlan {
    FilesystemMutationPlan {
        version: batch.version,
        journal_id: batch.journal_id.clone(),
        kind: FilesystemMutationKind::WriteFile,
        target: write.target.clone(),
        stage_directory: batch.stage_directory.clone(),
        candidate: Some(write.candidate.clone()),
        backup: write.backup.clone(),
        target_existed: write.target_existed,
        candidate_sha256: Some(write.candidate_sha256.clone()),
        writes: Vec::new(),
        playlists: Vec::new(),
    }
}

fn plan_candidate_from_plan(
    root: &Path,
    plan: &FilesystemMutationPlan,
) -> Result<PathBuf, ModeFilesystemMutationError> {
    rooted_artifact(
        root,
        plan.candidate.as_deref().ok_or_else(|| {
            ModeFilesystemMutationError::new("mode candidate is missing from the plan")
        })?,
        &plan.journal_id,
    )
}

fn finish(root: &Path, journal: &RecoveryJournalEntry) -> Result<(), ModeFilesystemMutationError> {
    let plan = decode_plan(journal)?;
    if plan.kind == FilesystemMutationKind::CreateMode {
        let marker = rooted_target(root, &plan.target)?.join(MODE_MARKER);
        if marker.exists() {
            fs::remove_file(marker).map_err(|_| {
                ModeFilesystemMutationError::new("mode publication marker could not be removed")
            })?;
        }
    }
    remove_stage(root, &plan, journal.id.as_str())
}

fn cleanup_orphans(root: &Path) -> Result<(), ModeFilesystemMutationError> {
    let staging = root.join(STAGING_DIRECTORY);
    let metadata = match fs::symlink_metadata(&staging) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(_) => {
            return Err(ModeFilesystemMutationError::new(
                "mode staging root could not be inspected",
            ));
        }
    };
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return Err(ModeFilesystemMutationError::new(
            "mode staging root is unsafe",
        ));
    }
    fs::remove_dir_all(staging)
        .map_err(|_| ModeFilesystemMutationError::new("mode staging root could not be cleaned"))
}

fn decode_plan(
    journal: &RecoveryJournalEntry,
) -> Result<FilesystemMutationPlan, ModeFilesystemMutationError> {
    if journal.domain != RecoveryDomain::Modes {
        return Err(ModeFilesystemMutationError::new(
            "recovery journal has the wrong domain",
        ));
    }
    let plan: FilesystemMutationPlan = serde_json::from_value(journal.plan.clone())
        .map_err(|_| ModeFilesystemMutationError::new("mode recovery plan is invalid"))?;
    if plan.version != PLAN_VERSION
        || plan.journal_id != journal.id.as_str()
        || plan.stage_directory != format!("{STAGING_DIRECTORY}/{}", journal.id.as_str())
    {
        return Err(ModeFilesystemMutationError::new(
            "mode recovery plan identity is invalid",
        ));
    }
    Ok(plan)
}

fn plan_candidate(
    root: &Path,
    plan: &FilesystemMutationPlan,
    journal: &RecoveryJournalEntry,
) -> Result<PathBuf, ModeFilesystemMutationError> {
    rooted_artifact(
        root,
        plan.candidate.as_deref().ok_or_else(|| {
            ModeFilesystemMutationError::new("mode candidate is missing from the plan")
        })?,
        journal.id.as_str(),
    )
}

fn plan_backup(
    root: &Path,
    plan: &FilesystemMutationPlan,
    journal: &RecoveryJournalEntry,
) -> Result<PathBuf, ModeFilesystemMutationError> {
    rooted_artifact(
        root,
        plan.backup.as_deref().ok_or_else(|| {
            ModeFilesystemMutationError::new("mode backup is missing from the plan")
        })?,
        journal.id.as_str(),
    )
}

fn remove_stage(
    root: &Path,
    plan: &FilesystemMutationPlan,
    journal_id: &str,
) -> Result<(), ModeFilesystemMutationError> {
    let stage = rooted_artifact(root, &plan.stage_directory, journal_id)?;
    if stage.exists() {
        fs::remove_dir_all(&stage).map_err(|_| {
            ModeFilesystemMutationError::new("mode mutation staging could not be removed")
        })?;
    }
    let staging_root = root.join(STAGING_DIRECTORY);
    let _ = fs::remove_dir(staging_root);
    Ok(())
}

fn restore_backup(backup: &Path, target: &Path) -> Result<(), ModeFilesystemMutationError> {
    if backup.exists() {
        if target.exists() {
            return Err(ModeFilesystemMutationError::new(
                "mode target was recreated before rollback",
            ));
        }
        fs::rename(backup, target)
            .map_err(|_| ModeFilesystemMutationError::new("mode backup could not be restored"))?;
    }
    Ok(())
}

fn prepare_stage_directory(root: &Path, stage: &Path) -> Result<(), ModeFilesystemMutationError> {
    let staging_root = root.join(STAGING_DIRECTORY);
    match fs::symlink_metadata(&staging_root) {
        Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_dir() => {
            return Err(ModeFilesystemMutationError::new(
                "mode staging root is unsafe",
            ));
        }
        Ok(_) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            fs::create_dir(&staging_root).map_err(|_| {
                ModeFilesystemMutationError::new("mode staging root could not be created")
            })?;
        }
        Err(_) => {
            return Err(ModeFilesystemMutationError::new(
                "mode staging root could not be inspected",
            ));
        }
    }
    fs::create_dir(stage)
        .map_err(|_| ModeFilesystemMutationError::new("mode staging directory already exists"))
}

fn ensure_mode_target_absent(
    root: &Path,
    mode_id: &str,
) -> Result<(), ModeFilesystemMutationError> {
    let target = rooted_target(root, mode_id)?;
    if target.exists() {
        Err(ModeFilesystemMutationError::new("mode already exists"))
    } else {
        Ok(())
    }
}

fn ensure_mode_target(root: &Path, mode_id: &str) -> Result<(), ModeFilesystemMutationError> {
    let target = rooted_target(root, mode_id)?;
    let metadata = fs::symlink_metadata(target)
        .map_err(|_| ModeFilesystemMutationError::new("mode does not exist"))?;
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return Err(ModeFilesystemMutationError::new("mode target is unsafe"));
    }
    Ok(())
}

fn regular_file_exists(path: &Path) -> Result<bool, ModeFilesystemMutationError> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_file() => Err(
            ModeFilesystemMutationError::new("mode document target is unsafe"),
        ),
        Ok(_) => Ok(true),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(_) => Err(ModeFilesystemMutationError::new(
            "mode document target could not be inspected",
        )),
    }
}

fn ensure_document_parent(root: &Path, target: &Path) -> Result<(), ModeFilesystemMutationError> {
    let parent = target
        .parent()
        .ok_or_else(|| ModeFilesystemMutationError::new("mode document parent is missing"))?;
    if parent.exists() {
        let metadata = fs::symlink_metadata(parent).map_err(|_| {
            ModeFilesystemMutationError::new("mode document parent could not be inspected")
        })?;
        if metadata.file_type().is_symlink() || !metadata.is_dir() {
            return Err(ModeFilesystemMutationError::new(
                "mode document parent is unsafe",
            ));
        }
        return Ok(());
    }
    let mode_root = parent
        .parent()
        .ok_or_else(|| ModeFilesystemMutationError::new("mode directory is missing"))?;
    if !mode_root.starts_with(root) || !mode_root.is_dir() {
        return Err(ModeFilesystemMutationError::new(
            "mode directory is unavailable",
        ));
    }
    fs::create_dir(parent)
        .map_err(|_| ModeFilesystemMutationError::new("mode document parent could not be created"))
}

fn rooted_target(root: &Path, relative: &str) -> Result<PathBuf, ModeFilesystemMutationError> {
    let components = validated_relative_components(relative)?;
    if components.is_empty()
        || !valid_slug(&components[0])
        || components.len() > 3
        || (components.len() == 2 && components[1] != "manifest.yaml")
        || (components.len() == 3
            && (!matches!(components[1].as_str(), "soundboards" | "cues" | "presets")
                || !components[2].strip_suffix(".yaml").is_some_and(valid_slug)))
    {
        return Err(ModeFilesystemMutationError::new(
            "mode target path is invalid",
        ));
    }
    Ok(components
        .iter()
        .fold(root.to_path_buf(), |path, component| path.join(component)))
}

fn rooted_artifact(
    root: &Path,
    relative: &str,
    journal_id: &str,
) -> Result<PathBuf, ModeFilesystemMutationError> {
    let components = validated_relative_components(relative)?;
    if components.len() < 2
        || components[0] != STAGING_DIRECTORY
        || components[1] != journal_id
        || components.len() > 3
    {
        return Err(ModeFilesystemMutationError::new(
            "mode artifact path is invalid",
        ));
    }
    Ok(components
        .iter()
        .fold(root.to_path_buf(), |path, component| path.join(component)))
}

fn validated_relative_components(
    relative: &str,
) -> Result<Vec<String>, ModeFilesystemMutationError> {
    if relative.is_empty() || relative.chars().count() > MAX_PLAN_PATH_CHARS {
        return Err(ModeFilesystemMutationError::new(
            "mode plan path is invalid",
        ));
    }
    let path = Path::new(relative);
    let components = path
        .components()
        .map(|component| match component {
            Component::Normal(value) => value
                .to_str()
                .filter(|value| !value.is_empty() && !value.contains('/') && !value.contains('\\'))
                .map(str::to_owned)
                .ok_or_else(|| ModeFilesystemMutationError::new("mode plan path is invalid")),
            _ => Err(ModeFilesystemMutationError::new(
                "mode plan path is invalid",
            )),
        })
        .collect::<Result<Vec<_>, _>>()?;
    Ok(components)
}

fn valid_slug(value: &str) -> bool {
    let mut characters = value.chars();
    let Some(first) = characters.next() else {
        return false;
    };
    value.chars().count() <= 64
        && (first.is_ascii_lowercase() || first.is_ascii_digit())
        && characters.all(|character| {
            character.is_ascii_lowercase()
                || character.is_ascii_digit()
                || matches!(character, '-' | '_')
        })
}

fn write_synced(path: &Path, content: &[u8]) -> Result<(), ModeFilesystemMutationError> {
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .map_err(|_| {
            ModeFilesystemMutationError::new("mode candidate file could not be created")
        })?;
    file.write_all(content)
        .and_then(|()| file.sync_all())
        .map_err(|_| ModeFilesystemMutationError::new("mode candidate file could not be synced"))
}

fn sha256_file(path: &Path) -> Result<String, ModeFilesystemMutationError> {
    let mut file = File::open(path)
        .map_err(|_| ModeFilesystemMutationError::new("mode document could not be hashed"))?;
    let mut hasher = Sha256::new();
    let mut buffer = [0_u8; 16 * 1024];
    loop {
        let read = file
            .read(&mut buffer)
            .map_err(|_| ModeFilesystemMutationError::new("mode document could not be hashed"))?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    Ok(format!("{:x}", hasher.finalize()))
}

fn mode_marker_matches(
    mode_directory: &Path,
    journal_id: &str,
) -> Result<bool, ModeFilesystemMutationError> {
    let marker = mode_directory.join(MODE_MARKER);
    match fs::read_to_string(marker) {
        Ok(value) => Ok(value == journal_id),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(_) => Err(ModeFilesystemMutationError::new(
            "mode publication marker could not be read",
        )),
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::error::Error;

    use music_application::modes::{
        CueDocument, ModeDocument, ModeMutation, ModeMutationEffects, SoundboardDocument,
    };
    use music_application::recovery::{
        RecoveryDomain, RecoveryJournalEntry, RecoveryJournalId, RecoveryState,
    };
    use tempfile::tempdir;

    use super::{FilesystemModeMutations, STAGING_DIRECTORY};

    #[tokio::test]
    async fn create_mode_and_document_are_published_without_partial_visibility()
    -> Result<(), Box<dyn Error + Send + Sync>> {
        let directory = tempdir()?;
        let root = directory.path().join("modes");
        std::fs::create_dir(&root)?;
        let effects = FilesystemModeMutations::open(&root)?;

        let create_id = RecoveryJournalId::new();
        let prepared = effects
            .prepare(
                &create_id,
                &ModeMutation::CreateMode {
                    expected_generation: 1,
                    manifest: manifest("table", "Table"),
                },
            )
            .await?;
        assert!(!root.join("table").exists());
        let applying = entry(create_id, prepared, RecoveryState::Applying);
        effects.apply(&applying).await?;
        assert!(root.join("table/manifest.yaml").is_file());
        assert!(root.join("table/.music-journal-id").is_file());
        effects
            .finish(&with_state(&applying, RecoveryState::Committed))
            .await?;
        assert!(!root.join("table/.music-journal-id").exists());

        let soundboard_id = RecoveryJournalId::new();
        let prepared = effects
            .prepare(
                &soundboard_id,
                &ModeMutation::PutSoundboard {
                    expected_generation: 2,
                    mode_id: "table".to_owned(),
                    soundboard_id: "combat".to_owned(),
                    document: SoundboardDocument {
                        id: Some("combat".to_owned()),
                        name: Some("Combat".to_owned()),
                        categories: Vec::new(),
                        extra: BTreeMap::new(),
                    },
                    create_only: true,
                },
            )
            .await?;
        assert!(!root.join("table/soundboards/combat.yaml").exists());
        let applying = entry(soundboard_id, prepared, RecoveryState::Applying);
        effects.apply(&applying).await?;
        assert!(root.join("table/soundboards/combat.yaml").is_file());
        effects
            .finish(&with_state(&applying, RecoveryState::Committed))
            .await?;
        assert!(!root.join(STAGING_DIRECTORY).exists());
        Ok(())
    }

    #[tokio::test]
    async fn replacement_rollback_restores_the_exact_previous_document()
    -> Result<(), Box<dyn Error + Send + Sync>> {
        let directory = tempdir()?;
        let root = directory.path().join("modes");
        create_mode_fixture(&root, "table", "Original")?;
        let original = std::fs::read(root.join("table/manifest.yaml"))?;
        let effects = FilesystemModeMutations::open(&root)?;
        let journal_id = RecoveryJournalId::new();
        let prepared = effects
            .prepare(
                &journal_id,
                &ModeMutation::PutManifest {
                    expected_generation: 1,
                    mode_id: "table".to_owned(),
                    manifest: manifest("table", "Changed"),
                },
            )
            .await?;
        let applying = entry(journal_id, prepared, RecoveryState::Applying);
        effects.apply(&applying).await?;
        assert_ne!(std::fs::read(root.join("table/manifest.yaml"))?, original);

        effects
            .rollback(&with_state(&applying, RecoveryState::RollingBack))
            .await?;
        assert_eq!(std::fs::read(root.join("table/manifest.yaml"))?, original);
        assert!(!root.join(STAGING_DIRECTORY).exists());
        Ok(())
    }

    #[tokio::test]
    async fn delete_mode_rollback_restores_the_directory()
    -> Result<(), Box<dyn Error + Send + Sync>> {
        let directory = tempdir()?;
        let root = directory.path().join("modes");
        create_mode_fixture(&root, "table", "Table")?;
        std::fs::write(root.join("table/theme.css"), "body { color: ivory; }")?;
        let effects = FilesystemModeMutations::open(&root)?;
        let journal_id = RecoveryJournalId::new();
        let prepared = effects
            .prepare(
                &journal_id,
                &ModeMutation::DeleteMode {
                    expected_generation: 1,
                    mode_id: "table".to_owned(),
                },
            )
            .await?;
        let applying = entry(journal_id, prepared, RecoveryState::Applying);
        effects.apply(&applying).await?;
        assert!(!root.join("table").exists());

        effects
            .rollback(&with_state(&applying, RecoveryState::RollingBack))
            .await?;
        assert_eq!(
            std::fs::read_to_string(root.join("table/theme.css"))?,
            "body { color: ivory; }"
        );
        Ok(())
    }

    #[tokio::test]
    async fn import_batch_rolls_back_prior_writes_without_clobbering_a_late_collision()
    -> Result<(), Box<dyn Error + Send + Sync>> {
        let directory = tempdir()?;
        let root = directory.path().join("modes");
        create_mode_fixture(&root, "table", "Original")?;
        let original = std::fs::read(root.join("table/manifest.yaml"))?;
        let effects = FilesystemModeMutations::open(&root)?;
        let journal_id = RecoveryJournalId::new();
        let prepared = effects
            .prepare(
                &journal_id,
                &ModeMutation::ImportResources {
                    expected_generation: 1,
                    mode_id: "table".to_owned(),
                    manifest: manifest("table", "Changed"),
                    soundboards: BTreeMap::from([(
                        "storms".to_owned(),
                        SoundboardDocument {
                            id: Some("storms".to_owned()),
                            name: Some("Storms".to_owned()),
                            categories: Vec::new(),
                            extra: BTreeMap::new(),
                        },
                    )]),
                    cues: BTreeMap::from([(
                        "arrival".to_owned(),
                        CueDocument {
                            id: Some("arrival".to_owned()),
                            name: "Arrival".to_owned(),
                            description: None,
                            preset: None,
                            playlist: None,
                            start_index: 0,
                            start_ms: 0,
                            sfx: Vec::new(),
                            loops: Vec::new(),
                            extra: BTreeMap::new(),
                        },
                    )]),
                    presets: BTreeMap::new(),
                    playlists: Vec::new(),
                },
            )
            .await?;
        std::fs::write(
            root.join("table/cues/arrival.yaml"),
            "id: arrival\nname: Externally created\n",
        )?;
        let applying = entry(journal_id, prepared, RecoveryState::Applying);
        assert!(effects.apply(&applying).await.is_err());
        effects
            .rollback(&with_state(&applying, RecoveryState::RollingBack))
            .await?;

        assert_eq!(std::fs::read(root.join("table/manifest.yaml"))?, original);
        assert!(!root.join("table/soundboards/storms.yaml").exists());
        assert!(
            std::fs::read_to_string(root.join("table/cues/arrival.yaml"))?
                .contains("Externally created")
        );
        assert!(!root.join(STAGING_DIRECTORY).exists());
        Ok(())
    }

    fn create_mode_fixture(root: &std::path::Path, id: &str, name: &str) -> std::io::Result<()> {
        std::fs::create_dir_all(root.join(id).join("soundboards"))?;
        std::fs::create_dir(root.join(id).join("cues"))?;
        std::fs::create_dir(root.join(id).join("presets"))?;
        std::fs::write(
            root.join(id).join("manifest.yaml"),
            format!("id: {id}\nname: {name}\n"),
        )
    }

    fn manifest(id: &str, name: &str) -> ModeDocument {
        ModeDocument {
            id: id.to_owned(),
            name: name.to_owned(),
            theme: None,
            panels: Vec::new(),
            playlist_categories: Vec::new(),
            interrupts: Vec::new(),
            integrations: Default::default(),
            default_crossfade_ms: 0,
            default_soundboard: None,
            extra: BTreeMap::new(),
        }
    }

    fn entry(
        id: RecoveryJournalId,
        prepared: music_application::modes::PreparedModeMutation,
        state: RecoveryState,
    ) -> RecoveryJournalEntry {
        RecoveryJournalEntry {
            id,
            domain: RecoveryDomain::Modes,
            operation: prepared.operation,
            state,
            plan: prepared.plan,
            progress: serde_json::json!({}),
            created_at_unix_seconds: 1,
            updated_at_unix_seconds: 1,
            completed_at_unix_seconds: state.is_terminal().then_some(1),
        }
    }

    fn with_state(entry: &RecoveryJournalEntry, state: RecoveryState) -> RecoveryJournalEntry {
        RecoveryJournalEntry {
            state,
            completed_at_unix_seconds: state.is_terminal().then_some(2),
            ..entry.clone()
        }
    }
}
