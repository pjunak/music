use std::error::Error;
use std::fmt::{self, Display, Formatter};
use std::io::{self, Read};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{RecvTimeoutError, sync_channel};
use std::thread;
use std::time::Duration;

use music_application::assistant::{Confidence, LOCAL_AUDIO_ANALYZER_ID};
use serde_json::{Map, Value, json};

const TARGET_SAMPLE_RATE: u32 = 8_000;
const FRAMES_PER_CHUNK: usize = 8_192;
const MIN_ANALYZABLE_SECONDS: f64 = 0.1;

#[derive(Debug, Clone, PartialEq)]
pub struct AudioSignalMeasurements {
    pub duration_s: f64,
    pub sample_rate_hz: u32,
    pub rms_dbfs: f64,
    pub peak_dbfs: f64,
    pub level_spread_db: f64,
    pub activity_ratio: f64,
    pub zero_crossing_rate: f64,
    pub high_frequency_ratio: f64,
    pub onset_rate_hz: f64,
    pub tempo_bpm: Option<f64>,
    pub tempo_confidence: f64,
}

impl AudioSignalMeasurements {
    #[must_use]
    pub fn as_json(&self) -> Map<String, Value> {
        match json!({
            "schema": LOCAL_AUDIO_ANALYZER_ID,
            "duration_s": round_to(self.duration_s, 3),
            "sample_rate_hz": self.sample_rate_hz,
            "rms_dbfs": round_to(self.rms_dbfs, 3),
            "peak_dbfs": round_to(self.peak_dbfs, 3),
            "level_spread_db": round_to(self.level_spread_db, 3),
            "activity_ratio": round_to(self.activity_ratio, 6),
            "zero_crossing_rate": round_to(self.zero_crossing_rate, 6),
            "high_frequency_ratio": round_to(self.high_frequency_ratio, 6),
            "onset_rate_hz": round_to(self.onset_rate_hz, 6),
            "tempo_bpm": self.tempo_bpm.map(|value| round_to(value, 3)),
            "tempo_confidence": round_to(self.tempo_confidence, 6),
        }) {
            Value::Object(object) => object,
            _ => Map::new(),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct AudioSignalProfile {
    pub energy: f64,
    pub brightness: f64,
    pub tension: f64,
    pub evidence: Vec<String>,
    pub confidence: Confidence,
    pub metrics: Map<String, Value>,
}

#[derive(Debug)]
pub enum AudioSignalError {
    MissingFile,
    Spawn(io::Error),
    Decode,
    Io(io::Error),
    TooShort,
    Cancelled,
}

impl Display for AudioSignalError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        match self {
            Self::MissingFile => formatter.write_str("audio file is missing"),
            Self::Spawn(_) => formatter.write_str("FFmpeg could not start"),
            Self::Decode => formatter.write_str("FFmpeg could not decode the audio stream"),
            Self::Io(_) => formatter.write_str("FFmpeg audio stream could not be read"),
            Self::TooShort => formatter.write_str("decoded audio is empty or too short to analyze"),
            Self::Cancelled => formatter.write_str("audio analysis was cancelled"),
        }
    }
}

impl Error for AudioSignalError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Spawn(error) | Self::Io(error) => Some(error),
            _ => None,
        }
    }
}

pub trait AudioSignalAnalyzer: std::fmt::Debug + Send + Sync {
    fn analyzer_id(&self) -> &'static str;
    fn analyze(
        &self,
        path: &Path,
        cancelled: &AtomicBool,
    ) -> Result<AudioSignalProfile, AudioSignalError>;
}

#[derive(Debug, Clone)]
pub struct FfmpegSignalAnalyzer {
    executable: PathBuf,
    sample_rate: u32,
}

impl FfmpegSignalAnalyzer {
    #[must_use]
    pub fn new(executable: impl Into<PathBuf>) -> Self {
        Self {
            executable: executable.into(),
            sample_rate: TARGET_SAMPLE_RATE,
        }
    }
}

impl AudioSignalAnalyzer for FfmpegSignalAnalyzer {
    fn analyzer_id(&self) -> &'static str {
        LOCAL_AUDIO_ANALYZER_ID
    }

    fn analyze(
        &self,
        path: &Path,
        cancelled: &AtomicBool,
    ) -> Result<AudioSignalProfile, AudioSignalError> {
        if !path.is_file() {
            return Err(AudioSignalError::MissingFile);
        }
        let mut command = Command::new(&self.executable);
        command
            .arg("-v")
            .arg("error")
            .arg("-nostdin")
            .arg("-threads")
            .arg("1")
            .arg("-i")
            .arg(path)
            .arg("-map")
            .arg("0:a:0")
            .arg("-vn")
            .arg("-ac")
            .arg("1")
            .arg("-ar")
            .arg(self.sample_rate.to_string())
            .arg("-f")
            .arg("s16le")
            .arg("-acodec")
            .arg("pcm_s16le")
            .arg("pipe:1")
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let child = command.spawn().map_err(AudioSignalError::Spawn)?;
        analyze_child(child, self.sample_rate, cancelled)
    }
}

fn analyze_child(
    mut child: Child,
    sample_rate: u32,
    cancelled: &AtomicBool,
) -> Result<AudioSignalProfile, AudioSignalError> {
    let stdout = child.stdout.take().ok_or(AudioSignalError::Decode)?;
    let stderr = child.stderr.take().ok_or(AudioSignalError::Decode)?;
    let error_thread = thread::spawn(move || drain_errors(stderr));
    let (audio_sender, audio_receiver) = sync_channel(2);
    let audio_thread = thread::spawn(move || read_audio(stdout, audio_sender));
    let mut accumulator = SignalAccumulator::new(sample_rate)?;
    let mut pending = None;
    let stream_result = loop {
        if cancelled.load(Ordering::Relaxed) {
            break Err(AudioSignalError::Cancelled);
        }
        match audio_receiver.recv_timeout(Duration::from_millis(100)) {
            Ok(AudioRead::End) => {
                if pending.is_some() {
                    break Err(AudioSignalError::Decode);
                }
                break accumulator.finish();
            }
            Ok(AudioRead::Data(bytes)) => {
                let read = bytes.len();
                let mut offset = 0;
                if let Some(low) = pending.take() {
                    accumulator.add_sample(i16::from_le_bytes([low, bytes[0]]));
                    offset = 1;
                }
                while offset + 1 < read {
                    accumulator.add_sample(i16::from_le_bytes([bytes[offset], bytes[offset + 1]]));
                    offset += 2;
                }
                if offset < read {
                    pending = Some(bytes[offset]);
                }
            }
            Ok(AudioRead::Error(error)) => break Err(AudioSignalError::Io(error)),
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => {
                break Err(AudioSignalError::Decode);
            }
        }
    };
    drop(audio_receiver);
    if stream_result.is_err() && child.try_wait().ok().flatten().is_none() {
        let _ = child.kill();
    }
    let status = child.wait().map_err(AudioSignalError::Io)?;
    let _ = audio_thread.join();
    let _ = error_thread.join();
    if matches!(&stream_result, Err(AudioSignalError::Cancelled)) {
        return Err(AudioSignalError::Cancelled);
    }
    if !status.success() {
        return Err(AudioSignalError::Decode);
    }
    stream_result.map(profile_from_measurements)
}

enum AudioRead {
    Data(Vec<u8>),
    End,
    Error(io::Error),
}

fn read_audio(mut stdout: impl Read, sender: std::sync::mpsc::SyncSender<AudioRead>) {
    let mut bytes = vec![0_u8; FRAMES_PER_CHUNK.saturating_mul(2)];
    loop {
        match stdout.read(&mut bytes) {
            Ok(0) => {
                let _ = sender.send(AudioRead::End);
                return;
            }
            Ok(read) => {
                if sender
                    .send(AudioRead::Data(bytes[..read].to_vec()))
                    .is_err()
                {
                    return;
                }
            }
            Err(error) => {
                let _ = sender.send(AudioRead::Error(error));
                return;
            }
        }
    }
}

fn drain_errors(mut stderr: impl Read) {
    let mut chunk = [0_u8; 4_096];
    loop {
        let Ok(read) = stderr.read(&mut chunk) else {
            return;
        };
        if read == 0 {
            return;
        }
    }
}

#[derive(Debug)]
struct SignalAccumulator {
    sample_rate: u32,
    window_size: usize,
    total_samples: u64,
    sum_squares: f64,
    sum_difference_squares: f64,
    peak: f64,
    zero_crossings: u64,
    previous: Option<f64>,
    window_samples: usize,
    window_sum_squares: f64,
    window_levels: Vec<f64>,
}

impl SignalAccumulator {
    fn new(sample_rate: u32) -> Result<Self, AudioSignalError> {
        if sample_rate == 0 {
            return Err(AudioSignalError::Decode);
        }
        let window_size = usize::try_from(sample_rate / 20)
            .unwrap_or(usize::MAX)
            .max(64);
        Ok(Self {
            sample_rate,
            window_size,
            total_samples: 0,
            sum_squares: 0.0,
            sum_difference_squares: 0.0,
            peak: 0.0,
            zero_crossings: 0,
            previous: None,
            window_samples: 0,
            window_sum_squares: 0.0,
            window_levels: Vec::new(),
        })
    }

    fn add_sample(&mut self, raw: i16) {
        let sample = clamp(f64::from(raw) / 32_768.0, -1.0, 1.0);
        let squared = sample * sample;
        self.total_samples = self.total_samples.saturating_add(1);
        self.sum_squares += squared;
        self.peak = self.peak.max(sample.abs());
        if let Some(previous) = self.previous {
            let difference = sample - previous;
            self.sum_difference_squares += difference * difference;
            if (sample < 0.0 && previous >= 0.0) || (previous < 0.0 && sample >= 0.0) {
                self.zero_crossings = self.zero_crossings.saturating_add(1);
            }
        }
        self.previous = Some(sample);
        self.window_samples = self.window_samples.saturating_add(1);
        self.window_sum_squares += squared;
        if self.window_samples >= self.window_size {
            self.finish_window();
        }
    }

    fn finish_window(&mut self) {
        if self.window_samples > 0 {
            self.window_levels
                .push((self.window_sum_squares / self.window_samples as f64).sqrt());
        }
        self.window_samples = 0;
        self.window_sum_squares = 0.0;
    }

    fn finish(mut self) -> Result<AudioSignalMeasurements, AudioSignalError> {
        self.finish_window();
        let duration_s = self.total_samples as f64 / f64::from(self.sample_rate);
        if duration_s < MIN_ANALYZABLE_SECONDS || self.total_samples < 2 {
            return Err(AudioSignalError::TooShort);
        }
        let rms = (self.sum_squares / self.total_samples as f64).sqrt();
        let activity_floor = 10_f64.powf(-50.0 / 20.0).max(rms * 0.1);
        let active_levels = self
            .window_levels
            .iter()
            .copied()
            .filter(|level| *level >= activity_floor)
            .collect::<Vec<_>>();
        let activity_ratio = if self.window_levels.is_empty() {
            0.0
        } else {
            active_levels.len() as f64 / self.window_levels.len() as f64
        };
        let mut active_db = active_levels.into_iter().map(dbfs).collect::<Vec<_>>();
        active_db.sort_by(f64::total_cmp);
        let level_spread_db = (quantile(&active_db, 0.9) - quantile(&active_db, 0.1)).max(0.0);
        let level_deltas = self
            .window_levels
            .windows(2)
            .map(|values| (values[1] - values[0]).max(0.0))
            .collect::<Vec<_>>();
        let delta_median = median(&level_deltas);
        let delta_deviations = level_deltas
            .iter()
            .map(|value| (value - delta_median).abs())
            .collect::<Vec<_>>();
        let onset_threshold = 0.002_f64.max(delta_median + 3.0 * median(&delta_deviations));
        let onset_count = level_deltas
            .iter()
            .filter(|value| **value > onset_threshold)
            .count();
        let onset_rate_hz = onset_count as f64 / duration_s;
        let windows_per_second = f64::from(self.sample_rate) / self.window_size as f64;
        let (tempo_bpm, tempo_confidence) =
            estimate_tempo(&self.window_levels, windows_per_second, duration_s);
        let zero_crossing_rate =
            self.zero_crossings as f64 / self.total_samples.saturating_sub(1).max(1) as f64;
        let high_frequency_ratio =
            (self.sum_difference_squares / (4.0 * self.sum_squares).max(1e-12)).sqrt();
        Ok(AudioSignalMeasurements {
            duration_s,
            sample_rate_hz: self.sample_rate,
            rms_dbfs: dbfs(rms),
            peak_dbfs: dbfs(self.peak),
            level_spread_db,
            activity_ratio,
            zero_crossing_rate: clamp(zero_crossing_rate, 0.0, 1.0),
            high_frequency_ratio: clamp(high_frequency_ratio, 0.0, 1.0),
            onset_rate_hz,
            tempo_bpm,
            tempo_confidence,
        })
    }
}

fn estimate_tempo(levels: &[f64], windows_per_second: f64, duration_s: f64) -> (Option<f64>, f64) {
    if duration_s < 15.0 || levels.len() < 16 {
        return (None, 0.0);
    }
    let mut envelope = levels
        .windows(2)
        .map(|values| (values[1] - values[0]).max(0.0))
        .collect::<Vec<_>>();
    if envelope.is_empty() {
        return (None, 0.0);
    }
    let floor = median(&envelope);
    for value in &mut envelope {
        *value = (*value - floor).max(0.0);
    }
    let energy = envelope.iter().map(|value| value * value).sum::<f64>();
    if energy <= 1e-9 {
        return (None, 0.0);
    }
    let min_lag = ((windows_per_second * 60.0 / 200.0).round() as usize).max(1);
    let max_lag = (envelope.len() / 2).min((windows_per_second * 60.0 / 40.0).round() as usize);
    if max_lag < min_lag {
        return (None, 0.0);
    }
    let mut best_lag = None;
    let mut best_score = 0.0_f64;
    for lag in min_lag..=max_lag {
        let left = &envelope[lag..];
        let right = &envelope[..envelope.len() - lag];
        let numerator = left.iter().zip(right).map(|(a, b)| a * b).sum::<f64>();
        let left_energy = left.iter().map(|value| value * value).sum::<f64>();
        let right_energy = right.iter().map(|value| value * value).sum::<f64>();
        let denominator = (left_energy * right_energy).sqrt();
        if denominator <= 1e-12 {
            continue;
        }
        let score = numerator / denominator;
        if score > best_score {
            best_score = score;
            best_lag = Some(lag);
        }
    }
    let confidence = clamp(best_score, 0.0, 1.0);
    match best_lag {
        Some(lag) if confidence >= 0.2 => {
            (Some(60.0 * windows_per_second / lag as f64), confidence)
        }
        _ => (None, confidence),
    }
}

fn profile_from_measurements(measurements: AudioSignalMeasurements) -> AudioSignalProfile {
    let energy =
        0.75 * normalize(measurements.rms_dbfs, -36.0, -9.0) + 0.25 * measurements.activity_ratio;
    let brightness = 0.65 * normalize(measurements.high_frequency_ratio, 0.03, 0.45)
        + 0.35 * normalize(measurements.zero_crossing_rate, 0.01, 0.20);
    let transient_activity = normalize(measurements.onset_rate_hz, 0.2, 4.0);
    let tension = measurements.tempo_bpm.map_or_else(
        || 0.65 * transient_activity + 0.35 * brightness,
        |tempo| {
            0.50 * transient_activity
                + 0.25 * brightness
                + 0.25 * normalize(tempo, 60.0, 180.0) * measurements.tempo_confidence
        },
    );
    let confidence = if measurements.duration_s >= 45.0 && measurements.activity_ratio >= 0.5 {
        Confidence::High
    } else if measurements.duration_s >= 10.0 && measurements.activity_ratio >= 0.2 {
        Confidence::Medium
    } else {
        Confidence::Low
    };
    let tempo_evidence = measurements.tempo_bpm.map_or_else(
        || "No stable tempo estimate was found".to_owned(),
        |tempo| {
            format!(
                "Tempo estimate {tempo:.1} BPM ({:.0}% periodicity confidence)",
                measurements.tempo_confidence * 100.0
            )
        },
    );
    let evidence = vec![
        format!(
            "Signal level: {:.1} dBFS RMS, {:.1} dBFS peak",
            measurements.rms_dbfs, measurements.peak_dbfs
        ),
        format!(
            "Short-window level spread: {:.1} dB",
            measurements.level_spread_db
        ),
        format!(
            "High-frequency proxy: {:.3}; zero-crossing rate: {:.3}",
            measurements.high_frequency_ratio, measurements.zero_crossing_rate
        ),
        format!(
            "Transient onset rate: {:.2} per second",
            measurements.onset_rate_hz
        ),
        tempo_evidence,
        "Signal proxies do not identify instruments, genre, setting, period, scene, or mood."
            .to_owned(),
    ];
    AudioSignalProfile {
        energy: round_to(clamp(energy, 0.0, 1.0), 6),
        brightness: round_to(clamp(brightness, 0.0, 1.0), 6),
        tension: round_to(clamp(tension, 0.0, 1.0), 6),
        evidence,
        confidence,
        metrics: measurements.as_json(),
    }
}

fn clamp(value: f64, minimum: f64, maximum: f64) -> f64 {
    value.max(minimum).min(maximum)
}

fn normalize(value: f64, low: f64, high: f64) -> f64 {
    clamp((value - low) / (high - low), 0.0, 1.0)
}

fn dbfs(amplitude: f64) -> f64 {
    20.0 * amplitude.max(1e-6).log10()
}

fn quantile(sorted: &[f64], fraction: f64) -> f64 {
    if sorted.is_empty() {
        return 0.0;
    }
    let position = clamp(fraction, 0.0, 1.0) * sorted.len().saturating_sub(1) as f64;
    let lower = position.floor() as usize;
    let upper = position.ceil() as usize;
    if lower == upper {
        sorted[lower]
    } else {
        let weight = position - lower as f64;
        sorted[lower] * (1.0 - weight) + sorted[upper] * weight
    }
}

fn median(values: &[f64]) -> f64 {
    let mut sorted = values.to_vec();
    sorted.sort_by(f64::total_cmp);
    quantile(&sorted, 0.5)
}

fn round_to(value: f64, digits: i32) -> f64 {
    let factor = 10_f64.powi(digits);
    (value * factor).round() / factor
}

#[cfg(test)]
mod tests {
    use super::{SignalAccumulator, profile_from_measurements};

    #[test]
    fn synthetic_signal_profile_is_bounded_and_versioned() -> Result<(), Box<dyn std::error::Error>>
    {
        let mut accumulator = SignalAccumulator::new(8_000)?;
        for index in 0..160_000_u32 {
            let phase = f64::from(index) * std::f64::consts::TAU * 440.0 / 8_000.0;
            let pulse = if index % 4_000 < 1_000 { 1.0 } else { 0.25 };
            let sample = (phase.sin() * pulse * 20_000.0).round() as i16;
            accumulator.add_sample(sample);
        }
        let profile = profile_from_measurements(accumulator.finish()?);
        assert_eq!(profile.metrics["schema"], "local-audio/v1");
        assert_eq!(profile.energy, 0.900_752);
        assert_eq!(profile.brightness, 0.403_852);
        assert_eq!(profile.tension, 0.456_226);
        assert_eq!(profile.metrics["duration_s"], 20.0);
        assert_eq!(profile.metrics["rms_dbfs"], -12.573);
        assert_eq!(profile.metrics["level_spread_db"], 12.041);
        assert_eq!(profile.metrics["high_frequency_ratio"], 0.171_929);
        assert_eq!(profile.metrics["zero_crossing_rate"], 0.109_994);
        assert_eq!(profile.metrics["onset_rate_hz"], 1.95);
        assert_eq!(profile.metrics["tempo_bpm"], 120.0);
        assert_eq!(profile.metrics["tempo_confidence"], 1.0);
        assert_eq!(profile.evidence.len(), 6);
        Ok(())
    }
}
