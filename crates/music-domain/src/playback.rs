use std::collections::BTreeMap;
use std::error::Error;
use std::fmt::{self, Display, Formatter};

const MAX_POSITION_MS: u64 = i64::MAX as u64;
const FLOAT_EPSILON: f64 = 1.0e-9;

#[derive(Debug, Clone, Copy, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub struct TrackId(i64);

impl TrackId {
    pub fn new(value: i64) -> Result<Self, PlaybackError> {
        if value > 0 {
            Ok(Self(value))
        } else {
            Err(PlaybackError::InvalidTrackId(value))
        }
    }

    #[must_use]
    pub const fn get(self) -> i64 {
        self.0
    }
}

#[derive(Debug, Clone, Copy, PartialEq, PartialOrd)]
pub struct UnitInterval(f64);

impl UnitInterval {
    pub fn new(value: f64) -> Result<Self, PlaybackError> {
        if value.is_finite() && (0.0..=1.0).contains(&value) {
            Ok(Self(value))
        } else {
            Err(PlaybackError::InvalidUnitInterval)
        }
    }

    #[must_use]
    pub const fn get(self) -> f64 {
        self.0
    }
}

impl Default for UnitInterval {
    fn default() -> Self {
        Self(1.0)
    }
}

#[derive(Debug, Clone, Copy, Default, Eq, PartialEq)]
pub enum LoopMode {
    #[default]
    Off,
    Follow,
    Queue,
    Track,
}

#[derive(Debug, Clone, Copy, Default, Eq, PartialEq)]
pub enum ShuffleMode {
    #[default]
    Off,
    Random,
}

#[derive(Debug, Clone, Copy, Default, Eq, PartialEq)]
pub enum CrossfadeType {
    #[default]
    Linear,
    EqualPower,
    Cut,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ClockSample {
    pub monotonic_ms: u64,
    pub unix_seconds: f64,
}

impl ClockSample {
    pub fn new(monotonic_ms: u64, unix_seconds: f64) -> Result<Self, PlaybackError> {
        if unix_seconds.is_finite() && unix_seconds >= 0.0 {
            Ok(Self {
                monotonic_ms,
                unix_seconds,
            })
        } else {
            Err(PlaybackError::InvalidClock)
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AmbientState {
    pub current_track_id: Option<TrackId>,
    pub queue: Vec<TrackId>,
    pub history: Vec<TrackId>,
    pub position_ms: u64,
    pub position_anchor_ms: Option<u64>,
    pub loop_mode: LoopMode,
    pub shuffle: ShuffleMode,
    pub source_playlist_id: Option<i64>,
}

impl Default for AmbientState {
    fn default() -> Self {
        Self {
            current_track_id: None,
            queue: Vec::new(),
            history: Vec::new(),
            position_ms: 0,
            position_anchor_ms: None,
            loop_mode: LoopMode::Off,
            shuffle: ShuffleMode::Off,
            source_playlist_id: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct InterruptState {
    pub current_track_id: TrackId,
    pub queue: Vec<TrackId>,
    pub position_ms: u64,
    pub position_anchor_ms: Option<u64>,
    pub return_to_ambient: bool,
    pub fade_in_ms: u32,
    pub fade_out_ms: u32,
    pub duck_to: Option<UnitInterval>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct LoopingSfx {
    pub id: String,
    pub name: String,
    pub soundboard_id: String,
    pub item_path: String,
    pub interval_seconds: f64,
    pub volume: UnitInterval,
}

impl LoopingSfx {
    pub fn new(
        id: String,
        name: String,
        soundboard_id: String,
        item_path: String,
        interval_seconds: f64,
        volume: UnitInterval,
    ) -> Result<Self, PlaybackError> {
        if id.is_empty()
            || name.is_empty()
            || soundboard_id.is_empty()
            || item_path.is_empty()
            || !interval_seconds.is_finite()
            || !(1.0..=3600.0).contains(&interval_seconds)
        {
            return Err(PlaybackError::InvalidLoopingSfx);
        }
        Ok(Self {
            id,
            name,
            soundboard_id,
            item_path,
            interval_seconds,
            volume,
        })
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct PositionReport {
    pub device_id: String,
    pub position_ms: u64,
    pub reported_at_unix_seconds: f64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct PlaybackState {
    pub publication_revision: u64,
    pub position_epoch: u64,
    pub is_playing: bool,
    pub active_mode_id: Option<String>,
    pub active_output_device_ids: Vec<String>,
    pub default_device_volume: UnitInterval,
    pub device_volumes: BTreeMap<String, UnitInterval>,
    pub active_soundboard_id: Option<String>,
    pub active_preset_ids: Vec<String>,
    pub preset_revision: u64,
    pub crossfade_ms: u32,
    pub crossfade_type: CrossfadeType,
    pub ambient: AmbientState,
    pub interrupt: Option<InterruptState>,
    pub looping_sfx: Vec<LoopingSfx>,
    pub last_position_report: Option<PositionReport>,
}

impl Default for PlaybackState {
    fn default() -> Self {
        Self {
            publication_revision: 0,
            position_epoch: 0,
            is_playing: false,
            active_mode_id: None,
            active_output_device_ids: Vec::new(),
            default_device_volume: UnitInterval::default(),
            device_volumes: BTreeMap::new(),
            active_soundboard_id: None,
            active_preset_ids: Vec::new(),
            preset_revision: 0,
            crossfade_ms: 0,
            crossfade_type: CrossfadeType::Linear,
            ambient: AmbientState::default(),
            interrupt: None,
            looping_sfx: Vec::new(),
            last_position_report: None,
        }
    }
}

impl PlaybackState {
    /// Remove session-only state and prevent playback from advancing through
    /// process downtime. Catalog pruning is performed by the application
    /// layer because the pure domain does not know which resources exist.
    pub fn normalize_for_startup(&mut self) {
        self.is_playing = false;
        self.active_output_device_ids.clear();
        self.looping_sfx.clear();
        self.interrupt = None;
        self.ambient.position_anchor_ms = None;
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum PlaybackCommand {
    SetGroupVolume(UnitInterval),
    SetPlaying(bool),
    SetActiveMode(Option<String>),
    SetActiveOutputs(Vec<String>),
    RegisterDevice {
        device_id: String,
        activate: bool,
    },
    RemoveActiveOutput(String),
    SetDeviceVolume {
        device_id: String,
        volume: UnitInterval,
    },
    SetActiveSoundboard(Option<String>),
    SetActivePresets {
        preset_ids: Vec<String>,
        crossfade_ms: Option<u32>,
    },
    PresetsChanged {
        active_preset_ids: Option<Vec<String>>,
        crossfade_ms: Option<u32>,
    },
    SetCrossfade {
        crossfade_ms: u32,
        crossfade_type: Option<CrossfadeType>,
    },
    ReportPosition {
        device_id: String,
        position_ms: u64,
    },
    AmbientPlayTrack(TrackId),
    AmbientSetQueue(Vec<TrackId>),
    AmbientJumpQueue(usize),
    AmbientEnqueue {
        track_id: TrackId,
        position: Option<usize>,
    },
    AmbientClearQueue,
    AmbientSkipNext {
        follow_next_id: Option<TrackId>,
        expected_track_id: Option<TrackId>,
    },
    AmbientSkipPrevious,
    AmbientSeek(u64),
    AmbientSetLoop(LoopMode),
    AmbientSetShuffle(ShuffleMode),
    AmbientStop,
    AmbientPlaySequence {
        track_ids: Vec<TrackId>,
        start_index: usize,
        source_playlist_id: Option<i64>,
    },
    FireInterruptSequence {
        track_ids: Vec<TrackId>,
        return_to_ambient: bool,
        fade_in_ms: u32,
        fade_out_ms: u32,
        duck_to: Option<UnitInterval>,
    },
    InterruptSkipNext {
        expected_track_id: Option<TrackId>,
    },
    InterruptSeek(u64),
    CancelInterrupt,
    FireSfx {
        soundboard_id: String,
        item_path: String,
        volume: UnitInterval,
    },
    StartLoop(LoopingSfx),
    StopLoop(String),
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ReductionContext {
    pub clock: ClockSample,
    /// Required only when a random-shuffle queue advance needs a choice.
    pub random_queue_index: Option<usize>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum DomainEvent {
    SfxFired {
        soundboard_id: String,
        item_path: String,
        volume: UnitInterval,
    },
    LoopStarted(LoopingSfx),
    LoopStopped {
        id: String,
    },
}

#[derive(Debug, Clone, Copy, Default, Eq, PartialEq)]
pub enum PersistenceIntent {
    #[default]
    None,
    Throttled,
    Immediate,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Reduction {
    pub next_state: PlaybackState,
    pub changed: bool,
    pub publish_state: bool,
    pub persistence: PersistenceIntent,
    pub events: Vec<DomainEvent>,
}

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum PlaybackError {
    InvalidTrackId(i64),
    InvalidUnitInterval,
    InvalidClock,
    InvalidLoopingSfx,
    RandomChoiceRequired,
    RandomChoiceOutOfRange,
    PositionEpochOverflow,
    PresetRevisionOverflow,
}

impl Display for PlaybackError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::InvalidTrackId(_) => "track id must be positive",
            Self::InvalidUnitInterval => "value must be finite and between zero and one",
            Self::InvalidClock => "clock sample must contain a finite non-negative wall time",
            Self::InvalidLoopingSfx => "looping sound effect is outside its domain bounds",
            Self::RandomChoiceRequired => "shuffle advance requires an explicit random choice",
            Self::RandomChoiceOutOfRange => "shuffle choice is outside the current queue",
            Self::PositionEpochOverflow => "playback position epoch overflowed",
            Self::PresetRevisionOverflow => "preset revision overflowed",
        })
    }
}

impl Error for PlaybackError {}

pub fn reduce(
    current: &PlaybackState,
    command: PlaybackCommand,
    context: ReductionContext,
) -> Result<Reduction, PlaybackError> {
    let mut next = current.clone();
    let mut events = Vec::new();
    let mut publish_state = true;
    let mut persistence = PersistenceIntent::Immediate;
    let changed = match command {
        PlaybackCommand::SetGroupVolume(volume) => set_group_volume(&mut next, volume),
        PlaybackCommand::SetPlaying(playing) => {
            set_playing(&mut next, playing, context.clock.monotonic_ms)
        }
        PlaybackCommand::SetActiveMode(mode_id) => set_active_mode(&mut next, mode_id),
        PlaybackCommand::SetActiveOutputs(device_ids) => {
            replace_if_different(&mut next.active_output_device_ids, device_ids)
        }
        PlaybackCommand::RegisterDevice {
            device_id,
            activate,
        } => register_device(&mut next, device_id, activate),
        PlaybackCommand::RemoveActiveOutput(device_id) => {
            remove_active_output(&mut next, &device_id)
        }
        PlaybackCommand::SetDeviceVolume { device_id, volume } => {
            set_device_volume(&mut next, device_id, volume)
        }
        PlaybackCommand::SetActiveSoundboard(soundboard_id) => {
            if next.active_soundboard_id == soundboard_id {
                false
            } else {
                next.active_soundboard_id = soundboard_id;
                true
            }
        }
        PlaybackCommand::SetActivePresets {
            preset_ids,
            crossfade_ms,
        } => set_active_presets(&mut next, preset_ids, crossfade_ms),
        PlaybackCommand::PresetsChanged {
            active_preset_ids,
            crossfade_ms,
        } => {
            next.preset_revision = next
                .preset_revision
                .checked_add(1)
                .ok_or(PlaybackError::PresetRevisionOverflow)?;
            if let Some(preset_ids) = active_preset_ids {
                next.active_preset_ids = preset_ids;
            }
            if let Some(milliseconds) = crossfade_ms {
                next.crossfade_ms = milliseconds;
            }
            true
        }
        PlaybackCommand::SetCrossfade {
            crossfade_ms,
            crossfade_type,
        } => set_crossfade(&mut next, crossfade_ms, crossfade_type),
        PlaybackCommand::ReportPosition {
            device_id,
            position_ms,
        } => {
            report_position(
                &mut next,
                device_id,
                position_ms.min(MAX_POSITION_MS),
                context.clock,
            );
            publish_state = false;
            persistence = PersistenceIntent::Throttled;
            true
        }
        PlaybackCommand::AmbientPlayTrack(track_id) => {
            ambient_play_track(&mut next, track_id, context.clock.monotonic_ms)?;
            true
        }
        PlaybackCommand::AmbientSetQueue(track_ids) => ambient_set_queue(&mut next, track_ids),
        PlaybackCommand::AmbientJumpQueue(position) => {
            ambient_jump_queue(&mut next, position, context.clock.monotonic_ms)?
        }
        PlaybackCommand::AmbientEnqueue { track_id, position } => {
            ambient_enqueue(&mut next, track_id, position);
            true
        }
        PlaybackCommand::AmbientClearQueue => {
            if next.ambient.queue.is_empty() {
                false
            } else {
                next.ambient.queue.clear();
                true
            }
        }
        PlaybackCommand::AmbientSkipNext {
            follow_next_id,
            expected_track_id,
        } => ambient_skip_next(&mut next, follow_next_id, expected_track_id, context)?,
        PlaybackCommand::AmbientSkipPrevious => {
            ambient_skip_previous(&mut next, context.clock.monotonic_ms)?
        }
        PlaybackCommand::AmbientSeek(position_ms) => {
            ambient_seek(&mut next, position_ms, context.clock.monotonic_ms)?
        }
        PlaybackCommand::AmbientSetLoop(loop_mode) => {
            if next.ambient.loop_mode == loop_mode {
                false
            } else {
                next.ambient.loop_mode = loop_mode;
                true
            }
        }
        PlaybackCommand::AmbientSetShuffle(shuffle) => {
            if next.ambient.shuffle == shuffle {
                false
            } else {
                next.ambient.shuffle = shuffle;
                true
            }
        }
        PlaybackCommand::AmbientStop => ambient_stop(&mut next)?,
        PlaybackCommand::AmbientPlaySequence {
            track_ids,
            start_index,
            source_playlist_id,
        } => ambient_play_sequence(
            &mut next,
            track_ids,
            start_index,
            source_playlist_id,
            context.clock.monotonic_ms,
        )?,
        PlaybackCommand::FireInterruptSequence {
            track_ids,
            return_to_ambient,
            fade_in_ms,
            fade_out_ms,
            duck_to,
        } => fire_interrupt(
            &mut next,
            track_ids,
            return_to_ambient,
            fade_in_ms,
            fade_out_ms,
            duck_to,
            context.clock.monotonic_ms,
        )?,
        PlaybackCommand::InterruptSkipNext { expected_track_id } => {
            interrupt_skip_next(&mut next, expected_track_id, context.clock.monotonic_ms)?
        }
        PlaybackCommand::InterruptSeek(position_ms) => {
            interrupt_seek(&mut next, position_ms, context.clock.monotonic_ms)?
        }
        PlaybackCommand::CancelInterrupt => end_interrupt(&mut next, context.clock.monotonic_ms)?,
        PlaybackCommand::FireSfx {
            soundboard_id,
            item_path,
            volume,
        } => {
            events.push(DomainEvent::SfxFired {
                soundboard_id,
                item_path,
                volume,
            });
            publish_state = false;
            persistence = PersistenceIntent::None;
            false
        }
        PlaybackCommand::StartLoop(looping_sfx) => {
            next.looping_sfx
                .retain(|existing| existing.id != looping_sfx.id);
            next.looping_sfx.push(looping_sfx.clone());
            events.push(DomainEvent::LoopStarted(looping_sfx));
            true
        }
        PlaybackCommand::StopLoop(id) => {
            let previous_length = next.looping_sfx.len();
            next.looping_sfx.retain(|looping_sfx| looping_sfx.id != id);
            if next.looping_sfx.len() == previous_length {
                false
            } else {
                events.push(DomainEvent::LoopStopped { id });
                true
            }
        }
    };

    if !changed && events.is_empty() {
        publish_state = false;
        persistence = PersistenceIntent::None;
    }
    Ok(Reduction {
        next_state: next,
        changed,
        publish_state,
        persistence,
        events,
    })
}

#[must_use]
pub fn materialize_positions(state: &PlaybackState, now_monotonic_ms: u64) -> PlaybackState {
    let mut materialized = state.clone();
    materialize_ambient(&mut materialized.ambient, now_monotonic_ms);
    if let Some(interrupt) = materialized.interrupt.as_mut() {
        materialize_interrupt(interrupt, now_monotonic_ms);
    }
    materialized
}

fn materialize_ambient(ambient: &mut AmbientState, now_ms: u64) {
    if let Some(anchor) = ambient.position_anchor_ms {
        let elapsed = now_ms.saturating_sub(anchor);
        if elapsed > 0 {
            ambient.position_ms = bounded_position_add(ambient.position_ms, elapsed);
            ambient.position_anchor_ms = Some(now_ms);
        }
    }
}

fn materialize_interrupt(interrupt: &mut InterruptState, now_ms: u64) {
    if let Some(anchor) = interrupt.position_anchor_ms {
        let elapsed = now_ms.saturating_sub(anchor);
        if elapsed > 0 {
            interrupt.position_ms = bounded_position_add(interrupt.position_ms, elapsed);
            interrupt.position_anchor_ms = Some(now_ms);
        }
    }
}

fn freeze_ambient(ambient: &mut AmbientState, now_ms: u64) {
    materialize_ambient(ambient, now_ms);
    ambient.position_anchor_ms = None;
}

fn ambient_anchor(state: &PlaybackState, now_ms: u64) -> Option<u64> {
    (state.is_playing
        && state
            .interrupt
            .as_ref()
            .is_none_or(|interrupt| interrupt.duck_to.is_some()))
    .then_some(now_ms)
}

fn increment_position_epoch(state: &mut PlaybackState) -> Result<(), PlaybackError> {
    state.position_epoch = state
        .position_epoch
        .checked_add(1)
        .ok_or(PlaybackError::PositionEpochOverflow)?;
    Ok(())
}

fn set_group_volume(state: &mut PlaybackState, target: UnitInterval) -> bool {
    let previous = state
        .device_volumes
        .values()
        .fold(state.default_device_volume.get(), |largest, volume| {
            largest.max(volume.get())
        });
    if (previous - target.get()).abs() < FLOAT_EPSILON {
        return false;
    }
    state.device_volumes = if previous > FLOAT_EPSILON {
        state
            .device_volumes
            .iter()
            .map(|(device_id, level)| {
                let scaled = (level.get() * target.get() / previous).clamp(0.0, 1.0);
                (device_id.clone(), UnitInterval(scaled))
            })
            .collect()
    } else {
        state
            .device_volumes
            .keys()
            .map(|device_id| (device_id.clone(), target))
            .collect()
    };
    state.default_device_volume = target;
    true
}

fn set_playing(state: &mut PlaybackState, playing: bool, now_ms: u64) -> bool {
    if state.is_playing == playing {
        return false;
    }
    if playing {
        state.is_playing = true;
        if state.ambient.current_track_id.is_some() {
            state.ambient.position_anchor_ms = ambient_anchor(state, now_ms);
        }
    } else {
        freeze_ambient(&mut state.ambient, now_ms);
        state.is_playing = false;
    }
    true
}

fn set_active_mode(state: &mut PlaybackState, mode_id: Option<String>) -> bool {
    if state.active_mode_id == mode_id {
        return false;
    }
    state.active_mode_id = mode_id;
    state.active_preset_ids.clear();
    state.active_soundboard_id = None;
    true
}

fn register_device(state: &mut PlaybackState, device_id: String, activate: bool) -> bool {
    let mut changed = false;
    if !state.device_volumes.contains_key(&device_id) {
        state
            .device_volumes
            .insert(device_id.clone(), state.default_device_volume);
        changed = true;
    }
    if activate && !state.active_output_device_ids.contains(&device_id) {
        state.active_output_device_ids.push(device_id);
        changed = true;
    }
    changed
}

fn remove_active_output(state: &mut PlaybackState, device_id: &str) -> bool {
    let previous_length = state.active_output_device_ids.len();
    state
        .active_output_device_ids
        .retain(|active| active != device_id);
    state.active_output_device_ids.len() != previous_length
}

fn set_device_volume(state: &mut PlaybackState, device_id: String, volume: UnitInterval) -> bool {
    if state.device_volumes.get(&device_id) == Some(&volume) {
        return false;
    }
    state.device_volumes.insert(device_id, volume);
    true
}

fn set_active_presets(
    state: &mut PlaybackState,
    preset_ids: Vec<String>,
    crossfade_ms: Option<u32>,
) -> bool {
    let mut deduplicated = Vec::new();
    for preset_id in preset_ids {
        if !deduplicated.contains(&preset_id) {
            deduplicated.push(preset_id);
        }
    }
    let mut changed = replace_if_different(&mut state.active_preset_ids, deduplicated);
    if let Some(milliseconds) = crossfade_ms
        && state.crossfade_ms != milliseconds
    {
        state.crossfade_ms = milliseconds;
        changed = true;
    }
    changed
}

fn set_crossfade(
    state: &mut PlaybackState,
    crossfade_ms: u32,
    crossfade_type: Option<CrossfadeType>,
) -> bool {
    let next_type = crossfade_type.unwrap_or(state.crossfade_type);
    if state.crossfade_ms == crossfade_ms && state.crossfade_type == next_type {
        return false;
    }
    state.crossfade_ms = crossfade_ms;
    state.crossfade_type = next_type;
    true
}

fn report_position(
    state: &mut PlaybackState,
    device_id: String,
    position_ms: u64,
    clock: ClockSample,
) {
    state.last_position_report = Some(PositionReport {
        device_id,
        position_ms,
        reported_at_unix_seconds: clock.unix_seconds,
    });
    if let Some(interrupt) = state.interrupt.as_mut() {
        interrupt.position_ms = position_ms;
        if interrupt.position_anchor_ms.is_some() {
            interrupt.position_anchor_ms = Some(clock.monotonic_ms);
        }
    } else {
        state.ambient.position_ms = position_ms;
        if state.ambient.position_anchor_ms.is_some() {
            state.ambient.position_anchor_ms = Some(clock.monotonic_ms);
        }
    }
}

fn ambient_play_track(
    state: &mut PlaybackState,
    track_id: TrackId,
    now_ms: u64,
) -> Result<(), PlaybackError> {
    let loop_mode = state.ambient.loop_mode;
    let shuffle = state.ambient.shuffle;
    state.is_playing = true;
    state.ambient = AmbientState {
        current_track_id: Some(track_id),
        position_anchor_ms: ambient_anchor(state, now_ms),
        loop_mode,
        shuffle,
        ..AmbientState::default()
    };
    increment_position_epoch(state)
}

fn ambient_set_queue(state: &mut PlaybackState, track_ids: Vec<TrackId>) -> bool {
    if state.ambient.queue == track_ids {
        return false;
    }
    state.ambient.queue = track_ids;
    state.ambient.source_playlist_id = None;
    true
}

fn ambient_jump_queue(
    state: &mut PlaybackState,
    position: usize,
    now_ms: u64,
) -> Result<bool, PlaybackError> {
    if position >= state.ambient.queue.len() {
        return Ok(false);
    }
    if let Some(current) = state.ambient.current_track_id {
        state.ambient.history.push(current);
    }
    state.ambient.current_track_id = Some(state.ambient.queue[position]);
    state.ambient.queue = state.ambient.queue[(position + 1)..].to_vec();
    state.ambient.position_ms = 0;
    state.is_playing = true;
    state.ambient.position_anchor_ms = ambient_anchor(state, now_ms);
    increment_position_epoch(state)?;
    Ok(true)
}

fn ambient_enqueue(state: &mut PlaybackState, track_id: TrackId, position: Option<usize>) {
    let index = position
        .unwrap_or(state.ambient.queue.len())
        .min(state.ambient.queue.len());
    state.ambient.queue.insert(index, track_id);
}

fn ambient_skip_next(
    state: &mut PlaybackState,
    follow_next_id: Option<TrackId>,
    expected_track_id: Option<TrackId>,
    context: ReductionContext,
) -> Result<bool, PlaybackError> {
    if expected_track_id.is_some() && state.ambient.current_track_id != expected_track_id {
        return Ok(false);
    }
    if state.ambient.current_track_id.is_none() && state.ambient.queue.is_empty() {
        return Ok(false);
    }
    let anchor = ambient_anchor(state, context.clock.monotonic_ms);
    increment_position_epoch(state)?;

    if state.ambient.loop_mode == LoopMode::Track && state.ambient.current_track_id.is_some() {
        state.ambient.position_ms = 0;
        state.ambient.position_anchor_ms = anchor;
        return Ok(true);
    }
    if !state.ambient.queue.is_empty() {
        let index = if state.ambient.shuffle == ShuffleMode::Random {
            let choice = context
                .random_queue_index
                .ok_or(PlaybackError::RandomChoiceRequired)?;
            if choice >= state.ambient.queue.len() {
                return Err(PlaybackError::RandomChoiceOutOfRange);
            }
            choice
        } else {
            0
        };
        if let Some(current) = state.ambient.current_track_id {
            state.ambient.history.push(current);
        }
        state.ambient.current_track_id = Some(state.ambient.queue.remove(index));
        state.ambient.position_ms = 0;
        state.ambient.position_anchor_ms = anchor;
        return Ok(true);
    }
    if state.ambient.loop_mode == LoopMode::Queue
        && (!state.ambient.history.is_empty() || state.ambient.current_track_id.is_some())
    {
        let mut sequence = std::mem::take(&mut state.ambient.history);
        if let Some(current) = state.ambient.current_track_id {
            sequence.push(current);
        }
        state.ambient.current_track_id = sequence.first().copied();
        state.ambient.queue = sequence.into_iter().skip(1).collect();
        state.ambient.position_ms = 0;
        state.ambient.position_anchor_ms = anchor;
        return Ok(true);
    }
    if state.ambient.loop_mode == LoopMode::Follow
        && let Some(follow) = follow_next_id
    {
        if let Some(current) = state.ambient.current_track_id {
            state.ambient.history.push(current);
        }
        state.ambient.current_track_id = Some(follow);
        state.ambient.queue.clear();
        state.ambient.position_ms = 0;
        state.ambient.position_anchor_ms = anchor;
        return Ok(true);
    }

    state.ambient.current_track_id = None;
    state.ambient.position_ms = 0;
    state.ambient.position_anchor_ms = None;
    state.is_playing = false;
    Ok(true)
}

fn ambient_skip_previous(state: &mut PlaybackState, now_ms: u64) -> Result<bool, PlaybackError> {
    if state.ambient.history.is_empty() && state.ambient.current_track_id.is_none() {
        return Ok(false);
    }
    let anchor = ambient_anchor(state, now_ms);
    if let Some(previous) = state.ambient.history.pop() {
        if let Some(current) = state.ambient.current_track_id {
            state.ambient.queue.insert(0, current);
        }
        state.ambient.current_track_id = Some(previous);
    }
    state.ambient.position_ms = 0;
    state.ambient.position_anchor_ms = anchor;
    increment_position_epoch(state)?;
    Ok(true)
}

fn ambient_seek(
    state: &mut PlaybackState,
    position_ms: u64,
    now_ms: u64,
) -> Result<bool, PlaybackError> {
    if state.ambient.current_track_id.is_none() {
        return Ok(false);
    }
    state.ambient.position_ms = position_ms.min(MAX_POSITION_MS);
    state.ambient.position_anchor_ms = ambient_anchor(state, now_ms);
    increment_position_epoch(state)?;
    Ok(true)
}

fn ambient_stop(state: &mut PlaybackState) -> Result<bool, PlaybackError> {
    if state.ambient.current_track_id.is_none()
        && state.ambient.queue.is_empty()
        && state.ambient.history.is_empty()
        && state.ambient.position_ms == 0
    {
        return Ok(false);
    }
    let loop_mode = state.ambient.loop_mode;
    let shuffle = state.ambient.shuffle;
    state.ambient = AmbientState {
        loop_mode,
        shuffle,
        ..AmbientState::default()
    };
    increment_position_epoch(state)?;
    Ok(true)
}

fn ambient_play_sequence(
    state: &mut PlaybackState,
    track_ids: Vec<TrackId>,
    start_index: usize,
    source_playlist_id: Option<i64>,
    now_ms: u64,
) -> Result<bool, PlaybackError> {
    if track_ids.is_empty() {
        return Ok(false);
    }
    let index = start_index.min(track_ids.len() - 1);
    let loop_mode = state.ambient.loop_mode;
    let shuffle = state.ambient.shuffle;
    state.is_playing = true;
    state.ambient = AmbientState {
        current_track_id: Some(track_ids[index]),
        queue: track_ids[(index + 1)..].to_vec(),
        history: track_ids[..index].to_vec(),
        position_ms: 0,
        position_anchor_ms: ambient_anchor(state, now_ms),
        loop_mode,
        shuffle,
        source_playlist_id,
    };
    increment_position_epoch(state)?;
    Ok(true)
}

#[allow(clippy::too_many_arguments)]
fn fire_interrupt(
    state: &mut PlaybackState,
    track_ids: Vec<TrackId>,
    return_to_ambient: bool,
    fade_in_ms: u32,
    fade_out_ms: u32,
    duck_to: Option<UnitInterval>,
    now_ms: u64,
) -> Result<bool, PlaybackError> {
    let Some((&current, queue)) = track_ids.split_first() else {
        return Ok(false);
    };
    if duck_to.is_none() {
        freeze_ambient(&mut state.ambient, now_ms);
    }
    state.interrupt = Some(InterruptState {
        current_track_id: current,
        queue: queue.to_vec(),
        position_ms: 0,
        position_anchor_ms: Some(now_ms),
        return_to_ambient,
        fade_in_ms,
        fade_out_ms,
        duck_to,
    });
    state.is_playing = true;
    increment_position_epoch(state)?;
    Ok(true)
}

fn interrupt_skip_next(
    state: &mut PlaybackState,
    expected_track_id: Option<TrackId>,
    now_ms: u64,
) -> Result<bool, PlaybackError> {
    let Some(interrupt) = state.interrupt.as_mut() else {
        return Ok(false);
    };
    if expected_track_id.is_some() && Some(interrupt.current_track_id) != expected_track_id {
        return Ok(false);
    }
    if !interrupt.queue.is_empty() {
        interrupt.current_track_id = interrupt.queue.remove(0);
        interrupt.position_ms = 0;
        interrupt.position_anchor_ms = Some(now_ms);
        increment_position_epoch(state)?;
        return Ok(true);
    }
    end_interrupt(state, now_ms)
}

fn interrupt_seek(
    state: &mut PlaybackState,
    position_ms: u64,
    now_ms: u64,
) -> Result<bool, PlaybackError> {
    let Some(interrupt) = state.interrupt.as_mut() else {
        return Ok(false);
    };
    interrupt.position_ms = position_ms.min(MAX_POSITION_MS);
    interrupt.position_anchor_ms = Some(now_ms);
    increment_position_epoch(state)?;
    Ok(true)
}

fn end_interrupt(state: &mut PlaybackState, now_ms: u64) -> Result<bool, PlaybackError> {
    let Some(interrupt) = state.interrupt.take() else {
        return Ok(false);
    };
    if interrupt.return_to_ambient {
        if state.is_playing
            && state.ambient.current_track_id.is_some()
            && state.ambient.position_anchor_ms.is_none()
        {
            state.ambient.position_anchor_ms = Some(now_ms);
        }
    } else {
        state.is_playing = false;
        freeze_ambient(&mut state.ambient, now_ms);
    }
    increment_position_epoch(state)?;
    Ok(true)
}

fn replace_if_different<T: PartialEq>(target: &mut T, replacement: T) -> bool {
    if *target == replacement {
        false
    } else {
        *target = replacement;
        true
    }
}

fn bounded_position_add(position_ms: u64, elapsed_ms: u64) -> u64 {
    position_ms.saturating_add(elapsed_ms).min(MAX_POSITION_MS)
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::error::Error;

    use proptest::prelude::*;

    use super::{
        ClockSample, CrossfadeType, LoopMode, PlaybackCommand, PlaybackError, PlaybackState,
        ReductionContext, ShuffleMode, TrackId, UnitInterval, materialize_positions, reduce,
    };

    fn track(id: i64) -> Result<TrackId, PlaybackError> {
        TrackId::new(id)
    }

    fn clock(monotonic_ms: u64) -> Result<ClockSample, PlaybackError> {
        ClockSample::new(monotonic_ms, 1_800_000_000.0 + monotonic_ms as f64 / 1000.0)
    }

    fn context(monotonic_ms: u64) -> Result<ReductionContext, PlaybackError> {
        Ok(ReductionContext {
            clock: clock(monotonic_ms)?,
            random_queue_index: None,
        })
    }

    fn apply(
        state: &PlaybackState,
        command: PlaybackCommand,
        monotonic_ms: u64,
    ) -> Result<PlaybackState, PlaybackError> {
        Ok(reduce(state, command, context(monotonic_ms)?)?.next_state)
    }

    #[test]
    fn authoritative_clock_freezes_and_resumes_without_bumping_epoch() -> Result<(), Box<dyn Error>>
    {
        let state = apply(
            &PlaybackState::default(),
            PlaybackCommand::AmbientPlayTrack(track(1)?),
            1_000,
        )?;
        assert_eq!(state.position_epoch, 1);
        assert_eq!(
            materialize_positions(&state, 3_500).ambient.position_ms,
            2_500
        );

        let paused = apply(&state, PlaybackCommand::SetPlaying(false), 3_500)?;
        assert_eq!(paused.ambient.position_ms, 2_500);
        assert_eq!(paused.ambient.position_anchor_ms, None);
        assert_eq!(paused.position_epoch, 1);
        assert_eq!(
            materialize_positions(&paused, 20_000).ambient.position_ms,
            2_500
        );

        let resumed = apply(&paused, PlaybackCommand::SetPlaying(true), 20_000)?;
        assert_eq!(resumed.position_epoch, 1);
        assert_eq!(
            materialize_positions(&resumed, 21_250).ambient.position_ms,
            3_750
        );
        Ok(())
    }

    #[test]
    fn queue_modes_shuffle_and_idempotency_match_the_python_contract() -> Result<(), Box<dyn Error>>
    {
        let mut state = apply(
            &PlaybackState::default(),
            PlaybackCommand::AmbientPlaySequence {
                track_ids: vec![track(1)?, track(2)?, track(3)?],
                start_index: 0,
                source_playlist_id: Some(9),
            },
            100,
        )?;
        state = apply(
            &state,
            PlaybackCommand::AmbientSetShuffle(ShuffleMode::Random),
            100,
        )?;
        let shuffled = reduce(
            &state,
            PlaybackCommand::AmbientSkipNext {
                follow_next_id: None,
                expected_track_id: Some(track(1)?),
            },
            ReductionContext {
                clock: clock(200)?,
                random_queue_index: Some(1),
            },
        )?
        .next_state;
        assert_eq!(shuffled.ambient.current_track_id, Some(track(3)?));
        assert_eq!(shuffled.ambient.queue, [track(2)?]);
        assert_eq!(shuffled.ambient.history, [track(1)?]);

        let duplicate = reduce(
            &shuffled,
            PlaybackCommand::AmbientSkipNext {
                follow_next_id: None,
                expected_track_id: Some(track(1)?),
            },
            ReductionContext {
                clock: clock(201)?,
                random_queue_index: Some(0),
            },
        )?;
        assert!(!duplicate.changed);
        assert_eq!(duplicate.next_state, shuffled);

        let mut repeat = shuffled;
        repeat.ambient.queue.clear();
        repeat.ambient.loop_mode = LoopMode::Queue;
        let wrapped = apply(
            &repeat,
            PlaybackCommand::AmbientSkipNext {
                follow_next_id: None,
                expected_track_id: None,
            },
            300,
        )?;
        assert_eq!(wrapped.ambient.current_track_id, Some(track(1)?));
        assert_eq!(wrapped.ambient.queue, [track(3)?]);
        assert!(wrapped.ambient.history.is_empty());
        Ok(())
    }

    #[test]
    fn pausing_and_ducking_interrupts_have_distinct_ambient_clock_semantics()
    -> Result<(), Box<dyn Error>> {
        let ambient = apply(
            &PlaybackState::default(),
            PlaybackCommand::AmbientPlayTrack(track(1)?),
            1_000,
        )?;
        let pausing = apply(
            &ambient,
            PlaybackCommand::FireInterruptSequence {
                track_ids: vec![track(2)?],
                return_to_ambient: true,
                fade_in_ms: 100,
                fade_out_ms: 200,
                duck_to: None,
            },
            2_000,
        )?;
        assert_eq!(pausing.ambient.position_ms, 1_000);
        assert_eq!(pausing.ambient.position_anchor_ms, None);
        let resumed = apply(&pausing, PlaybackCommand::CancelInterrupt, 5_000)?;
        assert_eq!(resumed.ambient.position_anchor_ms, Some(5_000));

        let ducked = apply(
            &ambient,
            PlaybackCommand::FireInterruptSequence {
                track_ids: vec![track(2)?],
                return_to_ambient: true,
                fade_in_ms: 0,
                fade_out_ms: 0,
                duck_to: Some(UnitInterval::new(0.25)?),
            },
            2_000,
        )?;
        assert_eq!(ducked.ambient.position_anchor_ms, Some(1_000));
        assert_eq!(
            materialize_positions(&ducked, 5_000).ambient.position_ms,
            4_000
        );
        Ok(())
    }

    #[test]
    fn volume_migration_model_and_presets_are_single_source_of_truth() -> Result<(), Box<dyn Error>>
    {
        let half = UnitInterval::new(0.5)?;
        let quarter = UnitInterval::new(0.25)?;
        let mut state = PlaybackState {
            default_device_volume: half,
            device_volumes: BTreeMap::from([
                ("loud".to_owned(), UnitInterval::new(1.0)?),
                ("quiet".to_owned(), half),
            ]),
            ..PlaybackState::default()
        };
        state = apply(&state, PlaybackCommand::SetGroupVolume(half), 0)?;
        assert_eq!(state.default_device_volume, half);
        assert_eq!(state.device_volumes["loud"], half);
        assert_eq!(state.device_volumes["quiet"], quarter);

        state = apply(
            &state,
            PlaybackCommand::SetActivePresets {
                preset_ids: vec!["warm".to_owned(), "warm".to_owned(), "room".to_owned()],
                crossfade_ms: Some(750),
            },
            0,
        )?;
        assert_eq!(state.active_preset_ids, ["warm", "room"]);
        assert_eq!(state.crossfade_ms, 750);
        state = apply(
            &state,
            PlaybackCommand::SetCrossfade {
                crossfade_ms: 250,
                crossfade_type: Some(CrossfadeType::EqualPower),
            },
            0,
        )?;
        assert_eq!(state.crossfade_type, CrossfadeType::EqualPower);
        Ok(())
    }

    #[test]
    fn startup_normalization_never_auto_resumes_session_state() -> Result<(), Box<dyn Error>> {
        let mut state = apply(
            &PlaybackState::default(),
            PlaybackCommand::AmbientPlayTrack(track(1)?),
            1_000,
        )?;
        state
            .active_output_device_ids
            .push("living-room".to_owned());
        state.interrupt = Some(super::InterruptState {
            current_track_id: track(2)?,
            queue: Vec::new(),
            position_ms: 0,
            position_anchor_ms: Some(2_000),
            return_to_ambient: true,
            fade_in_ms: 0,
            fade_out_ms: 0,
            duck_to: None,
        });
        state.normalize_for_startup();
        assert!(!state.is_playing);
        assert!(state.active_output_device_ids.is_empty());
        assert!(state.interrupt.is_none());
        assert_eq!(state.ambient.current_track_id, Some(track(1)?));
        assert_eq!(state.ambient.position_anchor_ms, None);
        Ok(())
    }

    proptest! {
        #[test]
        fn ticking_position_materialization_is_monotonic_and_bounded(
            initial_position in 0_u64..=i64::MAX as u64,
            anchor in any::<u64>(),
            elapsed in any::<u64>(),
        ) {
            let mut state = PlaybackState::default();
            state.ambient.position_ms = initial_position;
            state.ambient.position_anchor_ms = Some(anchor);
            let now = anchor.saturating_add(elapsed);

            let materialized = materialize_positions(&state, now);

            prop_assert!(materialized.ambient.position_ms >= initial_position);
            prop_assert!(materialized.ambient.position_ms <= i64::MAX as u64);
            prop_assert_eq!(
                materialized.ambient.position_ms,
                initial_position.saturating_add(now.saturating_sub(anchor)).min(i64::MAX as u64),
            );
        }

        #[test]
        fn shuffled_queue_advance_conserves_tracks_and_bumps_epoch(
            current_id in 1_i64..=10_000,
            queued_ids in prop::collection::vec(1_i64..=10_000, 1..32),
            raw_choice in any::<usize>(),
        ) {
            let current = TrackId::new(current_id)?;
            let queue = queued_ids
                .into_iter()
                .map(TrackId::new)
                .collect::<Result<Vec<_>, _>>()?;
            let choice = raw_choice % queue.len();
            let state = PlaybackState {
                is_playing: true,
                position_epoch: 41,
                ambient: super::AmbientState {
                    current_track_id: Some(current),
                    queue: queue.clone(),
                    shuffle: ShuffleMode::Random,
                    ..super::AmbientState::default()
                },
                ..PlaybackState::default()
            };

            let reduction = reduce(
                &state,
                PlaybackCommand::AmbientSkipNext {
                    follow_next_id: None,
                    expected_track_id: Some(current),
                },
                ReductionContext {
                    clock: ClockSample::new(5_000, 5.0)?,
                    random_queue_index: Some(choice),
                },
            )?;

            let mut before = queue;
            before.push(current);
            before.sort();
            let mut after = reduction.next_state.ambient.queue.clone();
            after.extend(reduction.next_state.ambient.history.iter().copied());
            after.extend(reduction.next_state.ambient.current_track_id);
            after.sort();
            prop_assert_eq!(after, before);
            prop_assert_eq!(reduction.next_state.position_epoch, 42);
            prop_assert!(reduction.changed);
        }
    }
}
