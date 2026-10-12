//! Playback requests that name library or mode resources.
//!
//! Each request is resolved against the current library and mode catalogs and
//! submitted at that catalog generation. When a concurrent library or mode
//! change makes the resolution stale, the actor rejects it and the request is
//! resolved once more instead of installing references to removed resources.

use std::fmt::{self, Display, Formatter};
use std::sync::Arc;

use music_domain::{
    CuePlayback, CuePlaylist, CuePreset, CueSfx, InterruptFade, LoopingSfx, PlaybackCommand,
    PlaybackState, TrackId, UnitInterval,
};

use super::{CatalogGeneration, PlaybackActorError, PlaybackActorHandle, ResolvedPlaybackCommand};
use crate::library::{LibraryCoordinatorHandle, LibraryService};
use crate::modes::{ModeBundle, ModeCatalog, ModeCoordinatorHandle, SoundboardItemDocument};
use crate::playlists::{PlaylistFilter, PlaylistService, PlaylistServiceError};

const ATTEMPTS: usize = 2;

/// A playback request whose references must exist in the current catalogs.
#[derive(Debug, Clone, PartialEq)]
pub enum CatalogRequest {
    SetActiveMode(Option<String>),
    PlayTrack(TrackId),
    SetQueue(Vec<TrackId>),
    Enqueue {
        track_id: TrackId,
        position: Option<usize>,
    },
    PlayPlaylist {
        playlist_id: i64,
        start_index: usize,
    },
    PlayFolder {
        path: String,
        start_index: usize,
    },
    SetActiveSoundboard(Option<String>),
    SetActivePresets(Vec<String>),
    InterruptTrack {
        track_id: TrackId,
        fade: InterruptFade,
    },
    InterruptPlaylist {
        playlist_id: i64,
        fade: InterruptFade,
    },
    FireCue(String),
    FireSfx {
        soundboard_id: String,
        item_path: String,
        volume: UnitInterval,
    },
    StartLoop(LoopingSfx),
}

/// Why a catalog request was not applied. `Display` is the operator-facing
/// message sent to the requesting client.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CatalogCommandError {
    NotReady,
    PlaybackFailed,
    CatalogChanged,
    UnknownMode(String),
    ModeChanged,
    TrackNotInLibrary,
    NoActiveMode,
    NoActiveModeForSoundboard,
    SoundboardNotInActiveMode { soundboard: String, mode: String },
    SoundboardChanged,
    UnknownPresets(Vec<String>),
    PresetCrossfadeTooLarge,
    PresetSelectionChanged,
    Playlist(PlaylistProblem),
    PlaylistEmpty,
    PlaylistTrackMissing,
    LibraryUnavailable,
    NoTracksInLibrary,
    NoTracksInFolder(String),
    UnknownSoundboard { soundboard: String, mode: String },
    SoundboardItemMissing { item: String, soundboard: String },
    SoundboardItemChanged,
    UnknownCue { cue: String, mode: String },
    CueUnknownPreset(String),
    CuePresetCrossfadeTooLarge,
    CueSfxUnknownSoundboard { soundboard: String, mode: String },
    CueSfxItemMissing { item: String, soundboard: String },
    CueLoopInvalid,
    VolumeOutOfRange,
    CuePlaylistMissing { name: String, mode: String },
    CuePlaylistEmpty(String),
    CueStartIndexTooLarge,
    ActiveModeChanged,
    CueChanged,
}

/// Playlist lookup failures that the client can act on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlaylistProblem {
    NotFound,
    InvalidRule,
    ConcurrentChange,
    Unavailable,
}

impl Display for CatalogCommandError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotReady => {
                formatter.write_str("the required library or mode catalog is not ready")
            }
            Self::PlaybackFailed => formatter.write_str("playback action failed"),
            Self::CatalogChanged => formatter.write_str("catalog changed; retry the request"),
            Self::UnknownMode(mode) => write!(formatter, "unknown mode: {mode}"),
            Self::ModeChanged => formatter.write_str("mode changed; retry the request"),
            Self::TrackNotInLibrary => formatter.write_str("track not in library"),
            Self::NoActiveMode => formatter.write_str("no active mode"),
            Self::NoActiveModeForSoundboard => {
                formatter.write_str("no active mode; cannot set soundboard without a mode")
            }
            Self::SoundboardNotInActiveMode { soundboard, mode } => {
                write!(
                    formatter,
                    "soundboard '{soundboard}' not in active mode '{mode}'"
                )
            }
            Self::SoundboardChanged => formatter.write_str("soundboard changed; retry the request"),
            Self::UnknownPresets(presets) => {
                write!(formatter, "unknown preset(s): {}", presets.join(", "))
            }
            Self::PresetCrossfadeTooLarge => formatter.write_str("preset crossfade is too large"),
            Self::PresetSelectionChanged => {
                formatter.write_str("preset selection changed; retry the request")
            }
            Self::Playlist(PlaylistProblem::NotFound) => formatter.write_str("playlist not found"),
            Self::Playlist(PlaylistProblem::InvalidRule) => {
                formatter.write_str("automatic playlist rule is invalid")
            }
            Self::Playlist(PlaylistProblem::ConcurrentChange) => {
                formatter.write_str("playlist changed; retry the request")
            }
            Self::Playlist(PlaylistProblem::Unavailable) => {
                formatter.write_str("playlist resolution failed")
            }
            Self::PlaylistEmpty => formatter.write_str("playlist is empty"),
            Self::PlaylistTrackMissing => {
                formatter.write_str("playlist contains a track that is no longer in the library")
            }
            Self::LibraryUnavailable => formatter.write_str("library resolution failed"),
            Self::NoTracksInLibrary => formatter.write_str("no tracks in the library"),
            Self::NoTracksInFolder(path) => write!(formatter, "no tracks in \"{path}\""),
            Self::UnknownSoundboard { soundboard, mode } => {
                write!(
                    formatter,
                    "unknown soundboard '{soundboard}' in mode '{mode}'"
                )
            }
            Self::SoundboardItemMissing { item, soundboard } => {
                write!(formatter, "item '{item}' not in soundboard '{soundboard}'")
            }
            Self::SoundboardItemChanged => {
                formatter.write_str("soundboard item changed; retry the request")
            }
            Self::UnknownCue { cue, mode } => {
                write!(formatter, "unknown cue '{cue}' in mode '{mode}'")
            }
            Self::CueUnknownPreset(preset) => {
                write!(formatter, "cue references unknown preset '{preset}'")
            }
            Self::CuePresetCrossfadeTooLarge => {
                formatter.write_str("cue preset crossfade is too large")
            }
            Self::CueSfxUnknownSoundboard { soundboard, mode } => {
                write!(
                    formatter,
                    "cue SFX: unknown soundboard '{soundboard}' in mode '{mode}'"
                )
            }
            Self::CueSfxItemMissing { item, soundboard } => {
                write!(
                    formatter,
                    "cue SFX: item '{item}' not in soundboard '{soundboard}'"
                )
            }
            Self::CueLoopInvalid => formatter.write_str("cue loop parameters are invalid"),
            Self::VolumeOutOfRange => formatter.write_str("volume is outside the supported range"),
            Self::CuePlaylistMissing { name, mode } => {
                write!(formatter, "no playlist named '{name}' for mode '{mode}'")
            }
            Self::CuePlaylistEmpty(name) => write!(formatter, "cue playlist '{name}' is empty"),
            Self::CueStartIndexTooLarge => formatter.write_str("cue start index is too large"),
            Self::ActiveModeChanged => formatter.write_str("active mode changed; retry the cue"),
            Self::CueChanged => formatter.write_str("cue changed; retry the request"),
        }
    }
}

impl std::error::Error for CatalogCommandError {}

impl From<PlaylistServiceError> for CatalogCommandError {
    fn from(error: PlaylistServiceError) -> Self {
        Self::Playlist(match error {
            PlaylistServiceError::NotFound => PlaylistProblem::NotFound,
            PlaylistServiceError::InvalidRule(_) => PlaylistProblem::InvalidRule,
            PlaylistServiceError::ConcurrentChange => PlaylistProblem::ConcurrentChange,
            error => {
                crate::log_dependency_failure("playlist resolution failed", &error);
                PlaylistProblem::Unavailable
            }
        })
    }
}

/// The catalogs and owners a catalog request is resolved against.
#[derive(Clone)]
pub struct CatalogCommands {
    playback: PlaybackActorHandle,
    library: LibraryCoordinatorHandle,
    tracks: Arc<LibraryService>,
    modes: ModeCoordinatorHandle,
    playlists: Arc<PlaylistService>,
}

/// What the actor said about one resolved attempt.
enum Attempt {
    Applied,
    Stale,
    InvalidReference,
}

impl CatalogCommands {
    #[must_use]
    pub fn new(
        playback: PlaybackActorHandle,
        library: LibraryCoordinatorHandle,
        tracks: Arc<LibraryService>,
        modes: ModeCoordinatorHandle,
        playlists: Arc<PlaylistService>,
    ) -> Self {
        Self {
            playback,
            library,
            tracks,
            modes,
            playlists,
        }
    }

    /// Resolve and apply `request`, retrying once after a concurrent change.
    pub async fn execute(&self, request: &CatalogRequest) -> Result<(), CatalogCommandError> {
        for attempt in 0..ATTEMPTS {
            let last = attempt + 1 == ATTEMPTS;
            match request {
                CatalogRequest::PlayFolder { path, start_index } => {
                    if self.play_folder(path, *start_index).await? {
                        return Ok(());
                    }
                }
                CatalogRequest::PlayPlaylist {
                    playlist_id,
                    start_index,
                } => {
                    let track_ids = self.playlist_tracks(*playlist_id).await?;
                    let command = PlaybackCommand::AmbientPlaySequence {
                        track_ids,
                        start_index: *start_index,
                        source_playlist_id: Some(*playlist_id),
                    };
                    match self.submit_current(command).await? {
                        Attempt::Applied => return Ok(()),
                        Attempt::Stale => {}
                        Attempt::InvalidReference => {
                            return Err(CatalogCommandError::PlaylistTrackMissing);
                        }
                    }
                }
                CatalogRequest::InterruptPlaylist { playlist_id, fade } => {
                    let track_ids = self.playlist_tracks(*playlist_id).await?;
                    match self.submit_current(interrupt(track_ids, *fade)).await? {
                        Attempt::Applied => return Ok(()),
                        Attempt::Stale => {}
                        Attempt::InvalidReference => {
                            return Err(CatalogCommandError::PlaylistTrackMissing);
                        }
                    }
                }
                CatalogRequest::FireCue(cue_id) => {
                    let (mode_id, command, generation) = self.resolve_cue(cue_id).await?;
                    match self.submit(command, generation).await? {
                        Attempt::Applied => return Ok(()),
                        Attempt::Stale => {}
                        Attempt::InvalidReference => {
                            let mode_changed =
                                self.playback.snapshot().await.ok().is_none_or(|snapshot| {
                                    snapshot.state.active_mode_id.as_deref()
                                        != Some(mode_id.as_str())
                                });
                            return Err(if mode_changed {
                                CatalogCommandError::ActiveModeChanged
                            } else {
                                CatalogCommandError::CueChanged
                            });
                        }
                    }
                }
                CatalogRequest::FireSfx {
                    soundboard_id,
                    item_path,
                    volume,
                } => {
                    let generation = self.check_soundboard_item(soundboard_id, item_path).await?;
                    let command = PlaybackCommand::FireSfx {
                        soundboard_id: soundboard_id.clone(),
                        item_path: item_path.clone(),
                        volume: *volume,
                    };
                    if self.submit_sfx(command, generation).await? {
                        return Ok(());
                    }
                }
                CatalogRequest::StartLoop(looping_sfx) => {
                    let generation = self
                        .check_soundboard_item(&looping_sfx.soundboard_id, &looping_sfx.item_path)
                        .await?;
                    let command = PlaybackCommand::StartLoop(looping_sfx.clone());
                    if self.submit_sfx(command, generation).await? {
                        return Ok(());
                    }
                }
                simple => {
                    let state = self.playback_state().await?;
                    let catalog = self.mode_catalog()?;
                    let command = resolve_simple(simple, &state, &catalog)?;
                    let generation = self.generation(&catalog);
                    match self.submit(command, generation).await? {
                        Attempt::Applied => return Ok(()),
                        Attempt::Stale => {}
                        // A reference can disappear between resolution and
                        // submission; only a repeated miss is reported.
                        Attempt::InvalidReference if last => {
                            return Err(invalid_reference(simple));
                        }
                        Attempt::InvalidReference => tokio::task::yield_now().await,
                    }
                }
            }
        }
        Err(CatalogCommandError::CatalogChanged)
    }

    async fn playback_state(&self) -> Result<Arc<PlaybackState>, CatalogCommandError> {
        self.playback
            .snapshot()
            .await
            .map(|publication| Arc::clone(&publication.state))
            .map_err(|_| CatalogCommandError::PlaybackFailed)
    }

    fn mode_catalog(&self) -> Result<Arc<ModeCatalog>, CatalogCommandError> {
        self.modes.snapshot().ok_or(CatalogCommandError::NotReady)
    }

    fn generation(&self, catalog: &ModeCatalog) -> CatalogGeneration {
        CatalogGeneration {
            library: self.library.status().generation.get(),
            modes: catalog.generation,
        }
    }

    async fn active_mode_id(&self) -> Result<String, CatalogCommandError> {
        self.playback_state()
            .await?
            .active_mode_id
            .clone()
            .ok_or(CatalogCommandError::NoActiveMode)
    }

    async fn submit(
        &self,
        command: PlaybackCommand,
        generation: CatalogGeneration,
    ) -> Result<Attempt, CatalogCommandError> {
        match self
            .playback
            .execute(ResolvedPlaybackCommand::at_generation(command, generation))
            .await
        {
            Ok(_) => Ok(Attempt::Applied),
            Err(PlaybackActorError::StaleCatalog { .. }) => {
                tokio::task::yield_now().await;
                Ok(Attempt::Stale)
            }
            Err(PlaybackActorError::InvalidCatalogReference) => Ok(Attempt::InvalidReference),
            Err(error) => {
                tracing::warn!(error = %crate::ErrorChain(&error), "catalog playback action was rejected");
                Err(CatalogCommandError::PlaybackFailed)
            }
        }
    }

    async fn submit_current(
        &self,
        command: PlaybackCommand,
    ) -> Result<Attempt, CatalogCommandError> {
        let catalog = self.mode_catalog()?;
        let generation = self.generation(&catalog);
        self.submit(command, generation).await
    }

    /// Returns whether the SFX command was applied (false: retry).
    async fn submit_sfx(
        &self,
        command: PlaybackCommand,
        generation: CatalogGeneration,
    ) -> Result<bool, CatalogCommandError> {
        match self.submit(command, generation).await? {
            Attempt::Applied => Ok(true),
            Attempt::Stale => Ok(false),
            Attempt::InvalidReference => Err(CatalogCommandError::SoundboardItemChanged),
        }
    }

    async fn playlist_tracks(&self, playlist_id: i64) -> Result<Vec<TrackId>, CatalogCommandError> {
        let track_ids = self.playlists.track_ids(playlist_id).await?;
        if track_ids.is_empty() {
            return Err(CatalogCommandError::PlaylistEmpty);
        }
        Ok(track_ids)
    }

    /// Returns whether the folder was queued (false: the library changed while
    /// it was read, retry).
    async fn play_folder(
        &self,
        requested: &str,
        start_index: usize,
    ) -> Result<bool, CatalogCommandError> {
        let folder = requested.trim_matches(&['/', '\\'][..]).replace('\\', "/");
        let before = self.library.status().generation;
        let tracks = self.tracks.all_tracks().await.map_err(|error| {
            crate::log_dependency_failure(
                "folder playback could not read the library",
                error.as_ref(),
            );
            CatalogCommandError::LibraryUnavailable
        })?;
        let after = self.library.status().generation;
        if before != after {
            tokio::task::yield_now().await;
            return Ok(false);
        }
        let track_ids = tracks
            .into_iter()
            .filter(|track| {
                folder.is_empty()
                    || track.path.as_str() == folder
                    || track
                        .path
                        .as_str()
                        .strip_prefix(folder.as_str())
                        .is_some_and(|suffix| suffix.starts_with('/'))
            })
            .map(|track| track.id)
            .collect::<Vec<_>>();
        if track_ids.is_empty() {
            return Err(if requested.is_empty() {
                CatalogCommandError::NoTracksInLibrary
            } else {
                CatalogCommandError::NoTracksInFolder(requested.to_owned())
            });
        }
        let catalog = self.mode_catalog()?;
        let generation = CatalogGeneration {
            library: after.get(),
            modes: catalog.generation,
        };
        let command = PlaybackCommand::AmbientPlaySequence {
            track_ids,
            start_index,
            source_playlist_id: None,
        };
        match self.submit(command, generation).await? {
            Attempt::Applied => Ok(true),
            Attempt::Stale => Ok(false),
            Attempt::InvalidReference => {
                tokio::task::yield_now().await;
                Ok(false)
            }
        }
    }

    async fn check_soundboard_item(
        &self,
        soundboard_id: &str,
        item_path: &str,
    ) -> Result<CatalogGeneration, CatalogCommandError> {
        let mode_id = self.active_mode_id().await?;
        let catalog = self.mode_catalog()?;
        let unknown_soundboard = || CatalogCommandError::UnknownSoundboard {
            soundboard: soundboard_id.to_owned(),
            mode: mode_id.clone(),
        };
        let soundboard = catalog
            .modes
            .get(&mode_id)
            .ok_or_else(unknown_soundboard)?
            .soundboards
            .get(soundboard_id)
            .ok_or_else(unknown_soundboard)?;
        let item_exists = soundboard
            .categories
            .iter()
            .flat_map(|category| &category.items)
            .any(|item| item.file == item_path);
        if !item_exists {
            return Err(CatalogCommandError::SoundboardItemMissing {
                item: item_path.to_owned(),
                soundboard: soundboard_id.to_owned(),
            });
        }
        Ok(self.generation(&catalog))
    }

    async fn resolve_cue(
        &self,
        cue_id: &str,
    ) -> Result<(String, PlaybackCommand, CatalogGeneration), CatalogCommandError> {
        let mode_id = self.active_mode_id().await?;
        let catalog = self.mode_catalog()?;
        let unknown_cue = || CatalogCommandError::UnknownCue {
            cue: cue_id.to_owned(),
            mode: mode_id.clone(),
        };
        let mode = catalog.modes.get(&mode_id).ok_or_else(unknown_cue)?;
        let cue = mode.cues.get(cue_id).ok_or_else(unknown_cue)?;

        let preset = match cue.preset.as_deref() {
            Some(preset_id) => {
                let preset = mode
                    .presets
                    .get(preset_id)
                    .ok_or_else(|| CatalogCommandError::CueUnknownPreset(preset_id.to_owned()))?;
                Some(CuePreset {
                    preset_id: preset_id.to_owned(),
                    crossfade_ms: preset
                        .crossfade_ms
                        .map(u32::try_from)
                        .transpose()
                        .map_err(|_| CatalogCommandError::CuePresetCrossfadeTooLarge)?,
                })
            }
            None => None,
        };

        let mut loops = Vec::with_capacity(cue.loops.len());
        for (index, spec) in cue.loops.iter().enumerate() {
            let item = cue_item(mode, &mode_id, &spec.soundboard, &spec.item)?;
            loops.push(
                LoopingSfx::new(
                    format!("cue:{cue_id}:{index}"),
                    item.name.clone(),
                    spec.soundboard.clone(),
                    spec.item.clone(),
                    spec.interval_s,
                    volume(spec.volume)?,
                )
                .map_err(|_| CatalogCommandError::CueLoopInvalid)?,
            );
        }
        let mut sfx = Vec::with_capacity(cue.sfx.len());
        for spec in &cue.sfx {
            cue_item(mode, &mode_id, &spec.soundboard, &spec.item)?;
            sfx.push(CueSfx {
                soundboard_id: spec.soundboard.clone(),
                item_path: spec.item.clone(),
                volume: volume(spec.volume)?,
            });
        }

        let playlist = match cue.playlist.as_deref() {
            Some(name) => {
                let candidates = self
                    .playlists
                    .list(&PlaylistFilter {
                        mode_id: Some(mode_id.clone()),
                        category: None,
                    })
                    .await?;
                let playlist = candidates
                    .into_iter()
                    .filter(|playlist| playlist.name == name)
                    .min_by_key(|playlist| playlist.id)
                    .ok_or_else(|| CatalogCommandError::CuePlaylistMissing {
                        name: name.to_owned(),
                        mode: mode_id.clone(),
                    })?;
                let track_ids = self.playlists.track_ids(playlist.id).await?;
                if track_ids.is_empty() {
                    return Err(CatalogCommandError::CuePlaylistEmpty(name.to_owned()));
                }
                Some(CuePlaylist {
                    track_ids,
                    start_index: usize::try_from(cue.start_index)
                        .map_err(|_| CatalogCommandError::CueStartIndexTooLarge)?,
                    start_ms: cue.start_ms,
                    source_playlist_id: playlist.id,
                })
            }
            None => None,
        };

        let command = PlaybackCommand::FireCue(CuePlayback {
            mode_id: mode_id.clone(),
            cue_id: cue_id.to_owned(),
            preset,
            playlist,
            loops,
            sfx,
        });
        let generation = self.generation(&catalog);
        Ok((mode_id, command, generation))
    }
}

/// Resolve requests that only need the playback state and mode catalog.
fn resolve_simple(
    request: &CatalogRequest,
    state: &PlaybackState,
    catalog: &ModeCatalog,
) -> Result<PlaybackCommand, CatalogCommandError> {
    Ok(match request {
        CatalogRequest::SetActiveMode(mode_id) => {
            if let Some(mode_id) = mode_id.as_deref()
                && !catalog.modes.contains_key(mode_id)
            {
                return Err(CatalogCommandError::UnknownMode(mode_id.to_owned()));
            }
            PlaybackCommand::SetActiveMode(mode_id.clone())
        }
        CatalogRequest::PlayTrack(track_id) => PlaybackCommand::AmbientPlayTrack(*track_id),
        CatalogRequest::SetQueue(track_ids) => PlaybackCommand::AmbientSetQueue(track_ids.clone()),
        CatalogRequest::Enqueue { track_id, position } => PlaybackCommand::AmbientEnqueue {
            track_id: *track_id,
            position: *position,
        },
        CatalogRequest::InterruptTrack { track_id, fade } => interrupt(vec![*track_id], *fade),
        CatalogRequest::SetActiveSoundboard(soundboard_id) => {
            if let Some(soundboard_id) = soundboard_id.as_deref() {
                let mode_id = state
                    .active_mode_id
                    .as_deref()
                    .ok_or(CatalogCommandError::NoActiveModeForSoundboard)?;
                if !catalog
                    .modes
                    .get(mode_id)
                    .is_some_and(|mode| mode.soundboards.contains_key(soundboard_id))
                {
                    return Err(CatalogCommandError::SoundboardNotInActiveMode {
                        soundboard: soundboard_id.to_owned(),
                        mode: mode_id.to_owned(),
                    });
                }
            }
            PlaybackCommand::SetActiveSoundboard(soundboard_id.clone())
        }
        CatalogRequest::SetActivePresets(preset_ids) => {
            let mode = state
                .active_mode_id
                .as_deref()
                .and_then(|mode_id| catalog.modes.get(mode_id));
            let unknown = preset_ids
                .iter()
                .filter(|preset_id| !mode.is_some_and(|mode| mode.presets.contains_key(*preset_id)))
                .cloned()
                .collect::<Vec<_>>();
            if !unknown.is_empty() {
                return Err(CatalogCommandError::UnknownPresets(unknown));
            }
            // The last selected preset that sets a crossfade wins.
            let crossfade_ms = mode
                .and_then(|mode| {
                    preset_ids.iter().fold(None, |effective, preset_id| {
                        mode.presets
                            .get(preset_id)
                            .and_then(|preset| preset.crossfade_ms)
                            .or(effective)
                    })
                })
                .map(u32::try_from)
                .transpose()
                .map_err(|_| CatalogCommandError::PresetCrossfadeTooLarge)?;
            PlaybackCommand::SetActivePresets {
                preset_ids: preset_ids.clone(),
                crossfade_ms,
            }
        }
        CatalogRequest::PlayPlaylist { .. }
        | CatalogRequest::PlayFolder { .. }
        | CatalogRequest::InterruptPlaylist { .. }
        | CatalogRequest::FireCue(_)
        | CatalogRequest::FireSfx { .. }
        | CatalogRequest::StartLoop(_) => return Err(CatalogCommandError::PlaybackFailed),
    })
}

fn invalid_reference(request: &CatalogRequest) -> CatalogCommandError {
    match request {
        CatalogRequest::SetActiveMode(_) => CatalogCommandError::ModeChanged,
        CatalogRequest::SetActiveSoundboard(_) => CatalogCommandError::SoundboardChanged,
        CatalogRequest::SetActivePresets(_) => CatalogCommandError::PresetSelectionChanged,
        _ => CatalogCommandError::TrackNotInLibrary,
    }
}

fn interrupt(track_ids: Vec<TrackId>, fade: InterruptFade) -> PlaybackCommand {
    PlaybackCommand::FireInterruptSequence { track_ids, fade }
}

fn cue_item<'a>(
    mode: &'a ModeBundle,
    mode_id: &str,
    soundboard_id: &str,
    item_path: &str,
) -> Result<&'a SoundboardItemDocument, CatalogCommandError> {
    if !mode.soundboards.contains_key(soundboard_id) {
        return Err(CatalogCommandError::CueSfxUnknownSoundboard {
            soundboard: soundboard_id.to_owned(),
            mode: mode_id.to_owned(),
        });
    }
    mode.soundboard_item(soundboard_id, item_path)
        .ok_or_else(|| CatalogCommandError::CueSfxItemMissing {
            item: item_path.to_owned(),
            soundboard: soundboard_id.to_owned(),
        })
}

fn volume(value: f64) -> Result<UnitInterval, CatalogCommandError> {
    UnitInterval::new(value).map_err(|_| CatalogCommandError::VolumeOutOfRange)
}
