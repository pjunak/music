use std::collections::{BTreeMap, BTreeSet};
use std::error::Error;
use std::fmt::{self, Display, Formatter};
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio::sync::{mpsc, oneshot, watch};
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

use crate::playback::PlaybackActorHandle;
use crate::recovery::{
    RecoveryDomain, RecoveryJournalDraft, RecoveryJournalEntry, RecoveryJournalId,
    RecoveryJournalRepository, RecoveryState, RecoveryTransition,
};

mod mutation;

pub use mutation::{
    ModeMutation, ModeMutationDependencyError, ModeMutationEffects, ModeMutationError,
    ModeMutationFailureKind, ModeMutationFuture, ModeMutationReport, PreparedModeMutation,
};

const MODE_COMMAND_CAPACITY: usize = 16;
const MAX_MODE_ERRORS: usize = 128;
const MAX_MODE_ERROR_CHARS: usize = 512;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct InterruptDocument {
    pub name: String,
    #[serde(default)]
    pub playlist: Option<String>,
    #[serde(default)]
    pub soundboard_item: Option<String>,
    #[serde(default)]
    pub fade_in_ms: i64,
    #[serde(default)]
    pub fade_out_ms: i64,
    #[serde(default = "default_true")]
    pub return_to_ambient: bool,
    #[serde(default)]
    pub duck_to: Option<f64>,
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct IntegrationsDocument {
    #[serde(default)]
    pub lights: Option<BTreeMap<String, Value>>,
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ModeDocument {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub theme: Option<String>,
    #[serde(default)]
    pub panels: Vec<String>,
    #[serde(default)]
    pub playlist_categories: Vec<String>,
    #[serde(default)]
    pub interrupts: Vec<InterruptDocument>,
    #[serde(default)]
    pub integrations: IntegrationsDocument,
    #[serde(default)]
    pub default_crossfade_ms: i64,
    #[serde(default)]
    pub default_soundboard: Option<String>,
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SoundboardItemDocument {
    pub file: String,
    pub name: String,
    #[serde(default)]
    pub icon: Option<String>,
    #[serde(default)]
    pub hotkey: Option<String>,
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SoundboardCategoryDocument {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub items: Vec<SoundboardItemDocument>,
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SoundboardDocument {
    #[serde(default)]
    pub id: Option<String>,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub categories: Vec<SoundboardCategoryDocument>,
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CueSfxDocument {
    pub soundboard: String,
    pub item: String,
    #[serde(default = "default_volume")]
    pub volume: f64,
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CueLoopDocument {
    pub soundboard: String,
    pub item: String,
    pub interval_s: f64,
    #[serde(default = "default_volume")]
    pub volume: f64,
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CueDocument {
    #[serde(default)]
    pub id: Option<String>,
    pub name: String,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub preset: Option<String>,
    #[serde(default)]
    pub playlist: Option<String>,
    #[serde(default)]
    pub start_index: u64,
    #[serde(default)]
    pub start_ms: u64,
    #[serde(default)]
    pub sfx: Vec<CueSfxDocument>,
    #[serde(default)]
    pub loops: Vec<CueLoopDocument>,
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EffectDocument {
    #[serde(rename = "type")]
    pub effect_type: String,
    #[serde(flatten)]
    pub parameters: BTreeMap<String, Value>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PresetDocument {
    #[serde(default)]
    pub id: Option<String>,
    pub name: String,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub effects: Vec<EffectDocument>,
    #[serde(default)]
    pub crossfade_ms: Option<u64>,
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ModeBundle {
    pub manifest: ModeDocument,
    pub soundboards: BTreeMap<String, SoundboardDocument>,
    pub cues: BTreeMap<String, CueDocument>,
    pub presets: BTreeMap<String, PresetDocument>,
    pub theme_css: Option<Arc<str>>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ModeCatalog {
    pub generation: u64,
    pub modes: BTreeMap<String, ModeBundle>,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct ModeLoadAttempt {
    pub modes: BTreeMap<String, ModeBundle>,
    pub errors: BTreeMap<String, String>,
}

#[derive(Debug, Clone, Eq, PartialEq)]
pub struct ModeSourceError {
    detail: &'static str,
}

impl ModeSourceError {
    #[must_use]
    pub const fn new(detail: &'static str) -> Self {
        Self { detail }
    }
}

impl Display for ModeSourceError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.detail)
    }
}

impl Error for ModeSourceError {}

pub type ModeSourceFuture<'a> =
    Pin<Box<dyn Future<Output = Result<ModeLoadAttempt, ModeSourceError>> + Send + 'a>>;

pub trait ModeCatalogSource: Send + Sync + 'static {
    fn load<'a>(&'a self) -> ModeSourceFuture<'a>;
}

#[derive(Debug, Clone, Eq, PartialEq, Ord, PartialOrd)]
pub struct ModePresetKey {
    pub mode_id: String,
    pub preset_id: String,
}

#[derive(Debug, Clone, Default, Eq, PartialEq)]
pub struct ModePlaybackReferences {
    pub soundboard_ids: BTreeSet<String>,
    pub preset_ids: BTreeSet<String>,
}

#[derive(Debug, Clone, Eq, PartialEq)]
pub struct ModeCatalogPublication {
    pub generation: u64,
    pub modes: BTreeMap<String, ModePlaybackReferences>,
    pub changed_presets: BTreeSet<ModePresetKey>,
}

#[derive(Debug, Clone, Eq, PartialEq)]
pub struct ModeCatalogSinkError {
    detail: &'static str,
}

impl ModeCatalogSinkError {
    #[must_use]
    pub const fn new(detail: &'static str) -> Self {
        Self { detail }
    }
}

impl Display for ModeCatalogSinkError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.detail)
    }
}

impl Error for ModeCatalogSinkError {}

pub type ModeCatalogSinkFuture<'a> =
    Pin<Box<dyn Future<Output = Result<(), ModeCatalogSinkError>> + Send + 'a>>;

pub trait ModeCatalogSink: Send + Sync + 'static {
    fn publish<'a>(&'a self, publication: ModeCatalogPublication) -> ModeCatalogSinkFuture<'a>;
}

impl ModeCatalogSink for PlaybackActorHandle {
    fn publish<'a>(&'a self, publication: ModeCatalogPublication) -> ModeCatalogSinkFuture<'a> {
        Box::pin(async move {
            self.replace_mode_catalog(publication)
                .await
                .map(|_| ())
                .map_err(|_| ModeCatalogSinkError::new("playback actor rejected the mode catalog"))
        })
    }
}

#[derive(Debug, Clone, Copy, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ModeLoadState {
    Starting,
    Current,
    Degraded,
    Failed,
}

#[derive(Debug, Clone, Eq, PartialEq, Serialize)]
pub struct ModeStatus {
    pub state: ModeLoadState,
    pub generation: u64,
    pub last_load_at_unix_seconds: Option<u64>,
    pub loaded_ids: Vec<String>,
    pub errors: BTreeMap<String, String>,
}

impl Default for ModeStatus {
    fn default() -> Self {
        Self {
            state: ModeLoadState::Starting,
            generation: 0,
            last_load_at_unix_seconds: None,
            loaded_ids: Vec::new(),
            errors: BTreeMap::new(),
        }
    }
}

#[derive(Debug, Clone, Eq, PartialEq)]
pub struct ModeReloadReport {
    pub loaded_ids: Vec<String>,
    pub errors: BTreeMap<String, String>,
    pub published: bool,
    pub generation: u64,
}

#[derive(Debug, Clone)]
pub enum ModeCoordinatorError {
    CommandClosed,
    ReplyDropped,
    GenerationOverflow,
    CatalogSink(ModeCatalogSinkError),
    MutationRecovery(&'static str),
}

impl Display for ModeCoordinatorError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        match self {
            Self::CommandClosed => formatter.write_str("mode coordinator is unavailable"),
            Self::ReplyDropped => formatter.write_str("mode coordinator dropped its reply"),
            Self::GenerationOverflow => formatter.write_str("mode catalog generation overflowed"),
            Self::CatalogSink(error) => Display::fmt(error, formatter),
            Self::MutationRecovery(detail) => formatter.write_str(detail),
        }
    }
}

impl Error for ModeCoordinatorError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::CatalogSink(source) => Some(source),
            Self::CommandClosed
            | Self::ReplyDropped
            | Self::GenerationOverflow
            | Self::MutationRecovery(_) => None,
        }
    }
}

impl From<ModeCatalogSinkError> for ModeCoordinatorError {
    fn from(error: ModeCatalogSinkError) -> Self {
        Self::CatalogSink(error)
    }
}

#[derive(Debug, Clone)]
pub struct ModeCoordinatorHandle {
    commands: mpsc::Sender<ModeCommand>,
    catalog: watch::Receiver<Option<Arc<ModeCatalog>>>,
    status: watch::Receiver<ModeStatus>,
    cancellation: CancellationToken,
}

impl ModeCoordinatorHandle {
    #[must_use]
    pub fn snapshot(&self) -> Option<Arc<ModeCatalog>> {
        self.catalog.borrow().clone()
    }

    #[must_use]
    pub fn status(&self) -> ModeStatus {
        self.status.borrow().clone()
    }

    #[must_use]
    pub fn subscribe_status(&self) -> watch::Receiver<ModeStatus> {
        self.status.clone()
    }

    pub async fn wait_until_initialized(&self) -> Result<ModeStatus, ModeCoordinatorError> {
        let mut status = self.status.clone();
        loop {
            let current = status.borrow().clone();
            if current.state != ModeLoadState::Starting {
                return Ok(current);
            }
            status
                .changed()
                .await
                .map_err(|_| ModeCoordinatorError::CommandClosed)?;
        }
    }

    pub async fn reload(&self) -> Result<ModeReloadReport, ModeCoordinatorError> {
        let (reply, response) = oneshot::channel();
        self.commands
            .send(ModeCommand::Reload { reply })
            .await
            .map_err(|_| ModeCoordinatorError::CommandClosed)?;
        response
            .await
            .map_err(|_| ModeCoordinatorError::ReplyDropped)?
    }

    pub async fn mutate(
        &self,
        mutation: ModeMutation,
    ) -> Result<ModeMutationReport, ModeMutationError> {
        let (reply, response) = oneshot::channel();
        self.commands
            .send(ModeCommand::Mutate {
                mutation: Box::new(mutation),
                reply,
            })
            .await
            .map_err(|_| {
                ModeMutationError::new(
                    ModeMutationFailureKind::Unavailable,
                    "mode coordinator is unavailable",
                )
            })?;
        response.await.map_err(|_| {
            ModeMutationError::new(
                ModeMutationFailureKind::Unavailable,
                "mode coordinator dropped its mutation reply",
            )
        })?
    }

    pub fn shutdown(&self) {
        self.cancellation.cancel();
    }
}

#[derive(Debug)]
pub struct SpawnedModeCoordinator {
    pub handle: ModeCoordinatorHandle,
    pub task: JoinHandle<Result<(), ModeCoordinatorError>>,
}

pub fn start_mode_coordinator(
    source: Arc<dyn ModeCatalogSource>,
    sink: Arc<dyn ModeCatalogSink>,
) -> SpawnedModeCoordinator {
    spawn_mode_coordinator(source, sink, None)
}

pub async fn start_mutable_mode_coordinator(
    source: Arc<dyn ModeCatalogSource>,
    sink: Arc<dyn ModeCatalogSink>,
    journal: Arc<dyn RecoveryJournalRepository>,
    effects: Arc<dyn ModeMutationEffects>,
) -> Result<SpawnedModeCoordinator, ModeCoordinatorError> {
    recover_mode_mutations(journal.as_ref(), effects.as_ref()).await?;
    Ok(spawn_mode_coordinator(
        source,
        sink,
        Some(MutationDependencies { journal, effects }),
    ))
}

fn spawn_mode_coordinator(
    source: Arc<dyn ModeCatalogSource>,
    sink: Arc<dyn ModeCatalogSink>,
    mutations: Option<MutationDependencies>,
) -> SpawnedModeCoordinator {
    let (commands, receiver) = mpsc::channel(MODE_COMMAND_CAPACITY);
    let (catalog_sender, catalog) = watch::channel(None);
    let (status_sender, status) = watch::channel(ModeStatus::default());
    let cancellation = CancellationToken::new();
    let handle = ModeCoordinatorHandle {
        commands,
        catalog,
        status,
        cancellation: cancellation.clone(),
    };
    let task = tokio::spawn(run_mode_coordinator(
        source,
        sink,
        receiver,
        catalog_sender,
        status_sender,
        cancellation,
        mutations,
    ));
    SpawnedModeCoordinator { handle, task }
}

#[derive(Debug)]
enum ModeCommand {
    Reload {
        reply: oneshot::Sender<Result<ModeReloadReport, ModeCoordinatorError>>,
    },
    Mutate {
        mutation: Box<ModeMutation>,
        reply: oneshot::Sender<Result<ModeMutationReport, ModeMutationError>>,
    },
}

#[derive(Clone)]
struct MutationDependencies {
    journal: Arc<dyn RecoveryJournalRepository>,
    effects: Arc<dyn ModeMutationEffects>,
}

async fn run_mode_coordinator(
    source: Arc<dyn ModeCatalogSource>,
    sink: Arc<dyn ModeCatalogSink>,
    mut commands: mpsc::Receiver<ModeCommand>,
    catalog_sender: watch::Sender<Option<Arc<ModeCatalog>>>,
    status_sender: watch::Sender<ModeStatus>,
    cancellation: CancellationToken,
    mutations: Option<MutationDependencies>,
) -> Result<(), ModeCoordinatorError> {
    let mut catalog = None;
    perform_reload(
        source.as_ref(),
        sink.as_ref(),
        &mut catalog,
        &catalog_sender,
        &status_sender,
    )
    .await?;

    loop {
        let command = tokio::select! {
            () = cancellation.cancelled() => return Ok(()),
            command = commands.recv() => {
                let Some(command) = command else {
                    return Ok(());
                };
                command
            }
        };
        match command {
            ModeCommand::Reload { reply } => {
                let result = perform_reload(
                    source.as_ref(),
                    sink.as_ref(),
                    &mut catalog,
                    &catalog_sender,
                    &status_sender,
                )
                .await;
                let fatal = result.as_ref().err().cloned();
                let _ = reply.send(result);
                if let Some(error) = fatal {
                    return Err(error);
                }
            }
            ModeCommand::Mutate { mutation, reply } => {
                let Some(dependencies) = mutations.as_ref() else {
                    let _ = reply.send(Err(ModeMutationError::new(
                        ModeMutationFailureKind::Unavailable,
                        "mode mutations are not configured",
                    )));
                    continue;
                };
                match perform_mutation(
                    source.as_ref(),
                    sink.as_ref(),
                    dependencies,
                    &mut catalog,
                    &catalog_sender,
                    &status_sender,
                    *mutation,
                )
                .await
                {
                    Ok(report) => {
                        let _ = reply.send(Ok(report));
                    }
                    Err(ModeMutationExecutionError::Rejected(error)) => {
                        let _ = reply.send(Err(error));
                    }
                    Err(ModeMutationExecutionError::Fatal(error)) => {
                        let _ = reply.send(Err(ModeMutationError::new(
                            ModeMutationFailureKind::Unavailable,
                            "mode mutation recovery failed",
                        )));
                        return Err(error);
                    }
                }
            }
        }
    }
}

async fn perform_reload(
    source: &dyn ModeCatalogSource,
    sink: &dyn ModeCatalogSink,
    current: &mut Option<Arc<ModeCatalog>>,
    catalog_sender: &watch::Sender<Option<Arc<ModeCatalog>>>,
    status_sender: &watch::Sender<ModeStatus>,
) -> Result<ModeReloadReport, ModeCoordinatorError> {
    let loaded_at = current_unix_seconds();
    let attempt = match source.load().await {
        Ok(attempt) => attempt,
        Err(error) => {
            let errors = bounded_errors(BTreeMap::from([("<root>".to_owned(), error.to_string())]));
            let generation = current.as_ref().map_or(0, |catalog| catalog.generation);
            status_sender.send_replace(ModeStatus {
                state: if current.is_some() {
                    ModeLoadState::Degraded
                } else {
                    ModeLoadState::Failed
                },
                generation,
                last_load_at_unix_seconds: Some(loaded_at),
                loaded_ids: Vec::new(),
                errors: errors.clone(),
            });
            return Ok(ModeReloadReport {
                loaded_ids: Vec::new(),
                errors,
                published: false,
                generation,
            });
        }
    };
    let loaded_ids = attempt.modes.keys().cloned().collect::<Vec<_>>();
    let errors = bounded_errors(attempt.errors);
    let current_generation = current.as_ref().map_or(0, |catalog| catalog.generation);
    if !errors.is_empty() {
        status_sender.send_replace(ModeStatus {
            state: if current.is_some() {
                ModeLoadState::Degraded
            } else {
                ModeLoadState::Failed
            },
            generation: current_generation,
            last_load_at_unix_seconds: Some(loaded_at),
            loaded_ids: loaded_ids.clone(),
            errors: errors.clone(),
        });
        return Ok(ModeReloadReport {
            loaded_ids,
            errors,
            published: false,
            generation: current_generation,
        });
    }

    let generation = current_generation
        .checked_add(1)
        .ok_or(ModeCoordinatorError::GenerationOverflow)?;
    let changed_presets = changed_presets(current.as_deref(), &attempt.modes);
    sink.publish(ModeCatalogPublication {
        generation,
        modes: playback_references(&attempt.modes),
        changed_presets,
    })
    .await?;
    let next = Arc::new(ModeCatalog {
        generation,
        modes: attempt.modes,
    });
    *current = Some(Arc::clone(&next));
    catalog_sender.send_replace(Some(next));
    status_sender.send_replace(ModeStatus {
        state: ModeLoadState::Current,
        generation,
        last_load_at_unix_seconds: Some(loaded_at),
        loaded_ids: loaded_ids.clone(),
        errors: BTreeMap::new(),
    });
    Ok(ModeReloadReport {
        loaded_ids,
        errors: BTreeMap::new(),
        published: true,
        generation,
    })
}

#[derive(Debug)]
enum ModeMutationExecutionError {
    Rejected(ModeMutationError),
    Fatal(ModeCoordinatorError),
}

impl From<ModeMutationError> for ModeMutationExecutionError {
    fn from(error: ModeMutationError) -> Self {
        Self::Rejected(error)
    }
}

async fn perform_mutation(
    source: &dyn ModeCatalogSource,
    sink: &dyn ModeCatalogSink,
    dependencies: &MutationDependencies,
    current: &mut Option<Arc<ModeCatalog>>,
    catalog_sender: &watch::Sender<Option<Arc<ModeCatalog>>>,
    status_sender: &watch::Sender<ModeStatus>,
    mutation: ModeMutation,
) -> Result<ModeMutationReport, ModeMutationExecutionError> {
    let current_catalog = current.as_deref().ok_or_else(|| {
        ModeMutationError::new(
            ModeMutationFailureKind::Unavailable,
            "mode catalog is not loaded",
        )
    })?;
    if mutation.expected_generation() != current_catalog.generation {
        return Err(ModeMutationError::new(
            ModeMutationFailureKind::Stale,
            "mode catalog generation changed",
        )
        .into());
    }
    let generation =
        current_catalog
            .generation
            .checked_add(1)
            .ok_or(ModeMutationExecutionError::Fatal(
                ModeCoordinatorError::GenerationOverflow,
            ))?;
    let mut proposed_modes = apply_catalog_mutation(current_catalog, &mutation)?;
    let target_mode_id = mutation.mode_id().to_owned();
    let deletes_mode = mutation.deletes_mode();
    let journal_id = RecoveryJournalId::new();
    let prepared = dependencies
        .effects
        .prepare(&journal_id, &mutation)
        .await
        .map_err(|_| {
            ModeMutationExecutionError::Rejected(ModeMutationError::new(
                ModeMutationFailureKind::Unavailable,
                "mode mutation could not be staged",
            ))
        })?;
    let mut draft =
        RecoveryJournalDraft::new(RecoveryDomain::Modes, prepared.operation, prepared.plan)
            .map_err(|_| {
                ModeMutationExecutionError::Fatal(ModeCoordinatorError::MutationRecovery(
                    "mode mutation journal plan is invalid",
                ))
            })?;
    draft.id = journal_id;
    let planned = match dependencies.journal.create_recovery_journal(draft).await {
        Ok(planned) => planned,
        Err(_) => {
            dependencies.effects.cleanup_orphans().await.map_err(|_| {
                ModeMutationExecutionError::Fatal(ModeCoordinatorError::MutationRecovery(
                    "unjournaled mode staging could not be cleaned",
                ))
            })?;
            return Err(ModeMutationExecutionError::Fatal(
                ModeCoordinatorError::MutationRecovery(
                    "mode mutation journal could not be created",
                ),
            ));
        }
    };
    let applying = transition_mode_journal(
        dependencies.journal.as_ref(),
        &planned,
        RecoveryState::Applying,
        serde_json::json!({"stage": "applying"}),
    )
    .await
    .map_err(ModeMutationExecutionError::Fatal)?;

    if dependencies.effects.apply(&applying).await.is_err() {
        rollback_mode_mutation(dependencies, &applying).await?;
        return Err(ModeMutationError::new(
            ModeMutationFailureKind::Unavailable,
            "mode filesystem mutation failed",
        )
        .into());
    }

    let attempt = match source.load().await {
        Ok(attempt) => attempt,
        Err(_) => {
            rollback_mode_mutation(dependencies, &applying).await?;
            return Err(ModeMutationError::new(
                ModeMutationFailureKind::Unavailable,
                "mode mutation could not be verified",
            )
            .into());
        }
    };
    if deletes_mode {
        if attempt.modes.contains_key(&target_mode_id)
            || attempt.errors.contains_key(&target_mode_id)
        {
            rollback_mode_mutation(dependencies, &applying).await?;
            return Err(ModeMutationError::new(
                ModeMutationFailureKind::Unavailable,
                "deleted mode remained visible on disk",
            )
            .into());
        }
    } else {
        let Some(disk_mode) = attempt.modes.get(&target_mode_id).cloned() else {
            rollback_mode_mutation(dependencies, &applying).await?;
            return Err(ModeMutationError::new(
                ModeMutationFailureKind::Unavailable,
                "written mode did not pass catalog validation",
            )
            .into());
        };
        proposed_modes.insert(target_mode_id, disk_mode);
    }

    let committed = match transition_mode_journal(
        dependencies.journal.as_ref(),
        &applying,
        RecoveryState::Committed,
        serde_json::json!({"catalog_generation": generation}),
    )
    .await
    {
        Ok(committed) => committed,
        Err(error) => {
            let _ = dependencies.effects.rollback(&applying).await;
            return Err(ModeMutationExecutionError::Fatal(error));
        }
    };
    let changed_presets = changed_presets(Some(current_catalog), &proposed_modes);
    sink.publish(ModeCatalogPublication {
        generation,
        modes: playback_references(&proposed_modes),
        changed_presets,
    })
    .await
    .map_err(|error| ModeMutationExecutionError::Fatal(error.into()))?;
    let next = Arc::new(ModeCatalog {
        generation,
        modes: proposed_modes,
    });
    *current = Some(Arc::clone(&next));
    catalog_sender.send_replace(Some(Arc::clone(&next)));
    let errors = bounded_errors(attempt.errors);
    status_sender.send_replace(ModeStatus {
        state: if errors.is_empty() {
            ModeLoadState::Current
        } else {
            ModeLoadState::Degraded
        },
        generation,
        last_load_at_unix_seconds: Some(current_unix_seconds()),
        loaded_ids: attempt.modes.keys().cloned().collect(),
        errors,
    });
    let _ = dependencies.effects.finish(&committed).await;
    Ok(ModeMutationReport { catalog: next })
}

fn apply_catalog_mutation(
    current: &ModeCatalog,
    mutation: &ModeMutation,
) -> Result<BTreeMap<String, ModeBundle>, ModeMutationError> {
    let mut modes = current.modes.clone();
    match mutation {
        ModeMutation::CreateMode { manifest, .. } => {
            validate_slug(&manifest.id)?;
            validate_manifest(manifest, &BTreeMap::new())?;
            if modes.contains_key(&manifest.id) {
                return Err(conflict("mode already exists"));
            }
            modes.insert(
                manifest.id.clone(),
                ModeBundle {
                    manifest: manifest.clone(),
                    soundboards: BTreeMap::new(),
                    cues: BTreeMap::new(),
                    presets: BTreeMap::new(),
                    theme_css: None,
                },
            );
        }
        ModeMutation::DeleteMode { mode_id, .. } => {
            validate_slug(mode_id)?;
            if modes.remove(mode_id).is_none() {
                return Err(not_found("mode not found"));
            }
        }
        ModeMutation::PutManifest {
            mode_id, manifest, ..
        } => {
            validate_slug(mode_id)?;
            if manifest.id != *mode_id {
                return Err(invalid("mode manifest id does not match the mode"));
            }
            let mode = modes
                .get_mut(mode_id)
                .ok_or_else(|| not_found("mode not found"))?;
            validate_manifest(manifest, &mode.soundboards)?;
            mode.manifest = manifest.clone();
        }
        ModeMutation::PutSoundboard {
            mode_id,
            soundboard_id,
            document,
            create_only,
            ..
        } => {
            validate_slug(mode_id)?;
            validate_slug(soundboard_id)?;
            if document.id.as_deref() != Some(soundboard_id) {
                return Err(invalid("soundboard id does not match its filename"));
            }
            let mode = modes
                .get_mut(mode_id)
                .ok_or_else(|| not_found("mode not found"))?;
            let exists = mode.soundboards.contains_key(soundboard_id);
            if *create_only && exists {
                return Err(conflict("soundboard already exists"));
            }
            if !*create_only && !exists {
                return Err(not_found("soundboard not found"));
            }
            mode.soundboards
                .insert(soundboard_id.clone(), document.clone());
        }
        ModeMutation::DeleteSoundboard {
            mode_id,
            soundboard_id,
            ..
        } => {
            validate_slug(mode_id)?;
            validate_slug(soundboard_id)?;
            let mode = modes
                .get_mut(mode_id)
                .ok_or_else(|| not_found("mode not found"))?;
            if mode.manifest.default_soundboard.as_deref() == Some(soundboard_id) {
                return Err(conflict("default soundboard cannot be deleted"));
            }
            if mode.soundboards.remove(soundboard_id).is_none() {
                return Err(not_found("soundboard not found"));
            }
        }
        ModeMutation::PutCue {
            mode_id,
            cue_id,
            document,
            create_only,
            ..
        } => {
            validate_slug(mode_id)?;
            validate_slug(cue_id)?;
            validate_cue(cue_id, document)?;
            let mode = modes
                .get_mut(mode_id)
                .ok_or_else(|| not_found("mode not found"))?;
            let exists = mode.cues.contains_key(cue_id);
            if *create_only && exists {
                return Err(conflict("cue already exists"));
            }
            if !*create_only && !exists {
                return Err(not_found("cue not found"));
            }
            mode.cues.insert(cue_id.clone(), document.clone());
        }
        ModeMutation::DeleteCue {
            mode_id, cue_id, ..
        } => {
            validate_slug(mode_id)?;
            validate_slug(cue_id)?;
            let mode = modes
                .get_mut(mode_id)
                .ok_or_else(|| not_found("mode not found"))?;
            if mode.cues.remove(cue_id).is_none() {
                return Err(not_found("cue not found"));
            }
        }
        ModeMutation::PutPreset {
            mode_id,
            preset_id,
            document,
            create_only,
            ..
        } => {
            validate_slug(mode_id)?;
            validate_slug(preset_id)?;
            validate_preset(preset_id, document)?;
            let mode = modes
                .get_mut(mode_id)
                .ok_or_else(|| not_found("mode not found"))?;
            let exists = mode.presets.contains_key(preset_id);
            if *create_only && exists {
                return Err(conflict("preset already exists"));
            }
            if !*create_only && !exists {
                return Err(not_found("preset not found"));
            }
            mode.presets.insert(preset_id.clone(), document.clone());
        }
        ModeMutation::DeletePreset {
            mode_id, preset_id, ..
        } => {
            validate_slug(mode_id)?;
            validate_slug(preset_id)?;
            let mode = modes
                .get_mut(mode_id)
                .ok_or_else(|| not_found("mode not found"))?;
            if mode.presets.remove(preset_id).is_none() {
                return Err(not_found("preset not found"));
            }
        }
    }
    Ok(modes)
}

fn validate_manifest(
    manifest: &ModeDocument,
    soundboards: &BTreeMap<String, SoundboardDocument>,
) -> Result<(), ModeMutationError> {
    validate_slug(&manifest.id)?;
    if manifest
        .default_soundboard
        .as_ref()
        .is_some_and(|id| !soundboards.contains_key(id))
    {
        return Err(invalid("default soundboard is not present"));
    }
    if manifest.interrupts.iter().any(|interrupt| {
        interrupt
            .duck_to
            .is_some_and(|value| !value.is_finite() || !(0.0..=1.0).contains(&value))
    }) {
        return Err(invalid("interrupt ducking is outside the supported range"));
    }
    Ok(())
}

fn validate_cue(id: &str, document: &CueDocument) -> Result<(), ModeMutationError> {
    if document.id.as_deref() != Some(id) {
        return Err(invalid("cue id does not match its filename"));
    }
    if document
        .sfx
        .iter()
        .any(|sfx| !sfx.volume.is_finite() || !(0.0..=1.0).contains(&sfx.volume))
        || document.loops.iter().any(|loop_spec| {
            !loop_spec.volume.is_finite()
                || !(0.0..=1.0).contains(&loop_spec.volume)
                || !loop_spec.interval_s.is_finite()
                || !(1.0..=3600.0).contains(&loop_spec.interval_s)
        })
    {
        return Err(invalid(
            "cue timing or volume is outside the supported range",
        ));
    }
    Ok(())
}

fn validate_preset(id: &str, document: &PresetDocument) -> Result<(), ModeMutationError> {
    if document.id.as_deref() != Some(id) {
        return Err(invalid("preset id does not match its filename"));
    }
    if document.crossfade_ms.is_some_and(|value| value > 60_000)
        || document.effects.iter().any(|effect| {
            !matches!(
                effect.effect_type.as_str(),
                "eq" | "reverb"
                    | "lowpass"
                    | "highpass"
                    | "bandpass"
                    | "delay"
                    | "distortion"
                    | "tremolo"
            )
        })
    {
        return Err(invalid("preset contains an unsupported effect"));
    }
    Ok(())
}

fn validate_slug(value: &str) -> Result<(), ModeMutationError> {
    let mut characters = value.chars();
    let Some(first) = characters.next() else {
        return Err(invalid("resource id is empty"));
    };
    if value.chars().count() > 64
        || !(first.is_ascii_lowercase() || first.is_ascii_digit())
        || !characters.all(|character| {
            character.is_ascii_lowercase()
                || character.is_ascii_digit()
                || matches!(character, '-' | '_')
        })
    {
        return Err(invalid("resource id is not a lowercase filesystem slug"));
    }
    Ok(())
}

const fn invalid(code: &'static str) -> ModeMutationError {
    ModeMutationError::new(ModeMutationFailureKind::Invalid, code)
}

const fn not_found(code: &'static str) -> ModeMutationError {
    ModeMutationError::new(ModeMutationFailureKind::NotFound, code)
}

const fn conflict(code: &'static str) -> ModeMutationError {
    ModeMutationError::new(ModeMutationFailureKind::Conflict, code)
}

async fn transition_mode_journal(
    repository: &dyn RecoveryJournalRepository,
    current: &RecoveryJournalEntry,
    next: RecoveryState,
    progress: Value,
) -> Result<RecoveryJournalEntry, ModeCoordinatorError> {
    match repository
        .transition_recovery_journal(&current.id, current.state, next, progress)
        .await
        .map_err(|_| {
            ModeCoordinatorError::MutationRecovery("mode mutation journal transition failed")
        })? {
        RecoveryTransition::Applied(entry) => Ok(entry),
        RecoveryTransition::Conflict(_) => Err(ModeCoordinatorError::MutationRecovery(
            "mode mutation journal transition conflicted",
        )),
    }
}

async fn rollback_mode_mutation(
    dependencies: &MutationDependencies,
    applying: &RecoveryJournalEntry,
) -> Result<(), ModeMutationExecutionError> {
    let rolling_back = transition_mode_journal(
        dependencies.journal.as_ref(),
        applying,
        RecoveryState::RollingBack,
        serde_json::json!({"stage": "rolling_back"}),
    )
    .await
    .map_err(ModeMutationExecutionError::Fatal)?;
    if dependencies.effects.rollback(&rolling_back).await.is_err() {
        let _ = transition_mode_journal(
            dependencies.journal.as_ref(),
            &rolling_back,
            RecoveryState::Failed,
            serde_json::json!({"stage": "rollback_failed"}),
        )
        .await;
        return Err(ModeMutationExecutionError::Fatal(
            ModeCoordinatorError::MutationRecovery("mode mutation rollback failed"),
        ));
    }
    transition_mode_journal(
        dependencies.journal.as_ref(),
        &rolling_back,
        RecoveryState::RolledBack,
        serde_json::json!({"stage": "rolled_back"}),
    )
    .await
    .map_err(ModeMutationExecutionError::Fatal)?;
    Ok(())
}

async fn recover_mode_mutations(
    journal: &dyn RecoveryJournalRepository,
    effects: &dyn ModeMutationEffects,
) -> Result<(), ModeCoordinatorError> {
    let unfinished = journal
        .unfinished_recovery_journals(RecoveryDomain::Modes)
        .await
        .map_err(|_| {
            ModeCoordinatorError::MutationRecovery("mode recovery journals could not be loaded")
        })?;
    for entry in unfinished {
        let rolling_back = if entry.state == RecoveryState::RollingBack {
            entry
        } else {
            transition_mode_journal(
                journal,
                &entry,
                RecoveryState::RollingBack,
                serde_json::json!({"stage": "startup_rollback"}),
            )
            .await?
        };
        effects
            .rollback(&rolling_back)
            .await
            .map_err(|_| ModeCoordinatorError::MutationRecovery("mode startup rollback failed"))?;
        transition_mode_journal(
            journal,
            &rolling_back,
            RecoveryState::RolledBack,
            serde_json::json!({"stage": "startup_rolled_back"}),
        )
        .await?;
    }
    effects
        .cleanup_orphans()
        .await
        .map_err(|_| ModeCoordinatorError::MutationRecovery("mode staging cleanup failed"))?;
    Ok(())
}

fn playback_references(
    modes: &BTreeMap<String, ModeBundle>,
) -> BTreeMap<String, ModePlaybackReferences> {
    modes
        .iter()
        .map(|(mode_id, mode)| {
            (
                mode_id.clone(),
                ModePlaybackReferences {
                    soundboard_ids: mode.soundboards.keys().cloned().collect(),
                    preset_ids: mode.presets.keys().cloned().collect(),
                },
            )
        })
        .collect()
}

fn changed_presets(
    current: Option<&ModeCatalog>,
    next: &BTreeMap<String, ModeBundle>,
) -> BTreeSet<ModePresetKey> {
    let Some(current) = current else {
        return BTreeSet::new();
    };
    let mut keys = BTreeSet::new();
    let mode_ids = current
        .modes
        .keys()
        .chain(next.keys())
        .cloned()
        .collect::<BTreeSet<_>>();
    for mode_id in mode_ids {
        let previous = current.modes.get(&mode_id);
        let following = next.get(&mode_id);
        let preset_ids = previous
            .into_iter()
            .flat_map(|mode| mode.presets.keys())
            .chain(following.into_iter().flat_map(|mode| mode.presets.keys()))
            .cloned()
            .collect::<BTreeSet<_>>();
        for preset_id in preset_ids {
            if previous.and_then(|mode| mode.presets.get(&preset_id))
                != following.and_then(|mode| mode.presets.get(&preset_id))
            {
                keys.insert(ModePresetKey {
                    mode_id: mode_id.clone(),
                    preset_id,
                });
            }
        }
    }
    keys
}

fn bounded_errors(errors: BTreeMap<String, String>) -> BTreeMap<String, String> {
    errors
        .into_iter()
        .take(MAX_MODE_ERRORS)
        .map(|(id, detail)| {
            (
                id,
                detail
                    .chars()
                    .take(MAX_MODE_ERROR_CHARS)
                    .collect::<String>(),
            )
        })
        .collect()
}

fn current_unix_seconds() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| duration.as_secs())
}

const fn default_true() -> bool {
    true
}

const fn default_volume() -> f64 {
    1.0
}

#[cfg(test)]
mod tests {
    use std::collections::{BTreeMap, VecDeque};
    use std::error::Error;
    use std::sync::{Arc, Mutex};

    use super::{
        ModeBundle, ModeCatalogPublication, ModeCatalogSink, ModeCatalogSinkError,
        ModeCatalogSinkFuture, ModeCatalogSource, ModeDocument, ModeLoadAttempt, ModeLoadState,
        ModeSourceError, ModeSourceFuture, PresetDocument, start_mode_coordinator,
    };

    #[derive(Debug)]
    struct FakeSource {
        attempts: Mutex<VecDeque<Result<ModeLoadAttempt, ModeSourceError>>>,
    }

    impl ModeCatalogSource for FakeSource {
        fn load<'a>(&'a self) -> ModeSourceFuture<'a> {
            Box::pin(async move {
                self.attempts
                    .lock()
                    .map_err(|_| ModeSourceError::new("source lock failed"))?
                    .pop_front()
                    .ok_or_else(|| ModeSourceError::new("no fixture attempt"))?
            })
        }
    }

    #[derive(Debug, Default)]
    struct FakeSink {
        publications: Mutex<Vec<ModeCatalogPublication>>,
    }

    impl ModeCatalogSink for FakeSink {
        fn publish<'a>(&'a self, publication: ModeCatalogPublication) -> ModeCatalogSinkFuture<'a> {
            Box::pin(async move {
                self.publications
                    .lock()
                    .map_err(|_| ModeCatalogSinkError::new("sink lock failed"))?
                    .push(publication);
                Ok(())
            })
        }
    }

    #[tokio::test]
    async fn failed_reload_keeps_the_last_good_catalog_and_success_tracks_changed_presets()
    -> Result<(), Box<dyn Error>> {
        let first = mode("table", "calm", "First");
        let changed = mode("table", "calm", "Changed");
        let source = Arc::new(FakeSource {
            attempts: Mutex::new(VecDeque::from([
                Ok(ModeLoadAttempt {
                    modes: BTreeMap::from([("table".to_owned(), first)]),
                    errors: BTreeMap::new(),
                }),
                Ok(ModeLoadAttempt {
                    modes: BTreeMap::new(),
                    errors: BTreeMap::from([("broken".to_owned(), "invalid YAML".to_owned())]),
                }),
                Ok(ModeLoadAttempt {
                    modes: BTreeMap::from([("table".to_owned(), changed)]),
                    errors: BTreeMap::new(),
                }),
            ])),
        });
        let sink = Arc::new(FakeSink::default());
        let spawned = start_mode_coordinator(source, sink.clone());
        tokio::time::timeout(
            std::time::Duration::from_secs(1),
            spawned.handle.wait_until_initialized(),
        )
        .await??;

        let initial = spawned.handle.snapshot().ok_or("catalog was not loaded")?;
        assert_eq!(initial.generation, 1);
        let failed = spawned.handle.reload().await?;
        assert!(!failed.published);
        assert_eq!(spawned.handle.snapshot(), Some(initial));
        assert_eq!(spawned.handle.status().state, ModeLoadState::Degraded);

        let reloaded = spawned.handle.reload().await?;
        assert!(reloaded.published);
        assert_eq!(reloaded.generation, 2);
        {
            let publications = sink.publications.lock().map_err(|_| "sink lock failed")?;
            assert_eq!(publications.len(), 2);
            assert!(publications[0].changed_presets.is_empty());
            assert!(
                publications[1]
                    .changed_presets
                    .iter()
                    .any(|key| key.mode_id == "table" && key.preset_id == "calm")
            );
        }
        spawned.handle.shutdown();
        spawned.task.await??;
        Ok(())
    }

    #[tokio::test]
    async fn removed_presets_are_reported_as_changed() -> Result<(), Box<dyn Error>> {
        let source = Arc::new(FakeSource {
            attempts: Mutex::new(VecDeque::from([
                Ok(ModeLoadAttempt {
                    modes: BTreeMap::from([("table".to_owned(), mode("table", "calm", "Calm"))]),
                    errors: BTreeMap::new(),
                }),
                Ok(ModeLoadAttempt {
                    modes: BTreeMap::from([(
                        "table".to_owned(),
                        ModeBundle {
                            presets: BTreeMap::new(),
                            ..mode("table", "unused", "Unused")
                        },
                    )]),
                    errors: BTreeMap::new(),
                }),
            ])),
        });
        let sink = Arc::new(FakeSink::default());
        let spawned = start_mode_coordinator(source, sink.clone());
        spawned.handle.wait_until_initialized().await?;

        spawned.handle.reload().await?;

        {
            let publications = sink.publications.lock().map_err(|_| "sink lock failed")?;
            assert!(
                publications[1]
                    .changed_presets
                    .contains(&super::ModePresetKey {
                        mode_id: "table".to_owned(),
                        preset_id: "calm".to_owned(),
                    })
            );
        }
        spawned.handle.shutdown();
        spawned.task.await??;
        Ok(())
    }

    fn mode(mode_id: &str, preset_id: &str, name: &str) -> ModeBundle {
        ModeBundle {
            manifest: ModeDocument {
                id: mode_id.to_owned(),
                name: mode_id.to_owned(),
                theme: None,
                panels: Vec::new(),
                playlist_categories: Vec::new(),
                interrupts: Vec::new(),
                integrations: Default::default(),
                default_crossfade_ms: 0,
                default_soundboard: None,
                extra: BTreeMap::new(),
            },
            soundboards: BTreeMap::new(),
            cues: BTreeMap::new(),
            presets: BTreeMap::from([(
                preset_id.to_owned(),
                PresetDocument {
                    id: Some(preset_id.to_owned()),
                    name: name.to_owned(),
                    description: None,
                    effects: Vec::new(),
                    crossfade_ms: None,
                    extra: BTreeMap::new(),
                },
            )]),
            theme_css: None,
        }
    }
}
