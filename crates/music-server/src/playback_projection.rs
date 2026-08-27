use std::error::Error;
use std::fmt::{self, Display, Formatter};

use music_application::playback::PlaybackPublication;
use music_domain as domain;
use music_protocol as protocol;

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum ProjectionError {
    NumericRange,
}

impl Display for ProjectionError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        formatter.write_str("playback state exceeds the public protocol numeric range")
    }
}

impl Error for ProjectionError {}

pub fn canonical_state(
    publication: &PlaybackPublication,
) -> Result<protocol::PlayerState, ProjectionError> {
    let state = publication.state.as_ref();
    Ok(protocol::PlayerState {
        revision: wire_integer(state.publication_revision)?,
        position_epoch: wire_integer(state.position_epoch)?,
        is_playing: state.is_playing,
        volume: 1.0,
        active_mode_id: state.active_mode_id.clone(),
        active_output_device_ids: state.active_output_device_ids.clone(),
        default_device_volume: state.default_device_volume.get(),
        device_volumes: state
            .device_volumes
            .iter()
            .map(|(device_id, volume)| (device_id.clone(), volume.get()))
            .collect(),
        active_soundboard_id: state.active_soundboard_id.clone(),
        active_preset_ids: state.active_preset_ids.clone(),
        preset_revision: wire_integer(state.preset_revision)?,
        crossfade_ms: i64::from(state.crossfade_ms),
        crossfade_type: crossfade_type(state.crossfade_type),
        ambient: protocol::AmbientState {
            current_track_id: state.ambient.current_track_id.map(domain::TrackId::get),
            queue: state
                .ambient
                .queue
                .iter()
                .copied()
                .map(domain::TrackId::get)
                .collect(),
            history: state
                .ambient
                .history
                .iter()
                .copied()
                .map(domain::TrackId::get)
                .collect(),
            position_ms: wire_integer(state.ambient.position_ms)?,
            position_anchored_at: state
                .ambient
                .position_anchor_ms
                .map(|_| publication.sampled_at.unix_seconds),
            loop_mode: loop_mode(state.ambient.loop_mode),
            shuffle: shuffle_mode(state.ambient.shuffle),
            source_playlist_id: state.ambient.source_playlist_id,
        },
        interrupt: state
            .interrupt
            .as_ref()
            .map(|interrupt| {
                Ok(protocol::InterruptState {
                    current_track_id: interrupt.current_track_id.get(),
                    queue: interrupt
                        .queue
                        .iter()
                        .copied()
                        .map(domain::TrackId::get)
                        .collect(),
                    position_ms: wire_integer(interrupt.position_ms)?,
                    position_anchored_at: interrupt
                        .position_anchor_ms
                        .map(|_| publication.sampled_at.unix_seconds),
                    return_to_ambient: interrupt.return_to_ambient,
                    fade_in_ms: i64::from(interrupt.fade_in_ms),
                    fade_out_ms: i64::from(interrupt.fade_out_ms),
                    duck_to: interrupt.duck_to.map(domain::UnitInterval::get),
                })
            })
            .transpose()?,
        looping_sfx: state
            .looping_sfx
            .iter()
            .map(|looping_sfx| protocol::LoopingSfx {
                id: looping_sfx.id.clone(),
                name: looping_sfx.name.clone(),
                soundboard_id: looping_sfx.soundboard_id.clone(),
                item_path: looping_sfx.item_path.clone(),
                interval_s: looping_sfx.interval_seconds,
                volume: looping_sfx.volume.get(),
            })
            .collect(),
        last_position_report: state
            .last_position_report
            .as_ref()
            .map(|report| {
                Ok(protocol::PositionReport {
                    device_id: report.device_id.clone(),
                    position_ms: wire_integer(report.position_ms)?,
                    reported_at: report.reported_at_unix_seconds,
                })
            })
            .transpose()?,
        connected_devices: publication
            .connected_clients
            .iter()
            .map(|client| protocol::DeviceInfo {
                device_id: client.client_id.clone(),
                client_id: client.client_id.clone(),
                name: client.name.clone(),
                is_output: client.is_default_output,
            })
            .collect(),
    })
}

#[must_use]
pub fn guest_state(
    mut state: protocol::PlayerState,
    own_client_id: Option<&str>,
) -> protocol::PlayerState {
    state.active_output_device_ids = own_client_id
        .filter(|client_id| {
            state
                .active_output_device_ids
                .iter()
                .any(|active| active == client_id)
        })
        .map(|client_id| vec![client_id.to_owned()])
        .unwrap_or_default();
    state.connected_devices.clear();
    state
        .device_volumes
        .retain(|device_id, _| own_client_id.is_some_and(|client_id| client_id == device_id));
    state
}

#[must_use]
pub fn legacy_state(mut state: protocol::PlayerState) -> protocol::PlayerState {
    let master = state
        .device_volumes
        .values()
        .fold(state.default_device_volume, |largest, level| {
            largest.max(*level)
        });
    state.volume = master;
    if master > 1.0e-9 {
        for level in state.device_volumes.values_mut() {
            *level /= master;
        }
    } else {
        for level in state.device_volumes.values_mut() {
            *level = 1.0;
        }
    }
    state
}

fn wire_integer(value: u64) -> Result<i64, ProjectionError> {
    i64::try_from(value).map_err(|_| ProjectionError::NumericRange)
}

fn crossfade_type(value: domain::CrossfadeType) -> protocol::CrossfadeType {
    match value {
        domain::CrossfadeType::Linear => protocol::CrossfadeType::Linear,
        domain::CrossfadeType::EqualPower => protocol::CrossfadeType::EqualPower,
        domain::CrossfadeType::Cut => protocol::CrossfadeType::Cut,
    }
}

fn loop_mode(value: domain::LoopMode) -> protocol::LoopMode {
    match value {
        domain::LoopMode::Off => protocol::LoopMode::Off,
        domain::LoopMode::Follow => protocol::LoopMode::Follow,
        domain::LoopMode::Queue => protocol::LoopMode::Queue,
        domain::LoopMode::Track => protocol::LoopMode::Track,
    }
}

fn shuffle_mode(value: domain::ShuffleMode) -> protocol::ShuffleMode {
    match value {
        domain::ShuffleMode::Off => protocol::ShuffleMode::Off,
        domain::ShuffleMode::Random => protocol::ShuffleMode::Random,
    }
}

#[cfg(test)]
mod tests {
    use std::error::Error;
    use std::sync::Arc;

    use music_application::playback::{ConnectedClient, PlaybackPublication};
    use music_domain::{ClockSample, PlaybackState, UnitInterval};

    use super::{canonical_state, guest_state, legacy_state};

    #[test]
    fn guest_projection_reveals_only_its_own_output_capability() -> Result<(), Box<dyn Error>> {
        let publication = PlaybackPublication {
            state: Arc::new(PlaybackState {
                active_output_device_ids: vec!["mine".to_owned(), "other".to_owned()],
                device_volumes: [
                    ("mine".to_owned(), UnitInterval::new(0.5)?),
                    ("other".to_owned(), UnitInterval::new(0.8)?),
                ]
                .into_iter()
                .collect(),
                ..PlaybackState::default()
            }),
            connected_clients: vec![
                ConnectedClient {
                    client_id: "mine".to_owned(),
                    name: "Mine".to_owned(),
                    is_default_output: false,
                },
                ConnectedClient {
                    client_id: "other".to_owned(),
                    name: "Other".to_owned(),
                    is_default_output: true,
                },
            ]
            .into(),
            sampled_at: ClockSample::new(0, 1_800_000_000.0)?,
        };

        let state = guest_state(canonical_state(&publication)?, Some("mine"));
        assert_eq!(state.active_output_device_ids, ["mine"]);
        assert_eq!(state.device_volumes.len(), 1);
        assert_eq!(state.device_volumes["mine"], 0.5);
        assert!(state.connected_devices.is_empty());
        Ok(())
    }

    #[test]
    fn legacy_projection_preserves_absolute_ratios() {
        let state = music_protocol::PlayerState {
            default_device_volume: 0.4,
            device_volumes: [("a".to_owned(), 0.8), ("b".to_owned(), 0.2)]
                .into_iter()
                .collect(),
            ..music_protocol::PlayerState::default()
        };

        let legacy = legacy_state(state);
        assert_eq!(legacy.volume, 0.8);
        assert_eq!(legacy.device_volumes["a"], 1.0);
        assert_eq!(legacy.device_volumes["b"], 0.25);
    }
}
