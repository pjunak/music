use std::error::Error;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use serde_json::Value;

use crate::recovery::{RecoveryJournalEntry, RecoveryJournalId, RecoveryOperation};

use super::{CueDocument, ModeCatalog, ModeDocument, PresetDocument, SoundboardDocument};

pub type ModeMutationDependencyError = Box<dyn Error + Send + Sync>;
pub type ModeMutationFuture<'a, T> =
    Pin<Box<dyn Future<Output = Result<T, ModeMutationDependencyError>> + Send + 'a>>;

#[derive(Debug, Clone, PartialEq)]
pub enum ModeMutation {
    CreateMode {
        expected_generation: u64,
        manifest: ModeDocument,
    },
    DeleteMode {
        expected_generation: u64,
        mode_id: String,
    },
    PutManifest {
        expected_generation: u64,
        mode_id: String,
        manifest: ModeDocument,
    },
    PutSoundboard {
        expected_generation: u64,
        mode_id: String,
        soundboard_id: String,
        document: SoundboardDocument,
        create_only: bool,
    },
    DeleteSoundboard {
        expected_generation: u64,
        mode_id: String,
        soundboard_id: String,
    },
    PutCue {
        expected_generation: u64,
        mode_id: String,
        cue_id: String,
        document: CueDocument,
        create_only: bool,
    },
    DeleteCue {
        expected_generation: u64,
        mode_id: String,
        cue_id: String,
    },
    PutPreset {
        expected_generation: u64,
        mode_id: String,
        preset_id: String,
        document: PresetDocument,
        create_only: bool,
    },
    DeletePreset {
        expected_generation: u64,
        mode_id: String,
        preset_id: String,
    },
}

impl ModeMutation {
    #[must_use]
    pub const fn expected_generation(&self) -> u64 {
        match self {
            Self::CreateMode {
                expected_generation,
                ..
            }
            | Self::DeleteMode {
                expected_generation,
                ..
            }
            | Self::PutManifest {
                expected_generation,
                ..
            }
            | Self::PutSoundboard {
                expected_generation,
                ..
            }
            | Self::DeleteSoundboard {
                expected_generation,
                ..
            }
            | Self::PutCue {
                expected_generation,
                ..
            }
            | Self::DeleteCue {
                expected_generation,
                ..
            }
            | Self::PutPreset {
                expected_generation,
                ..
            }
            | Self::DeletePreset {
                expected_generation,
                ..
            } => *expected_generation,
        }
    }

    #[must_use]
    pub fn mode_id(&self) -> &str {
        match self {
            Self::CreateMode { manifest, .. } => &manifest.id,
            Self::DeleteMode { mode_id, .. }
            | Self::PutManifest { mode_id, .. }
            | Self::PutSoundboard { mode_id, .. }
            | Self::DeleteSoundboard { mode_id, .. }
            | Self::PutCue { mode_id, .. }
            | Self::DeleteCue { mode_id, .. }
            | Self::PutPreset { mode_id, .. }
            | Self::DeletePreset { mode_id, .. } => mode_id,
        }
    }

    #[must_use]
    pub const fn deletes_mode(&self) -> bool {
        matches!(self, Self::DeleteMode { .. })
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct PreparedModeMutation {
    pub operation: RecoveryOperation,
    pub plan: Value,
}

pub trait ModeMutationEffects: std::fmt::Debug + Send + Sync + 'static {
    fn prepare<'a>(
        &'a self,
        journal_id: &'a RecoveryJournalId,
        mutation: &'a ModeMutation,
    ) -> ModeMutationFuture<'a, PreparedModeMutation>;

    fn apply<'a>(&'a self, journal: &'a RecoveryJournalEntry) -> ModeMutationFuture<'a, ()>;

    fn rollback<'a>(&'a self, journal: &'a RecoveryJournalEntry) -> ModeMutationFuture<'a, ()>;

    fn finish<'a>(&'a self, journal: &'a RecoveryJournalEntry) -> ModeMutationFuture<'a, ()>;

    fn cleanup_orphans(&self) -> ModeMutationFuture<'_, ()>;
}

#[derive(Debug, Clone, PartialEq)]
pub struct ModeMutationReport {
    pub catalog: Arc<ModeCatalog>,
}

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum ModeMutationFailureKind {
    Stale,
    Invalid,
    NotFound,
    Conflict,
    Unavailable,
}

#[derive(Debug, Clone, Eq, PartialEq)]
pub struct ModeMutationError {
    pub kind: ModeMutationFailureKind,
    pub code: &'static str,
}

impl ModeMutationError {
    #[must_use]
    pub const fn new(kind: ModeMutationFailureKind, code: &'static str) -> Self {
        Self { kind, code }
    }
}

impl std::fmt::Display for ModeMutationError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.code)
    }
}

impl Error for ModeMutationError {}
