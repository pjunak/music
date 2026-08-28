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
}

impl Display for ModeCoordinatorError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        match self {
            Self::CommandClosed => formatter.write_str("mode coordinator is unavailable"),
            Self::ReplyDropped => formatter.write_str("mode coordinator dropped its reply"),
            Self::GenerationOverflow => formatter.write_str("mode catalog generation overflowed"),
            Self::CatalogSink(error) => Display::fmt(error, formatter),
        }
    }
}

impl Error for ModeCoordinatorError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::CatalogSink(source) => Some(source),
            Self::CommandClosed | Self::ReplyDropped | Self::GenerationOverflow => None,
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
    ));
    SpawnedModeCoordinator { handle, task }
}

#[derive(Debug)]
enum ModeCommand {
    Reload {
        reply: oneshot::Sender<Result<ModeReloadReport, ModeCoordinatorError>>,
    },
}

async fn run_mode_coordinator(
    source: Arc<dyn ModeCatalogSource>,
    sink: Arc<dyn ModeCatalogSink>,
    mut commands: mpsc::Receiver<ModeCommand>,
    catalog_sender: watch::Sender<Option<Arc<ModeCatalog>>>,
    status_sender: watch::Sender<ModeStatus>,
    cancellation: CancellationToken,
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
