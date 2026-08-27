use std::collections::{BTreeMap, BTreeSet};
use std::error::Error;
use std::fmt::{self, Display, Formatter};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use music_domain::{
    ClockSample, DomainEvent, PersistenceIntent, PlaybackCommand, PlaybackError, PlaybackState,
    ReductionContext, ShuffleMode, TrackId, materialize_positions, reduce,
};
use rand::Rng;
use tokio::sync::{broadcast, mpsc, oneshot, watch};
use tokio::task::JoinHandle;
use tokio::time::MissedTickBehavior;
use tokio_util::sync::CancellationToken;

use super::persistence::{
    PlaybackStateStore, StoreCompareAndSwap, decode_persisted_state, encode_persisted_state,
};

const MAX_CLIENT_ID_LENGTH: usize = 64;
const MAX_CLIENT_NAME_LENGTH: usize = 128;

#[derive(Debug, Clone, Copy, Default, Eq, PartialEq)]
pub struct CatalogGeneration {
    pub library: u64,
    pub modes: u64,
}

#[derive(Debug, Clone, Default, Eq, PartialEq)]
pub struct CatalogMode {
    pub soundboard_ids: BTreeSet<String>,
    pub preset_ids: BTreeSet<String>,
}

#[derive(Debug, Clone, Default, Eq, PartialEq)]
pub struct CatalogSnapshot {
    pub generation: CatalogGeneration,
    /// `None` means the catalog is not loaded yet. `Some(empty)` is a loaded,
    /// valid empty catalog and therefore prunes all track references.
    pub track_ids: Option<BTreeSet<TrackId>>,
    pub modes: Option<BTreeMap<String, CatalogMode>>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ResolvedPlaybackCommand {
    pub command: PlaybackCommand,
    pub catalog_generation: Option<CatalogGeneration>,
}

impl ResolvedPlaybackCommand {
    #[must_use]
    pub const fn direct(command: PlaybackCommand) -> Self {
        Self {
            command,
            catalog_generation: None,
        }
    }

    #[must_use]
    pub const fn at_generation(
        command: PlaybackCommand,
        catalog_generation: CatalogGeneration,
    ) -> Self {
        Self {
            command,
            catalog_generation: Some(catalog_generation),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConnectedClient {
    pub client_id: String,
    pub name: String,
    pub is_default_output: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct PlaybackPublication {
    pub state: Arc<PlaybackState>,
    pub connected_clients: Arc<[ConnectedClient]>,
    pub sampled_at: ClockSample,
}

#[derive(Debug, Clone, Copy, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub struct ConnectionId(u64);

impl ConnectionId {
    #[must_use]
    pub const fn get(self) -> u64 {
        self.0
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClientRegistration {
    pub client_id: String,
    pub name: String,
    pub is_default_output: bool,
}

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub struct PlaybackCommandResult {
    pub changed: bool,
    pub publication_revision: u64,
}

#[derive(Debug, Clone)]
pub struct PlaybackActorConfig {
    pub state_id: i64,
    pub mailbox_capacity: usize,
    pub event_capacity: usize,
    pub position_flush_interval: Duration,
}

impl Default for PlaybackActorConfig {
    fn default() -> Self {
        Self {
            state_id: 1,
            mailbox_capacity: 128,
            event_capacity: 128,
            position_flush_interval: Duration::from_secs(5),
        }
    }
}

pub trait PlaybackClock: Send + Sync + 'static {
    fn sample(&self) -> Result<ClockSample, PlaybackActorError>;
}

#[derive(Debug, Clone)]
pub struct SystemPlaybackClock {
    started: Instant,
    unix_seconds_at_start: f64,
}

impl SystemPlaybackClock {
    pub fn try_new() -> Result<Self, PlaybackActorError> {
        let unix_seconds_at_start = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|_| PlaybackActorError::ClockUnavailable)?
            .as_secs_f64();
        Ok(Self {
            started: Instant::now(),
            unix_seconds_at_start,
        })
    }
}

impl PlaybackClock for SystemPlaybackClock {
    fn sample(&self) -> Result<ClockSample, PlaybackActorError> {
        let elapsed = self.started.elapsed();
        let monotonic_ms = u64::try_from(elapsed.as_millis()).unwrap_or(u64::MAX);
        ClockSample::new(
            monotonic_ms,
            self.unix_seconds_at_start + elapsed.as_secs_f64(),
        )
        .map_err(PlaybackActorError::Domain)
    }
}

pub trait QueueRandom: Send + 'static {
    fn index(&mut self, upper_exclusive: usize) -> usize;
}

#[derive(Debug, Default)]
pub struct SystemQueueRandom;

impl QueueRandom for SystemQueueRandom {
    fn index(&mut self, upper_exclusive: usize) -> usize {
        rand::rng().random_range(0..upper_exclusive)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PlaybackActorError {
    InvalidConfiguration(&'static str),
    InvalidRegistration(&'static str),
    UnknownConnection(ConnectionId),
    MissingCatalogResolution,
    StaleCatalog {
        resolved: CatalogGeneration,
        current: CatalogGeneration,
    },
    InvalidCatalogReference,
    CatalogGenerationRegressed,
    DisconnectedOutput(Vec<String>),
    InactivePositionReporter(String),
    PublicationRevisionOverflow,
    ConnectionIdOverflow,
    Persistence(String),
    PersistenceConflict,
    ClockUnavailable,
    Domain(PlaybackError),
    Stopped,
}

impl Display for PlaybackActorError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidConfiguration(detail) => {
                write!(formatter, "invalid actor configuration: {detail}")
            }
            Self::InvalidRegistration(detail) => {
                write!(formatter, "invalid client registration: {detail}")
            }
            Self::UnknownConnection(connection_id) => {
                write!(formatter, "unknown connection {}", connection_id.get())
            }
            Self::MissingCatalogResolution => {
                formatter.write_str("command requires a catalog generation")
            }
            Self::StaleCatalog { resolved, current } => write!(
                formatter,
                "command resolved at stale catalog generation {resolved:?}; current is {current:?}"
            ),
            Self::InvalidCatalogReference => {
                formatter.write_str("command references a missing catalog resource")
            }
            Self::CatalogGenerationRegressed => {
                formatter.write_str("catalog generation cannot move backwards")
            }
            Self::DisconnectedOutput(device_ids) => {
                write!(formatter, "output device is not connected: {device_ids:?}")
            }
            Self::InactivePositionReporter(device_id) => write!(
                formatter,
                "only an active output may report position: {device_id}"
            ),
            Self::PublicationRevisionOverflow => {
                formatter.write_str("publication revision overflowed")
            }
            Self::ConnectionIdOverflow => formatter.write_str("connection id overflowed"),
            Self::Persistence(detail) => write!(formatter, "playback persistence failed: {detail}"),
            Self::PersistenceConflict => {
                formatter.write_str("playback persistence compare-and-swap conflict")
            }
            Self::ClockUnavailable => formatter.write_str("system clock is unavailable"),
            Self::Domain(error) => Display::fmt(error, formatter),
            Self::Stopped => formatter.write_str("playback actor is stopped"),
        }
    }
}

impl Error for PlaybackActorError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Domain(source) => Some(source),
            _ => None,
        }
    }
}

impl From<PlaybackError> for PlaybackActorError {
    fn from(error: PlaybackError) -> Self {
        Self::Domain(error)
    }
}

#[derive(Clone)]
pub struct PlaybackActorHandle {
    commands: mpsc::Sender<ActorMessage>,
    publications: watch::Receiver<PlaybackPublication>,
    events: broadcast::Sender<DomainEvent>,
    cancellation: CancellationToken,
}

impl fmt::Debug for PlaybackActorHandle {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("PlaybackActorHandle")
            .field("closed", &self.commands.is_closed())
            .finish_non_exhaustive()
    }
}

impl PlaybackActorHandle {
    pub async fn execute(
        &self,
        command: ResolvedPlaybackCommand,
    ) -> Result<PlaybackCommandResult, PlaybackActorError> {
        self.request(|reply| ActorMessage::Execute { command, reply })
            .await
    }

    pub async fn snapshot(&self) -> Result<PlaybackPublication, PlaybackActorError> {
        self.request(|reply| ActorMessage::Snapshot { reply }).await
    }

    pub async fn open_connection(&self) -> Result<ConnectionId, PlaybackActorError> {
        self.request(|reply| ActorMessage::OpenConnection { reply })
            .await
    }

    pub async fn register_connection(
        &self,
        connection_id: ConnectionId,
        registration: ClientRegistration,
    ) -> Result<PlaybackCommandResult, PlaybackActorError> {
        self.request(|reply| ActorMessage::RegisterConnection {
            connection_id,
            registration,
            reply,
        })
        .await
    }

    pub async fn disconnect(
        &self,
        connection_id: ConnectionId,
    ) -> Result<PlaybackCommandResult, PlaybackActorError> {
        self.request(|reply| ActorMessage::Disconnect {
            connection_id,
            reply,
        })
        .await
    }

    pub async fn replace_catalog(
        &self,
        catalog: CatalogSnapshot,
        active_preset_content_changed: bool,
    ) -> Result<PlaybackCommandResult, PlaybackActorError> {
        self.request(|reply| ActorMessage::ReplaceCatalog {
            catalog,
            active_preset_content_changed,
            reply,
        })
        .await
    }

    #[must_use]
    pub fn subscribe_state(&self) -> watch::Receiver<PlaybackPublication> {
        self.publications.clone()
    }

    #[must_use]
    pub fn subscribe_events(&self) -> broadcast::Receiver<DomainEvent> {
        self.events.subscribe()
    }

    pub fn shutdown(&self) {
        self.cancellation.cancel();
    }

    async fn request<T>(
        &self,
        build: impl FnOnce(oneshot::Sender<Result<T, PlaybackActorError>>) -> ActorMessage,
    ) -> Result<T, PlaybackActorError> {
        let (reply, response) = oneshot::channel();
        self.commands
            .send(build(reply))
            .await
            .map_err(|_| PlaybackActorError::Stopped)?;
        response.await.map_err(|_| PlaybackActorError::Stopped)?
    }
}

#[derive(Debug)]
pub struct SpawnedPlaybackActor {
    pub handle: PlaybackActorHandle,
    pub task: JoinHandle<Result<(), PlaybackActorError>>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct ConnectionRecord {
    registration: Option<ClientRegistration>,
}

enum ActorMessage {
    Execute {
        command: ResolvedPlaybackCommand,
        reply: oneshot::Sender<Result<PlaybackCommandResult, PlaybackActorError>>,
    },
    Snapshot {
        reply: oneshot::Sender<Result<PlaybackPublication, PlaybackActorError>>,
    },
    OpenConnection {
        reply: oneshot::Sender<Result<ConnectionId, PlaybackActorError>>,
    },
    RegisterConnection {
        connection_id: ConnectionId,
        registration: ClientRegistration,
        reply: oneshot::Sender<Result<PlaybackCommandResult, PlaybackActorError>>,
    },
    Disconnect {
        connection_id: ConnectionId,
        reply: oneshot::Sender<Result<PlaybackCommandResult, PlaybackActorError>>,
    },
    ReplaceCatalog {
        catalog: CatalogSnapshot,
        active_preset_content_changed: bool,
        reply: oneshot::Sender<Result<PlaybackCommandResult, PlaybackActorError>>,
    },
}

struct PlaybackActor<S, C, R> {
    store: Arc<S>,
    clock: C,
    random: R,
    config: PlaybackActorConfig,
    state: PlaybackState,
    storage_revision: i64,
    dirty_position: bool,
    catalog: CatalogSnapshot,
    connections: BTreeMap<ConnectionId, ConnectionRecord>,
    next_connection_id: u64,
    commands: mpsc::Receiver<ActorMessage>,
    publications: watch::Sender<PlaybackPublication>,
    events: broadcast::Sender<DomainEvent>,
    cancellation: CancellationToken,
}

pub async fn start_playback_actor<S, C, R>(
    store: Arc<S>,
    clock: C,
    random: R,
    config: PlaybackActorConfig,
    catalog: CatalogSnapshot,
) -> Result<SpawnedPlaybackActor, PlaybackActorError>
where
    S: PlaybackStateStore,
    C: PlaybackClock,
    R: QueueRandom,
{
    validate_config(&config)?;
    let sample = clock.sample()?;
    let (mut state, storage_revision) =
        load_or_initialize(store.as_ref(), config.state_id, sample.monotonic_ms).await?;
    let pruned = prune_for_catalog(&mut state, &catalog, sample.monotonic_ms, false)?;
    let storage_revision = if pruned {
        persist_candidate(
            store.as_ref(),
            config.state_id,
            storage_revision,
            &state,
            sample.monotonic_ms,
        )
        .await?
    } else {
        storage_revision
    };

    let initial = publication(&state, &BTreeMap::new(), sample);
    let (publication_tx, publication_rx) = watch::channel(initial);
    let (event_tx, _) = broadcast::channel(config.event_capacity);
    let (command_tx, command_rx) = mpsc::channel(config.mailbox_capacity);
    let cancellation = CancellationToken::new();
    let handle = PlaybackActorHandle {
        commands: command_tx,
        publications: publication_rx,
        events: event_tx.clone(),
        cancellation: cancellation.clone(),
    };
    let actor = PlaybackActor {
        store,
        clock,
        random,
        config,
        state,
        storage_revision,
        dirty_position: false,
        catalog,
        connections: BTreeMap::new(),
        next_connection_id: 1,
        commands: command_rx,
        publications: publication_tx,
        events: event_tx,
        cancellation,
    };
    let task = tokio::spawn(actor.run());
    Ok(SpawnedPlaybackActor { handle, task })
}

impl<S, C, R> PlaybackActor<S, C, R>
where
    S: PlaybackStateStore,
    C: PlaybackClock,
    R: QueueRandom,
{
    async fn run(mut self) -> Result<(), PlaybackActorError> {
        let mut flush = tokio::time::interval(self.config.position_flush_interval);
        flush.set_missed_tick_behavior(MissedTickBehavior::Skip);
        flush.tick().await;

        loop {
            tokio::select! {
                biased;
                _ = self.cancellation.cancelled() => break,
                message = self.commands.recv() => {
                    let Some(message) = message else { break; };
                    self.handle_message(message).await?;
                }
                _ = flush.tick() => self.flush_position().await?,
            }
        }

        self.flush_position().await
    }

    async fn handle_message(&mut self, message: ActorMessage) -> Result<(), PlaybackActorError> {
        match message {
            ActorMessage::Execute { command, reply } => {
                let result = self.execute(command).await;
                let fatal = result.as_ref().err().is_some_and(is_fatal);
                let fatal_error = result.as_ref().err().cloned();
                let _ = reply.send(result);
                if fatal && let Some(error) = fatal_error {
                    return Err(error);
                }
            }
            ActorMessage::Snapshot { reply } => {
                let result = self
                    .clock
                    .sample()
                    .map(|sample| publication(&self.state, &self.connections, sample));
                let _ = reply.send(result);
            }
            ActorMessage::OpenConnection { reply } => {
                let result = self.open_connection();
                let _ = reply.send(result);
            }
            ActorMessage::RegisterConnection {
                connection_id,
                registration,
                reply,
            } => {
                let result = self.register_connection(connection_id, registration).await;
                let fatal = result.as_ref().err().is_some_and(is_fatal);
                let fatal_error = result.as_ref().err().cloned();
                let _ = reply.send(result);
                if fatal && let Some(error) = fatal_error {
                    return Err(error);
                }
            }
            ActorMessage::Disconnect {
                connection_id,
                reply,
            } => {
                let result = self.disconnect(connection_id).await;
                let fatal = result.as_ref().err().is_some_and(is_fatal);
                let fatal_error = result.as_ref().err().cloned();
                let _ = reply.send(result);
                if fatal && let Some(error) = fatal_error {
                    return Err(error);
                }
            }
            ActorMessage::ReplaceCatalog {
                catalog,
                active_preset_content_changed,
                reply,
            } => {
                let result = self
                    .replace_catalog(catalog, active_preset_content_changed)
                    .await;
                let fatal = result.as_ref().err().is_some_and(is_fatal);
                let fatal_error = result.as_ref().err().cloned();
                let _ = reply.send(result);
                if fatal && let Some(error) = fatal_error {
                    return Err(error);
                }
            }
        }
        Ok(())
    }

    async fn execute(
        &mut self,
        resolved: ResolvedPlaybackCommand,
    ) -> Result<PlaybackCommandResult, PlaybackActorError> {
        self.validate_resolved_command(&resolved)?;
        self.validate_live_references(&resolved.command)?;
        let sample = self.clock.sample()?;
        let random_queue_index =
            random_queue_index(&self.state, &resolved.command, &mut self.random);
        let reduction = reduce(
            &self.state,
            resolved.command,
            ReductionContext {
                clock: sample,
                random_queue_index,
            },
        )?;

        match reduction.persistence {
            PersistenceIntent::None => {
                emit_events(&self.events, reduction.events);
                Ok(self.result(reduction.changed))
            }
            PersistenceIntent::Throttled => {
                self.state = reduction.next_state;
                self.dirty_position = true;
                emit_events(&self.events, reduction.events);
                Ok(self.result(reduction.changed))
            }
            PersistenceIntent::Immediate => {
                if !reduction.changed {
                    emit_events(&self.events, reduction.events);
                    return Ok(self.result(false));
                }
                let mut candidate = reduction.next_state;
                if reduction.publish_state {
                    bump_publication_revision(&mut candidate, self.state.publication_revision)?;
                }
                self.storage_revision = persist_candidate(
                    self.store.as_ref(),
                    self.config.state_id,
                    self.storage_revision,
                    &candidate,
                    sample.monotonic_ms,
                )
                .await?;
                self.state = candidate;
                self.dirty_position = false;
                if reduction.publish_state {
                    self.publish(sample);
                }
                emit_events(&self.events, reduction.events);
                Ok(self.result(true))
            }
        }
    }

    fn open_connection(&mut self) -> Result<ConnectionId, PlaybackActorError> {
        let connection_id = ConnectionId(self.next_connection_id);
        self.next_connection_id = self
            .next_connection_id
            .checked_add(1)
            .ok_or(PlaybackActorError::ConnectionIdOverflow)?;
        self.connections
            .insert(connection_id, ConnectionRecord::default());
        Ok(connection_id)
    }

    async fn register_connection(
        &mut self,
        connection_id: ConnectionId,
        registration: ClientRegistration,
    ) -> Result<PlaybackCommandResult, PlaybackActorError> {
        validate_registration(&registration)?;
        let Some(current_record) = self.connections.get(&connection_id) else {
            return Err(PlaybackActorError::UnknownConnection(connection_id));
        };
        if current_record.registration.as_ref() == Some(&registration) {
            return Ok(self.result(false));
        }

        let before_projection = connected_clients(&self.connections);
        let mut candidate_connections = self.connections.clone();
        let previous_registration = candidate_connections
            .get(&connection_id)
            .and_then(|record| record.registration.clone());
        if let Some(record) = candidate_connections.get_mut(&connection_id) {
            record.registration = Some(registration.clone());
        }

        let sample = self.clock.sample()?;
        let mut candidate_state = self.state.clone();
        let mut durable_changed = false;
        if let Some(previous) = previous_registration
            && previous.client_id != registration.client_id
            && !has_registered_sibling(&candidate_connections, connection_id, &previous.client_id)
        {
            let reduction = reduce(
                &candidate_state,
                PlaybackCommand::RemoveActiveOutput(previous.client_id),
                direct_context(sample),
            )?;
            durable_changed |= reduction.changed;
            candidate_state = reduction.next_state;
        }
        let reduction = reduce(
            &candidate_state,
            PlaybackCommand::RegisterDevice {
                device_id: registration.client_id,
                activate: registration.is_default_output,
            },
            direct_context(sample),
        )?;
        durable_changed |= reduction.changed;
        candidate_state = reduction.next_state;
        let presence_changed = before_projection != connected_clients(&candidate_connections);
        self.commit_external_change(
            candidate_state,
            candidate_connections,
            durable_changed,
            presence_changed,
            sample,
        )
        .await
    }

    async fn disconnect(
        &mut self,
        connection_id: ConnectionId,
    ) -> Result<PlaybackCommandResult, PlaybackActorError> {
        let Some(record) = self.connections.get(&connection_id).cloned() else {
            return Err(PlaybackActorError::UnknownConnection(connection_id));
        };
        let before_projection = connected_clients(&self.connections);
        let mut candidate_connections = self.connections.clone();
        candidate_connections.remove(&connection_id);
        let sample = self.clock.sample()?;
        let mut candidate_state = self.state.clone();
        let mut durable_changed = false;
        if let Some(registration) = record.registration
            && !candidate_connections.values().any(|candidate| {
                candidate
                    .registration
                    .as_ref()
                    .is_some_and(|other| other.client_id == registration.client_id)
            })
        {
            let reduction = reduce(
                &candidate_state,
                PlaybackCommand::RemoveActiveOutput(registration.client_id),
                direct_context(sample),
            )?;
            durable_changed = reduction.changed;
            candidate_state = reduction.next_state;
        }
        let presence_changed = before_projection != connected_clients(&candidate_connections);
        self.commit_external_change(
            candidate_state,
            candidate_connections,
            durable_changed,
            presence_changed,
            sample,
        )
        .await
    }

    async fn replace_catalog(
        &mut self,
        catalog: CatalogSnapshot,
        active_preset_content_changed: bool,
    ) -> Result<PlaybackCommandResult, PlaybackActorError> {
        if catalog.generation.library < self.catalog.generation.library
            || catalog.generation.modes < self.catalog.generation.modes
        {
            return Err(PlaybackActorError::CatalogGenerationRegressed);
        }
        if catalog == self.catalog && !active_preset_content_changed {
            return Ok(self.result(false));
        }
        let sample = self.clock.sample()?;
        let mut candidate = self.state.clone();
        let changed = prune_for_catalog(
            &mut candidate,
            &catalog,
            sample.monotonic_ms,
            active_preset_content_changed,
        )?;
        self.catalog = catalog;
        if !changed {
            return Ok(self.result(false));
        }
        bump_publication_revision(&mut candidate, self.state.publication_revision)?;
        self.storage_revision = persist_candidate(
            self.store.as_ref(),
            self.config.state_id,
            self.storage_revision,
            &candidate,
            sample.monotonic_ms,
        )
        .await?;
        self.state = candidate;
        self.dirty_position = false;
        self.publish(sample);
        Ok(self.result(true))
    }

    async fn commit_external_change(
        &mut self,
        mut candidate_state: PlaybackState,
        candidate_connections: BTreeMap<ConnectionId, ConnectionRecord>,
        durable_changed: bool,
        presence_changed: bool,
        sample: ClockSample,
    ) -> Result<PlaybackCommandResult, PlaybackActorError> {
        if !durable_changed && !presence_changed {
            self.connections = candidate_connections;
            return Ok(self.result(false));
        }
        bump_publication_revision(&mut candidate_state, self.state.publication_revision)?;
        if durable_changed || self.dirty_position {
            self.storage_revision = persist_candidate(
                self.store.as_ref(),
                self.config.state_id,
                self.storage_revision,
                &candidate_state,
                sample.monotonic_ms,
            )
            .await?;
            self.dirty_position = false;
        }
        self.state = candidate_state;
        self.connections = candidate_connections;
        self.publish(sample);
        Ok(self.result(true))
    }

    async fn flush_position(&mut self) -> Result<(), PlaybackActorError> {
        if !self.dirty_position {
            return Ok(());
        }
        let sample = self.clock.sample()?;
        self.storage_revision = persist_candidate(
            self.store.as_ref(),
            self.config.state_id,
            self.storage_revision,
            &self.state,
            sample.monotonic_ms,
        )
        .await?;
        self.dirty_position = false;
        Ok(())
    }

    fn publish(&self, sample: ClockSample) {
        self.publications
            .send_replace(publication(&self.state, &self.connections, sample));
    }

    const fn result(&self, changed: bool) -> PlaybackCommandResult {
        PlaybackCommandResult {
            changed,
            publication_revision: self.state.publication_revision,
        }
    }

    fn validate_resolved_command(
        &self,
        resolved: &ResolvedPlaybackCommand,
    ) -> Result<(), PlaybackActorError> {
        if command_requires_catalog(&resolved.command) {
            let generation = resolved
                .catalog_generation
                .ok_or(PlaybackActorError::MissingCatalogResolution)?;
            if generation != self.catalog.generation {
                return Err(PlaybackActorError::StaleCatalog {
                    resolved: generation,
                    current: self.catalog.generation,
                });
            }
        }
        Ok(())
    }

    fn validate_live_references(
        &self,
        command: &PlaybackCommand,
    ) -> Result<(), PlaybackActorError> {
        if let PlaybackCommand::SetActiveOutputs(device_ids) = command {
            let connected = connected_client_ids(&self.connections);
            let already_active = self
                .state
                .active_output_device_ids
                .iter()
                .collect::<BTreeSet<_>>();
            let disconnected = device_ids
                .iter()
                .filter(|device_id| {
                    !already_active.contains(device_id) && !connected.contains(device_id.as_str())
                })
                .cloned()
                .collect::<Vec<_>>();
            if !disconnected.is_empty() {
                return Err(PlaybackActorError::DisconnectedOutput(disconnected));
            }
        }
        if let PlaybackCommand::ReportPosition { device_id, .. } = command
            && !self.state.active_output_device_ids.contains(device_id)
        {
            return Err(PlaybackActorError::InactivePositionReporter(
                device_id.clone(),
            ));
        }
        if !catalog_references_valid(command, &self.catalog, &self.state) {
            return Err(PlaybackActorError::InvalidCatalogReference);
        }
        Ok(())
    }
}

fn validate_config(config: &PlaybackActorConfig) -> Result<(), PlaybackActorError> {
    if config.state_id <= 0 {
        return Err(PlaybackActorError::InvalidConfiguration(
            "state_id must be positive",
        ));
    }
    if config.mailbox_capacity == 0 {
        return Err(PlaybackActorError::InvalidConfiguration(
            "mailbox_capacity must be positive",
        ));
    }
    if config.event_capacity == 0 {
        return Err(PlaybackActorError::InvalidConfiguration(
            "event_capacity must be positive",
        ));
    }
    if config.position_flush_interval.is_zero() {
        return Err(PlaybackActorError::InvalidConfiguration(
            "position_flush_interval must be positive",
        ));
    }
    Ok(())
}

fn validate_registration(registration: &ClientRegistration) -> Result<(), PlaybackActorError> {
    let client_length = registration.client_id.chars().count();
    if !(1..=MAX_CLIENT_ID_LENGTH).contains(&client_length) {
        return Err(PlaybackActorError::InvalidRegistration(
            "client_id must contain 1 to 64 characters",
        ));
    }
    let name_length = registration.name.chars().count();
    if !(1..=MAX_CLIENT_NAME_LENGTH).contains(&name_length) {
        return Err(PlaybackActorError::InvalidRegistration(
            "name must contain 1 to 128 characters",
        ));
    }
    Ok(())
}

async fn load_or_initialize<S: PlaybackStateStore>(
    store: &S,
    state_id: i64,
    now_monotonic_ms: u64,
) -> Result<(PlaybackState, i64), PlaybackActorError> {
    let loaded = store
        .load(state_id)
        .await
        .map_err(|error| PlaybackActorError::Persistence(error.to_string()))?;
    if let Some(snapshot) = loaded {
        let state = decode_persisted_state(&snapshot.state_json)
            .map_err(|error| PlaybackActorError::Persistence(error.to_string()))?;
        let encoded = encode_persisted_state(&state, now_monotonic_ms)
            .map_err(|error| PlaybackActorError::Persistence(error.to_string()))?;
        if json_equivalent(&snapshot.state_json, &encoded) {
            return Ok((state, snapshot.storage_revision));
        }
        let revision =
            compare_and_swap(store, state_id, snapshot.storage_revision, &encoded).await?;
        return Ok((state, revision));
    }

    let state = PlaybackState::default();
    let encoded = encode_persisted_state(&state, now_monotonic_ms)
        .map_err(|error| PlaybackActorError::Persistence(error.to_string()))?;
    let inserted = store
        .insert_if_missing(state_id, &encoded)
        .await
        .map_err(|error| PlaybackActorError::Persistence(error.to_string()))?;
    if inserted {
        Ok((state, 0))
    } else {
        Err(PlaybackActorError::PersistenceConflict)
    }
}

async fn persist_candidate<S: PlaybackStateStore>(
    store: &S,
    state_id: i64,
    storage_revision: i64,
    state: &PlaybackState,
    now_monotonic_ms: u64,
) -> Result<i64, PlaybackActorError> {
    let encoded = encode_persisted_state(state, now_monotonic_ms)
        .map_err(|error| PlaybackActorError::Persistence(error.to_string()))?;
    compare_and_swap(store, state_id, storage_revision, &encoded).await
}

async fn compare_and_swap<S: PlaybackStateStore>(
    store: &S,
    state_id: i64,
    storage_revision: i64,
    encoded: &str,
) -> Result<i64, PlaybackActorError> {
    match store
        .compare_and_swap(state_id, storage_revision, encoded)
        .await
        .map_err(|error| PlaybackActorError::Persistence(error.to_string()))?
    {
        StoreCompareAndSwap::Updated { storage_revision } => Ok(storage_revision),
        StoreCompareAndSwap::Conflict => Err(PlaybackActorError::PersistenceConflict),
    }
}

fn json_equivalent(left: &str, right: &str) -> bool {
    match (
        serde_json::from_str::<serde_json::Value>(left),
        serde_json::from_str::<serde_json::Value>(right),
    ) {
        (Ok(left), Ok(right)) => left == right,
        _ => false,
    }
}

fn bump_publication_revision(
    state: &mut PlaybackState,
    current_revision: u64,
) -> Result<(), PlaybackActorError> {
    state.publication_revision = current_revision
        .checked_add(1)
        .ok_or(PlaybackActorError::PublicationRevisionOverflow)?;
    Ok(())
}

fn publication(
    state: &PlaybackState,
    connections: &BTreeMap<ConnectionId, ConnectionRecord>,
    sample: ClockSample,
) -> PlaybackPublication {
    PlaybackPublication {
        state: Arc::new(materialize_positions(state, sample.monotonic_ms)),
        connected_clients: connected_clients(connections).into(),
        sampled_at: sample,
    }
}

fn connected_clients(
    connections: &BTreeMap<ConnectionId, ConnectionRecord>,
) -> Vec<ConnectedClient> {
    let mut clients = BTreeMap::new();
    for record in connections.values() {
        if let Some(registration) = &record.registration {
            clients.insert(
                registration.client_id.clone(),
                ConnectedClient {
                    client_id: registration.client_id.clone(),
                    name: registration.name.clone(),
                    is_default_output: registration.is_default_output,
                },
            );
        }
    }
    clients.into_values().collect()
}

fn connected_client_ids(connections: &BTreeMap<ConnectionId, ConnectionRecord>) -> BTreeSet<&str> {
    connections
        .values()
        .filter_map(|record| {
            record
                .registration
                .as_ref()
                .map(|registration| registration.client_id.as_str())
        })
        .collect()
}

fn has_registered_sibling(
    connections: &BTreeMap<ConnectionId, ConnectionRecord>,
    connection_id: ConnectionId,
    client_id: &str,
) -> bool {
    connections.iter().any(|(candidate_id, record)| {
        *candidate_id != connection_id
            && record
                .registration
                .as_ref()
                .is_some_and(|registration| registration.client_id == client_id)
    })
}

const fn direct_context(clock: ClockSample) -> ReductionContext {
    ReductionContext {
        clock,
        random_queue_index: None,
    }
}

fn random_queue_index<R: QueueRandom>(
    state: &PlaybackState,
    command: &PlaybackCommand,
    random: &mut R,
) -> Option<usize> {
    let needs_choice = matches!(command, PlaybackCommand::AmbientSkipNext { .. })
        && state.ambient.shuffle == ShuffleMode::Random
        && !state.ambient.queue.is_empty();
    needs_choice.then(|| random.index(state.ambient.queue.len()))
}

fn command_requires_catalog(command: &PlaybackCommand) -> bool {
    matches!(
        command,
        PlaybackCommand::SetActiveMode(_)
            | PlaybackCommand::SetActiveSoundboard(_)
            | PlaybackCommand::SetActivePresets { .. }
            | PlaybackCommand::PresetsChanged { .. }
            | PlaybackCommand::AmbientPlayTrack(_)
            | PlaybackCommand::AmbientSetQueue(_)
            | PlaybackCommand::AmbientEnqueue { .. }
            | PlaybackCommand::AmbientPlaySequence { .. }
            | PlaybackCommand::FireInterruptSequence { .. }
            | PlaybackCommand::FireSfx { .. }
            | PlaybackCommand::StartLoop(_)
    ) || matches!(
        command,
        PlaybackCommand::AmbientSkipNext {
            follow_next_id: Some(_),
            ..
        }
    )
}

fn catalog_references_valid(
    command: &PlaybackCommand,
    catalog: &CatalogSnapshot,
    state: &PlaybackState,
) -> bool {
    let tracks_valid = |track_ids: &[TrackId]| {
        track_ids.is_empty()
            || catalog
                .track_ids
                .as_ref()
                .is_some_and(|known| track_ids.iter().all(|track_id| known.contains(track_id)))
    };
    let mode = state
        .active_mode_id
        .as_ref()
        .and_then(|mode_id| catalog.modes.as_ref()?.get(mode_id));
    match command {
        PlaybackCommand::SetActiveMode(None) | PlaybackCommand::SetActiveSoundboard(None) => true,
        PlaybackCommand::SetActiveMode(Some(mode_id)) => catalog
            .modes
            .as_ref()
            .is_some_and(|modes| modes.contains_key(mode_id)),
        PlaybackCommand::SetActiveSoundboard(Some(soundboard_id))
        | PlaybackCommand::FireSfx { soundboard_id, .. } => {
            mode.is_some_and(|mode| mode.soundboard_ids.contains(soundboard_id))
        }
        PlaybackCommand::SetActivePresets { preset_ids, .. } => {
            preset_ids.is_empty()
                || mode.is_some_and(|mode| preset_ids.iter().all(|id| mode.preset_ids.contains(id)))
        }
        PlaybackCommand::PresetsChanged {
            active_preset_ids: Some(preset_ids),
            ..
        } => {
            preset_ids.is_empty()
                || mode.is_some_and(|mode| preset_ids.iter().all(|id| mode.preset_ids.contains(id)))
        }
        PlaybackCommand::AmbientPlayTrack(track_id)
        | PlaybackCommand::AmbientEnqueue { track_id, .. } => tracks_valid(&[*track_id]),
        PlaybackCommand::AmbientSetQueue(track_ids)
        | PlaybackCommand::AmbientPlaySequence { track_ids, .. }
        | PlaybackCommand::FireInterruptSequence { track_ids, .. } => tracks_valid(track_ids),
        PlaybackCommand::AmbientSkipNext {
            follow_next_id: Some(track_id),
            ..
        } => tracks_valid(&[*track_id]),
        PlaybackCommand::StartLoop(looping_sfx) => {
            mode.is_some_and(|mode| mode.soundboard_ids.contains(&looping_sfx.soundboard_id))
        }
        _ => true,
    }
}

fn prune_for_catalog(
    state: &mut PlaybackState,
    catalog: &CatalogSnapshot,
    now_monotonic_ms: u64,
    active_preset_content_changed: bool,
) -> Result<bool, PlaybackActorError> {
    let mut changed = false;
    if let Some(track_ids) = &catalog.track_ids {
        let previous_queue = state.ambient.queue.len();
        let previous_history = state.ambient.history.len();
        state.ambient.queue.retain(|id| track_ids.contains(id));
        state.ambient.history.retain(|id| track_ids.contains(id));
        changed |= previous_queue != state.ambient.queue.len()
            || previous_history != state.ambient.history.len();
        if state
            .ambient
            .current_track_id
            .is_some_and(|id| !track_ids.contains(&id))
        {
            state.ambient.current_track_id = None;
            state.ambient.position_ms = 0;
            state.ambient.position_anchor_ms = None;
            state.is_playing = false;
            changed = true;
        }
        if let Some(interrupt) = state.interrupt.as_mut() {
            let previous = interrupt.queue.len();
            interrupt.queue.retain(|id| track_ids.contains(id));
            changed |= previous != interrupt.queue.len();
            if !track_ids.contains(&interrupt.current_track_id) {
                state.interrupt = None;
                if state.is_playing && state.ambient.current_track_id.is_some() {
                    state.ambient.position_anchor_ms = Some(now_monotonic_ms);
                }
                changed = true;
            }
        }
    }

    if let Some(modes) = &catalog.modes {
        if state
            .active_mode_id
            .as_ref()
            .is_some_and(|mode_id| !modes.contains_key(mode_id))
        {
            state.active_mode_id = None;
            state.active_soundboard_id = None;
            state.active_preset_ids.clear();
            state.looping_sfx.clear();
            changed = true;
        }
        let active_mode = state
            .active_mode_id
            .as_ref()
            .and_then(|mode_id| modes.get(mode_id));
        if state
            .active_soundboard_id
            .as_ref()
            .is_some_and(|soundboard_id| {
                active_mode.is_none_or(|mode| !mode.soundboard_ids.contains(soundboard_id))
            })
        {
            state.active_soundboard_id = None;
            changed = true;
        }
        let previous_presets = state.active_preset_ids.len();
        state.active_preset_ids.retain(|preset_id| {
            active_mode.is_some_and(|mode| mode.preset_ids.contains(preset_id))
        });
        changed |= previous_presets != state.active_preset_ids.len();
        let previous_loops = state.looping_sfx.len();
        state.looping_sfx.retain(|looping_sfx| {
            active_mode.is_some_and(|mode| mode.soundboard_ids.contains(&looping_sfx.soundboard_id))
        });
        changed |= previous_loops != state.looping_sfx.len();
    }

    if active_preset_content_changed && !state.active_preset_ids.is_empty() {
        state.preset_revision = state
            .preset_revision
            .checked_add(1)
            .ok_or(PlaybackError::PresetRevisionOverflow)?;
        changed = true;
    }
    Ok(changed)
}

fn emit_events(sender: &broadcast::Sender<DomainEvent>, events: Vec<DomainEvent>) {
    for event in events {
        let _ = sender.send(event);
    }
}

fn is_fatal(error: &PlaybackActorError) -> bool {
    matches!(
        error,
        PlaybackActorError::Persistence(_)
            | PlaybackActorError::PersistenceConflict
            | PlaybackActorError::PublicationRevisionOverflow
            | PlaybackActorError::ConnectionIdOverflow
            | PlaybackActorError::ClockUnavailable
    )
}

#[cfg(test)]
mod tests {
    use std::collections::{BTreeMap, BTreeSet};
    use std::error::Error;
    use std::fmt::{self, Display, Formatter};
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::sync::{Arc, Mutex, MutexGuard};
    use std::time::Duration;

    use music_domain::{DomainEvent, PlaybackCommand, TrackId, UnitInterval};
    use serde_json::Value;

    use super::{
        CatalogGeneration, CatalogMode, CatalogSnapshot, ClientRegistration, PlaybackActorConfig,
        PlaybackActorError, PlaybackClock, QueueRandom, ResolvedPlaybackCommand,
        start_playback_actor,
    };
    use crate::playback::{
        PlaybackStateStore, StoreCompareAndSwap, StoreFuture, StoredPlaybackSnapshot,
    };

    #[derive(Debug, Clone, Copy)]
    struct FakeStoreError;

    impl Display for FakeStoreError {
        fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
            formatter.write_str("injected storage error")
        }
    }

    impl Error for FakeStoreError {}

    #[derive(Debug, Default)]
    struct FakeStoreState {
        snapshot: Option<StoredPlaybackSnapshot>,
        fail_compare_and_swap: bool,
        writes: usize,
    }

    #[derive(Debug, Default)]
    struct FakeStore {
        state: Mutex<FakeStoreState>,
    }

    impl FakeStore {
        fn guard(&self) -> MutexGuard<'_, FakeStoreState> {
            self.state
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
        }

        fn set_compare_and_swap_failure(&self, value: bool) {
            self.guard().fail_compare_and_swap = value;
        }

        fn snapshot_json(&self) -> Option<String> {
            self.guard()
                .snapshot
                .as_ref()
                .map(|snapshot| snapshot.state_json.clone())
        }

        fn writes(&self) -> usize {
            self.guard().writes
        }
    }

    impl PlaybackStateStore for FakeStore {
        type Error = FakeStoreError;

        fn load(&self, _id: i64) -> StoreFuture<'_, Option<StoredPlaybackSnapshot>, Self::Error> {
            let snapshot = self.guard().snapshot.clone();
            Box::pin(async move { Ok(snapshot) })
        }

        fn insert_if_missing<'a>(
            &'a self,
            _id: i64,
            state_json: &'a str,
        ) -> StoreFuture<'a, bool, Self::Error> {
            Box::pin(async move {
                let mut state = self.guard();
                if state.snapshot.is_some() {
                    return Ok(false);
                }
                state.snapshot = Some(StoredPlaybackSnapshot {
                    state_json: state_json.to_owned(),
                    storage_revision: 0,
                });
                state.writes += 1;
                Ok(true)
            })
        }

        fn compare_and_swap<'a>(
            &'a self,
            _id: i64,
            expected_storage_revision: i64,
            state_json: &'a str,
        ) -> StoreFuture<'a, StoreCompareAndSwap, Self::Error> {
            Box::pin(async move {
                let mut state = self.guard();
                if state.fail_compare_and_swap {
                    return Err(FakeStoreError);
                }
                let Some(snapshot) = state.snapshot.as_mut() else {
                    return Ok(StoreCompareAndSwap::Conflict);
                };
                if snapshot.storage_revision != expected_storage_revision {
                    return Ok(StoreCompareAndSwap::Conflict);
                }
                let next_revision = expected_storage_revision + 1;
                snapshot.storage_revision = next_revision;
                snapshot.state_json = state_json.to_owned();
                state.writes += 1;
                Ok(StoreCompareAndSwap::Updated {
                    storage_revision: next_revision,
                })
            })
        }
    }

    #[derive(Debug, Clone, Default)]
    struct TestClock {
        monotonic_ms: Arc<AtomicU64>,
    }

    impl TestClock {
        fn set(&self, monotonic_ms: u64) {
            self.monotonic_ms.store(monotonic_ms, Ordering::SeqCst);
        }
    }

    impl PlaybackClock for TestClock {
        fn sample(&self) -> Result<music_domain::ClockSample, PlaybackActorError> {
            let monotonic_ms = self.monotonic_ms.load(Ordering::SeqCst);
            music_domain::ClockSample::new(
                monotonic_ms,
                1_800_000_000.0 + monotonic_ms as f64 / 1_000.0,
            )
            .map_err(PlaybackActorError::Domain)
        }
    }

    #[derive(Debug, Default)]
    struct FirstRandom;

    impl QueueRandom for FirstRandom {
        fn index(&mut self, _upper_exclusive: usize) -> usize {
            0
        }
    }

    fn test_config() -> PlaybackActorConfig {
        PlaybackActorConfig {
            position_flush_interval: Duration::from_secs(3_600),
            ..PlaybackActorConfig::default()
        }
    }

    fn registration(client_id: &str, default_output: bool) -> ClientRegistration {
        ClientRegistration {
            client_id: client_id.to_owned(),
            name: format!("{client_id} player"),
            is_default_output: default_output,
        }
    }

    #[tokio::test]
    async fn persists_before_publishing_a_durable_mutation() -> Result<(), Box<dyn Error>> {
        let store = Arc::new(FakeStore::default());
        let spawned = start_playback_actor(
            Arc::clone(&store),
            TestClock::default(),
            FirstRandom,
            test_config(),
            CatalogSnapshot::default(),
        )
        .await?;
        let mut states = spawned.handle.subscribe_state();

        let result = spawned
            .handle
            .execute(ResolvedPlaybackCommand::direct(
                PlaybackCommand::SetPlaying(true),
            ))
            .await?;

        assert!(result.changed);
        assert_eq!(result.publication_revision, 1);
        states.changed().await?;
        assert!(states.borrow().state.is_playing);
        let stored: Value =
            serde_json::from_str(&store.snapshot_json().ok_or("missing snapshot")?)?;
        assert_eq!(stored["revision"], 1);
        assert_eq!(stored["is_playing"], true);
        spawned.handle.shutdown();
        spawned.task.await??;
        Ok(())
    }

    #[tokio::test]
    async fn persistence_failure_keeps_publication_and_terminates_owner()
    -> Result<(), Box<dyn Error>> {
        let store = Arc::new(FakeStore::default());
        let spawned = start_playback_actor(
            Arc::clone(&store),
            TestClock::default(),
            FirstRandom,
            test_config(),
            CatalogSnapshot::default(),
        )
        .await?;
        let states = spawned.handle.subscribe_state();
        store.set_compare_and_swap_failure(true);

        let result = spawned
            .handle
            .execute(ResolvedPlaybackCommand::direct(
                PlaybackCommand::SetPlaying(true),
            ))
            .await;
        let error = match result {
            Err(error) => error,
            Ok(_) => {
                return Err(
                    std::io::Error::other("injected persistence failure did not surface").into(),
                );
            }
        };

        assert!(matches!(error, PlaybackActorError::Persistence(_)));
        assert!(!states.borrow().state.is_playing);
        assert_eq!(states.borrow().state.publication_revision, 0);
        assert!(matches!(
            spawned.task.await?,
            Err(PlaybackActorError::Persistence(_))
        ));
        Ok(())
    }

    #[tokio::test]
    async fn position_report_is_silent_and_flushes_on_shutdown() -> Result<(), Box<dyn Error>> {
        let store = Arc::new(FakeStore::default());
        let clock = TestClock::default();
        let spawned = start_playback_actor(
            Arc::clone(&store),
            clock.clone(),
            FirstRandom,
            test_config(),
            CatalogSnapshot::default(),
        )
        .await?;
        let connection = spawned.handle.open_connection().await?;
        spawned
            .handle
            .register_connection(connection, registration("output", true))
            .await?;
        let writes_before_report = store.writes();
        clock.set(2_000);

        let result = spawned
            .handle
            .execute(ResolvedPlaybackCommand::direct(
                PlaybackCommand::ReportPosition {
                    device_id: "output".to_owned(),
                    position_ms: 1_750,
                },
            ))
            .await?;

        assert!(result.changed);
        assert_eq!(store.writes(), writes_before_report);
        spawned.handle.shutdown();
        spawned.task.await??;
        assert_eq!(store.writes(), writes_before_report + 1);
        let stored: Value =
            serde_json::from_str(&store.snapshot_json().ok_or("missing snapshot")?)?;
        assert_eq!(stored["last_position_report"]["position_ms"], 1_750);
        Ok(())
    }

    #[tokio::test]
    async fn disconnecting_one_sibling_does_not_deactivate_the_other() -> Result<(), Box<dyn Error>>
    {
        let store = Arc::new(FakeStore::default());
        let spawned = start_playback_actor(
            store,
            TestClock::default(),
            FirstRandom,
            test_config(),
            CatalogSnapshot::default(),
        )
        .await?;
        let first = spawned.handle.open_connection().await?;
        let second = spawned.handle.open_connection().await?;
        spawned
            .handle
            .register_connection(first, registration("shared", true))
            .await?;
        spawned
            .handle
            .register_connection(second, registration("shared", true))
            .await?;

        assert!(!spawned.handle.disconnect(first).await?.changed);
        assert_eq!(
            spawned
                .handle
                .snapshot()
                .await?
                .state
                .active_output_device_ids,
            ["shared"]
        );
        assert!(spawned.handle.disconnect(second).await?.changed);
        assert!(
            spawned
                .handle
                .snapshot()
                .await?
                .state
                .active_output_device_ids
                .is_empty()
        );
        spawned.handle.shutdown();
        spawned.task.await??;
        Ok(())
    }

    #[tokio::test]
    async fn rejects_stale_or_missing_catalog_references_and_emits_transients()
    -> Result<(), Box<dyn Error>> {
        let store = Arc::new(FakeStore::default());
        let generation = CatalogGeneration {
            library: 4,
            modes: 7,
        };
        let catalog = CatalogSnapshot {
            generation,
            track_ids: Some(BTreeSet::from([TrackId::new(1)?])),
            modes: Some(BTreeMap::from([(
                "tabletop".to_owned(),
                CatalogMode {
                    soundboard_ids: BTreeSet::from(["storm".to_owned()]),
                    preset_ids: BTreeSet::new(),
                },
            )])),
        };
        let spawned = start_playback_actor(
            store,
            TestClock::default(),
            FirstRandom,
            test_config(),
            catalog,
        )
        .await?;

        let stale = spawned
            .handle
            .execute(ResolvedPlaybackCommand::at_generation(
                PlaybackCommand::AmbientPlayTrack(TrackId::new(1)?),
                CatalogGeneration::default(),
            ))
            .await;
        assert!(matches!(
            stale,
            Err(PlaybackActorError::StaleCatalog { .. })
        ));
        let missing = spawned
            .handle
            .execute(ResolvedPlaybackCommand::at_generation(
                PlaybackCommand::AmbientPlayTrack(TrackId::new(2)?),
                generation,
            ))
            .await;
        assert!(matches!(
            missing,
            Err(PlaybackActorError::InvalidCatalogReference)
        ));

        spawned
            .handle
            .execute(ResolvedPlaybackCommand::at_generation(
                PlaybackCommand::SetActiveMode(Some("tabletop".to_owned())),
                generation,
            ))
            .await?;
        let mut events = spawned.handle.subscribe_events();
        spawned
            .handle
            .execute(ResolvedPlaybackCommand::at_generation(
                PlaybackCommand::FireSfx {
                    soundboard_id: "storm".to_owned(),
                    item_path: "thunder.mp3".to_owned(),
                    volume: UnitInterval::new(0.75)?,
                },
                generation,
            ))
            .await?;
        assert!(matches!(
            events.recv().await?,
            DomainEvent::SfxFired { soundboard_id, .. } if soundboard_id == "storm"
        ));
        spawned.handle.shutdown();
        spawned.task.await??;
        Ok(())
    }
}
