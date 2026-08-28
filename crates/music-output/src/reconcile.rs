use music_protocol::PlayerState;
use serde::Serialize;

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum VolumeContract {
    CanonicalV2,
    LegacyMasterTimesTrim,
}

#[derive(Debug, Clone, PartialEq)]
pub enum PlaybackCommand {
    SetVolume(f64),
    Load { url: String, start_seconds: f64 },
    SetPaused(bool),
    Stop,
    SeekAbsolute(f64),
}

#[derive(Debug, Clone, PartialEq)]
pub struct ReconcileOutcome {
    pub accepted: bool,
    pub commands: Vec<PlaybackCommand>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ControlSnapshot {
    pub on: bool,
    pub volume: f64,
    pub is_playing: bool,
    pub track_id: Option<i64>,
}

#[derive(Debug)]
pub struct Reconciler {
    server_url: String,
    client_id: String,
    respect_console: bool,
    local_on: bool,
    local_volume: f64,
    state: Option<PlayerState>,
    volume_contract: VolumeContract,
    loaded_url: Option<String>,
    last_epoch: Option<i64>,
    has_state_this_connection: bool,
}

impl Reconciler {
    #[must_use]
    pub fn new(
        server_url: String,
        client_id: String,
        respect_console: bool,
        local_on: bool,
        local_volume: f64,
    ) -> Self {
        Self {
            server_url: server_url.trim_end_matches('/').to_owned(),
            client_id,
            respect_console,
            local_on,
            local_volume: clamp01(local_volume),
            state: None,
            volume_contract: VolumeContract::CanonicalV2,
            loaded_url: None,
            last_epoch: None,
            has_state_this_connection: false,
        }
    }

    pub const fn begin_connection(&mut self) {
        self.has_state_this_connection = false;
    }

    pub fn on_state(
        &mut self,
        state: PlayerState,
        volume_contract: VolumeContract,
    ) -> ReconcileOutcome {
        if self.has_state_this_connection
            && self
                .state
                .as_ref()
                .is_some_and(|current| state.revision < current.revision)
        {
            return ReconcileOutcome {
                accepted: false,
                commands: Vec::new(),
            };
        }
        self.state = Some(state);
        self.volume_contract = volume_contract;
        self.has_state_this_connection = true;
        ReconcileOutcome {
            accepted: true,
            commands: self.reconcile(),
        }
    }

    pub fn set_local(&mut self, on: Option<bool>, volume: Option<f64>) -> Vec<PlaybackCommand> {
        if let Some(on) = on {
            self.local_on = on;
        }
        if let Some(volume) = volume {
            self.local_volume = clamp01(volume);
        }
        self.reconcile()
    }

    #[must_use]
    pub fn output_volume(&self, event_volume: f64) -> f64 {
        let server_volume = self.state.as_ref().map_or(1.0, |state| {
            let device_volume = state.device_volumes.get(&self.client_id).copied();
            match self.volume_contract {
                VolumeContract::CanonicalV2 => device_volume.unwrap_or(state.default_device_volume),
                VolumeContract::LegacyMasterTimesTrim => {
                    state.volume * device_volume.unwrap_or(1.0)
                }
            }
        });
        clamp01(event_volume) * clamp01(server_volume) * self.local_volume
    }

    #[must_use]
    pub fn control_snapshot(&self) -> ControlSnapshot {
        let (track_id, is_playing) = self.active_lane();
        ControlSnapshot {
            on: self.local_on,
            volume: self.local_volume,
            is_playing,
            track_id,
        }
    }

    #[must_use]
    pub fn may_report_position(&self) -> bool {
        let Some(state) = self.state.as_ref() else {
            return false;
        };
        let (track_id, playing) = self.active_lane();
        self.local_on
            && playing
            && track_id.is_some()
            && state
                .active_output_device_ids
                .iter()
                .any(|device_id| device_id == &self.client_id)
    }

    #[must_use]
    pub fn sfx_allowed(&self) -> bool {
        let Some(state) = self.state.as_ref() else {
            return false;
        };
        self.local_on
            && (!self.respect_console
                || state
                    .active_output_device_ids
                    .iter()
                    .any(|device_id| device_id == &self.client_id))
    }

    fn reconcile(&mut self) -> Vec<PlaybackCommand> {
        let Some(state) = self.state.as_ref() else {
            return Vec::new();
        };
        let (track_id, playing, position_ms) = if let Some(interrupt) = state.interrupt.as_ref() {
            (
                Some(interrupt.current_track_id),
                true,
                interrupt.position_ms,
            )
        } else {
            (
                state.ambient.current_track_id,
                state.is_playing,
                state.ambient.position_ms,
            )
        };
        let console_on = state
            .active_output_device_ids
            .iter()
            .any(|device_id| device_id == &self.client_id);
        let on = self.local_on && (!self.respect_console || console_on);
        let mut commands = vec![PlaybackCommand::SetVolume(self.output_volume(1.0))];
        if !on || track_id.is_none() || !playing {
            if track_id.is_none() {
                commands.push(PlaybackCommand::Stop);
                self.loaded_url = None;
            } else {
                commands.push(PlaybackCommand::SetPaused(true));
            }
            return commands;
        }

        let track_id = track_id.unwrap_or_default();
        let url = self.stream_url(track_id);
        let start_seconds = nonnegative_seconds(position_ms);
        if self.loaded_url.as_deref() != Some(&url) {
            commands.push(PlaybackCommand::Load {
                url: url.clone(),
                start_seconds,
            });
            self.loaded_url = Some(url);
        } else {
            if self
                .last_epoch
                .is_some_and(|epoch| epoch != state.position_epoch)
            {
                commands.push(PlaybackCommand::SeekAbsolute(start_seconds));
            }
            commands.push(PlaybackCommand::SetPaused(false));
        }
        self.last_epoch = Some(state.position_epoch);
        commands
    }

    fn active_lane(&self) -> (Option<i64>, bool) {
        self.state.as_ref().map_or((None, false), |state| {
            state.interrupt.as_ref().map_or(
                (state.ambient.current_track_id, state.is_playing),
                |interrupt| (Some(interrupt.current_track_id), true),
            )
        })
    }

    fn stream_url(&self, track_id: i64) -> String {
        format!("{}/api/library/tracks/{track_id}/stream", self.server_url)
    }
}

fn clamp01(value: f64) -> f64 {
    if value.is_finite() {
        value.clamp(0.0, 1.0)
    } else {
        0.0
    }
}

fn nonnegative_seconds(position_ms: i64) -> f64 {
    (position_ms.max(0) as f64) / 1_000.0
}

#[cfg(test)]
mod tests {
    use music_protocol::{AmbientState, InterruptState};

    use super::*;

    fn state(revision: i64, track_id: i64, position_ms: i64, epoch: i64) -> PlayerState {
        PlayerState {
            revision,
            position_epoch: epoch,
            is_playing: true,
            ambient: AmbientState {
                current_track_id: Some(track_id),
                position_ms,
                ..AmbientState::default()
            },
            ..PlayerState::default()
        }
    }

    fn reconciler() -> Reconciler {
        Reconciler::new(
            "http://music.test/".to_owned(),
            "headless-fixture".to_owned(),
            false,
            true,
            0.5,
        )
    }

    #[test]
    fn seeks_only_for_track_changes_and_position_epochs() {
        let mut reconciler = reconciler();
        let first = reconciler.on_state(state(1, 7, 5_000, 2), VolumeContract::CanonicalV2);
        assert!(first.commands.contains(&PlaybackCommand::Load {
            url: "http://music.test/api/library/tracks/7/stream".to_owned(),
            start_seconds: 5.0,
        }));

        let clock_only = reconciler.on_state(state(2, 7, 9_000, 2), VolumeContract::CanonicalV2);
        assert!(
            !clock_only
                .commands
                .iter()
                .any(|command| matches!(command, PlaybackCommand::SeekAbsolute(_)))
        );

        let seek = reconciler.on_state(state(3, 7, 2_500, 3), VolumeContract::CanonicalV2);
        assert!(seek.commands.contains(&PlaybackCommand::SeekAbsolute(2.5)));
    }

    #[test]
    fn rejects_stale_revisions_only_within_one_connection() {
        let mut reconciler = reconciler();
        assert!(
            reconciler
                .on_state(state(5, 7, 0, 1), VolumeContract::CanonicalV2)
                .accepted
        );
        assert!(
            !reconciler
                .on_state(state(4, 8, 0, 2), VolumeContract::CanonicalV2)
                .accepted
        );
        reconciler.begin_connection();
        assert!(
            reconciler
                .on_state(state(1, 8, 0, 2), VolumeContract::CanonicalV2)
                .accepted
        );
    }

    #[test]
    fn interrupt_wins_and_console_activation_is_an_optional_gate() {
        let mut reconciler = Reconciler::new(
            "http://music.test".to_owned(),
            "device".to_owned(),
            true,
            true,
            1.0,
        );
        let mut current = state(1, 7, 0, 1);
        current.interrupt = Some(InterruptState {
            current_track_id: 99,
            queue: Vec::new(),
            position_ms: 1_250,
            position_anchored_at: None,
            return_to_ambient: true,
            fade_in_ms: 0,
            fade_out_ms: 0,
            duck_to: None,
        });
        let off = reconciler.on_state(current.clone(), VolumeContract::CanonicalV2);
        assert!(off.commands.contains(&PlaybackCommand::SetPaused(true)));
        current.revision = 2;
        current.active_output_device_ids.push("device".to_owned());
        let on = reconciler.on_state(current, VolumeContract::CanonicalV2);
        assert!(on.commands.contains(&PlaybackCommand::Load {
            url: "http://music.test/api/library/tracks/99/stream".to_owned(),
            start_seconds: 1.25,
        }));
    }

    #[test]
    fn supports_canonical_and_legacy_volume_contracts() {
        let mut reconciler = reconciler();
        let mut current = state(1, 7, 0, 1);
        current.default_device_volume = 0.8;
        current.volume = 0.25;
        current
            .device_volumes
            .insert("headless-fixture".to_owned(), 0.4);
        reconciler.on_state(current.clone(), VolumeContract::CanonicalV2);
        assert_eq!(reconciler.output_volume(0.5), 0.1);
        current.revision = 2;
        reconciler.on_state(current, VolumeContract::LegacyMasterTimesTrim);
        assert_eq!(reconciler.output_volume(0.5), 0.025);
    }
}
