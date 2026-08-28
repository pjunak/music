use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use futures_util::StreamExt;
use music_protocol::PlayerState;
use reqwest::Url;
use serde::{Deserialize, Serialize};
use tokio::sync::Mutex;

use crate::config::OutputConfig;
use crate::mpv::{MpvError, MpvPlayer};
use crate::reconcile::{Reconciler, VolumeContract};

const MAX_METADATA_BYTES: usize = 256 * 1024;
const MAX_METADATA_CACHE_ENTRIES: usize = 256;

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ControlStatus {
    pub on: bool,
    pub volume: f64,
    pub is_playing: bool,
    pub track_id: Option<i64>,
    pub title: Option<String>,
    pub artist: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct TrackMetadata {
    title: Option<String>,
    artist: Option<String>,
}

#[derive(Debug, Deserialize)]
struct TrackMetadataResponse {
    #[serde(default)]
    title: String,
    #[serde(default)]
    display_title: String,
    #[serde(default)]
    artist: String,
}

#[derive(Debug)]
pub struct OutputRuntime {
    config: Arc<OutputConfig>,
    reconciler: Mutex<Reconciler>,
    player: Arc<MpvPlayer>,
    http: reqwest::Client,
    metadata: Mutex<BTreeMap<i64, TrackMetadata>>,
}

impl OutputRuntime {
    pub async fn start(config: OutputConfig) -> Result<Arc<Self>, MpvError> {
        let player = Arc::new(MpvPlayer::start(&config.mpv_executable, config.play_sfx).await?);
        let reconciler = Reconciler::new(
            config.server_url.to_string(),
            config.client_id.clone(),
            config.respect_console,
            config.local_on,
            config.local_volume,
        );
        let http = reqwest::Client::builder()
            .timeout(Duration::from_secs(2))
            .build()
            .map_err(|error| MpvError::configuration(error.to_string()))?;
        Ok(Arc::new(Self {
            config: Arc::new(config),
            reconciler: Mutex::new(reconciler),
            player,
            http,
            metadata: Mutex::new(BTreeMap::new()),
        }))
    }

    #[must_use]
    pub fn config(&self) -> &OutputConfig {
        &self.config
    }

    pub async fn begin_connection(&self) {
        self.reconciler.lock().await.begin_connection();
    }

    pub async fn apply_state(
        &self,
        state: PlayerState,
        volume_contract: VolumeContract,
    ) -> Result<bool, MpvError> {
        let outcome = self
            .reconciler
            .lock()
            .await
            .on_state(state, volume_contract);
        if outcome.accepted {
            self.player.execute(&outcome.commands).await?;
        }
        Ok(outcome.accepted)
    }

    pub async fn set_local(&self, on: Option<bool>, volume: Option<f64>) -> Result<(), MpvError> {
        let commands = self.reconciler.lock().await.set_local(on, volume);
        self.player.execute(&commands).await
    }

    pub async fn fire_sfx(&self, item_path: &str, event_volume: f64) -> Result<(), MpvError> {
        if !self.config.play_sfx {
            return Ok(());
        }
        let volume = {
            let reconciler = self.reconciler.lock().await;
            if !reconciler.sfx_allowed() {
                return Ok(());
            }
            reconciler.output_volume(event_volume)
        };
        let mut url = self.config.server_url.clone();
        url.set_path("/api/sfx/file");
        url.set_query(None);
        url.query_pairs_mut().append_pair("path", item_path);
        self.player.fire_sfx(url.as_str(), volume).await
    }

    pub async fn position_report_millis(&self) -> Result<Option<i64>, MpvError> {
        if !self.reconciler.lock().await.may_report_position() {
            return Ok(None);
        }
        Ok(self
            .player
            .time_position_seconds()
            .await?
            .map(seconds_to_millis))
    }

    pub async fn audio_healthcheck(&self) -> Result<(), MpvError> {
        self.player.healthcheck().await
    }

    pub async fn control_status(&self) -> ControlStatus {
        let snapshot = self.reconciler.lock().await.control_snapshot();
        let metadata = match snapshot.track_id {
            Some(track_id) => self.track_metadata(track_id).await,
            None => None,
        };
        ControlStatus {
            on: snapshot.on,
            volume: snapshot.volume,
            is_playing: snapshot.is_playing,
            track_id: snapshot.track_id,
            title: metadata.as_ref().and_then(|value| value.title.clone()),
            artist: metadata.and_then(|value| value.artist),
        }
    }

    pub async fn shutdown(&self) {
        self.player.shutdown().await;
    }

    async fn track_metadata(&self, track_id: i64) -> Option<TrackMetadata> {
        if let Some(cached) = self.metadata.lock().await.get(&track_id).cloned() {
            return Some(cached);
        }
        let mut url = self.config.server_url.clone();
        url.set_path(&format!("/api/library/tracks/{track_id}"));
        url.set_query(None);
        let fetched = fetch_metadata(&self.http, url).await?;
        let mut cache = self.metadata.lock().await;
        if cache.len() >= MAX_METADATA_CACHE_ENTRIES {
            cache.clear();
        }
        cache.insert(track_id, fetched.clone());
        Some(fetched)
    }
}

async fn fetch_metadata(client: &reqwest::Client, url: Url) -> Option<TrackMetadata> {
    let response = client.get(url).send().await.ok()?;
    if !response.status().is_success()
        || response
            .content_length()
            .is_some_and(|length| length > MAX_METADATA_BYTES as u64)
    {
        return None;
    }
    let mut stream = response.bytes_stream();
    let mut bytes = Vec::new();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.ok()?;
        if bytes.len().saturating_add(chunk.len()) > MAX_METADATA_BYTES {
            return None;
        }
        bytes.extend_from_slice(&chunk);
    }
    let response: TrackMetadataResponse = serde_json::from_slice(&bytes).ok()?;
    let title = nonempty(if response.display_title.trim().is_empty() {
        response.title
    } else {
        response.display_title
    });
    Some(TrackMetadata {
        title,
        artist: nonempty(response.artist),
    })
}

fn nonempty(value: String) -> Option<String> {
    (!value.trim().is_empty()).then_some(value)
}

fn seconds_to_millis(seconds: f64) -> i64 {
    let millis = seconds.max(0.0) * 1_000.0;
    if millis >= i64::MAX as f64 {
        i64::MAX
    } else {
        millis.round() as i64
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn position_reports_are_nonnegative_and_saturating() {
        assert_eq!(seconds_to_millis(-1.0), 0);
        assert_eq!(seconds_to_millis(1.234), 1_234);
        assert_eq!(seconds_to_millis(f64::MAX), i64::MAX);
    }
}
