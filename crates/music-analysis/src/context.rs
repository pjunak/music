use std::cmp::Ordering as CmpOrdering;
use std::error::Error;
use std::ffi::OsStr;
use std::fmt::{self, Display, Formatter};
use std::io::{self, Read};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{RecvTimeoutError, sync_channel};
use std::thread;
use std::time::{Duration, Instant};

use music_application::assistant::{LOCAL_CONTEXT_ANALYZER_ID, LOCAL_CONTEXT_IMPLEMENTATION_ID};
use rustfft::num_complex::Complex;
use rustfft::{Fft, FftPlanner};
use serde_json::{Map, Value, json};

const CONTEXT_SAMPLE_RATE: u32 = 16_000;
const CONTEXT_FRAME_SECONDS: f64 = 0.5;
const CONTEXT_TIMELINE_SECONDS: f64 = 2.0;
const FFT_SIZE: usize = 2_048;
const SPECTRAL_BANDS: usize = 24;
const MAX_SECTIONS: usize = 10;
const MIN_SECTION_SECONDS: f64 = 10.0;
const MAX_AUDIO_SECONDS: u64 = 24 * 60 * 60;
const FRAMES_PER_CHUNK: usize = 8_192;
const CAPTURE_LIMIT: usize = 64 * 1_024;
const PROBE_TIMEOUT: Duration = Duration::from_secs(30);
const LOUDNESS_TIMEOUT: Duration = Duration::from_secs(30 * 60);

#[derive(Debug, Clone, PartialEq)]
pub struct AudioContextPerformance {
    pub audio_seconds: f64,
    pub elapsed_seconds: f64,
    pub stage_seconds: Map<String, Value>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct AudioContextDocument {
    pub confidence: &'static str,
    pub completeness: &'static str,
    pub summary: Map<String, Value>,
    pub timeline: Vec<Map<String, Value>>,
    pub sections: Vec<Map<String, Value>>,
    pub technical: Map<String, Value>,
    pub stages: Map<String, Value>,
    pub performance: AudioContextPerformance,
}

#[derive(Debug, Clone, Eq, PartialEq)]
pub enum VoiceContextPreparation {
    NotConfigured,
    Deferred,
    Unavailable { reason: String },
}

#[derive(Debug)]
pub enum AudioContextError {
    MissingFile,
    Spawn(io::Error),
    Decode,
    Io(io::Error),
    TooShort,
    TooLong,
    Cancelled,
}

impl Display for AudioContextError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::MissingFile => "audio file is missing",
            Self::Spawn(_) => "audio analysis process could not start",
            Self::Decode => "FFmpeg could not decode the audio stream",
            Self::Io(_) => "audio analysis process could not be read",
            Self::TooShort => "decoded audio is empty or too short to analyze",
            Self::TooLong => "decoded audio exceeds the 24-hour analysis limit",
            Self::Cancelled => "audio context analysis was cancelled",
        })
    }
}

impl Error for AudioContextError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Spawn(error) | Self::Io(error) => Some(error),
            _ => None,
        }
    }
}

pub trait AudioContextAnalyzer: std::fmt::Debug + Send + Sync {
    fn analyzer_id(&self) -> &'static str;
    fn implementation_id(&self) -> &'static str;
    fn analyze(
        &self,
        path: &Path,
        cancelled: &AtomicBool,
        voice: VoiceContextPreparation,
    ) -> Result<AudioContextDocument, AudioContextError>;
}

#[derive(Debug, Clone)]
pub struct FfmpegContextAnalyzer {
    ffmpeg: PathBuf,
    ffprobe: PathBuf,
}

impl FfmpegContextAnalyzer {
    #[must_use]
    pub fn new(ffmpeg: impl Into<PathBuf>, ffprobe: impl Into<PathBuf>) -> Self {
        Self {
            ffmpeg: ffmpeg.into(),
            ffprobe: ffprobe.into(),
        }
    }
}

impl AudioContextAnalyzer for FfmpegContextAnalyzer {
    fn analyzer_id(&self) -> &'static str {
        LOCAL_CONTEXT_ANALYZER_ID
    }

    fn implementation_id(&self) -> &'static str {
        LOCAL_CONTEXT_IMPLEMENTATION_ID
    }

    fn analyze(
        &self,
        path: &Path,
        cancelled: &AtomicBool,
        voice: VoiceContextPreparation,
    ) -> Result<AudioContextDocument, AudioContextError> {
        if !path.is_file() {
            return Err(AudioContextError::MissingFile);
        }
        let analysis_started = Instant::now();
        let probe_started = Instant::now();
        let mut technical = technical_probe(&self.ffprobe, path, cancelled)?;
        let mut stage_seconds = Map::from_iter([(
            "probe".to_owned(),
            json!(probe_started.elapsed().as_secs_f64()),
        )]);

        let decode_started = Instant::now();
        let (frames, short_levels, global, spectrum_seconds) =
            decode_context(&self.ffmpeg, path, cancelled)?;
        let decode_seconds = decode_started.elapsed().as_secs_f64();
        stage_seconds.insert("spectrum".to_owned(), json!(spectrum_seconds));
        stage_seconds.insert(
            "decode_and_frames".to_owned(),
            json!((decode_seconds - spectrum_seconds).max(0.0)),
        );

        let feature_started = Instant::now();
        let rows = timeline_frames(&frames, &short_levels);
        let tempo_points = tempo_curve(&short_levels, global.duration_s);
        let boundaries = change_boundaries(&rows);
        let sections = section_summary(&rows, &boundaries, &tempo_points, global.duration_s);
        let timeline = downsample_timeline(&rows);
        let trajectories = Map::from_iter(
            TimelineMetric::TRAJECTORIES
                .into_iter()
                .map(|metric| (metric.name().to_owned(), trajectory(&metric.values(&rows)))),
        );
        let tempo_bpms = tempo_points
            .iter()
            .filter(|point| point.confidence >= 0.25)
            .map(|point| point.bpm)
            .collect::<Vec<_>>();
        let tempo_summary = tempo_summary(&tempo_points, &tempo_bpms);
        stage_seconds.insert(
            "feature_summary".to_owned(),
            json!(feature_started.elapsed().as_secs_f64()),
        );

        let voice_started = Instant::now();
        let (voice_summary, voice_stage, voice_reliability, completeness) =
            voice_placeholders(&voice);
        if !matches!(voice, VoiceContextPreparation::Deferred) {
            stage_seconds.insert(
                "voice".to_owned(),
                json!(voice_started.elapsed().as_secs_f64()),
            );
        }

        let loudness_started = Instant::now();
        let loudness = ebu_loudness(&self.ffmpeg, path, cancelled)?;
        stage_seconds.insert(
            "ebu_loudness".to_owned(),
            json!(loudness_started.elapsed().as_secs_f64()),
        );
        check_cancelled(cancelled)?;

        let finalize_started = Instant::now();
        technical.insert(
            "loudness".to_owned(),
            loudness.map_or_else(
                || {
                    json!({
                        "status": "dbfs_proxy",
                        "rms_dbfs": round_to(global.rms_dbfs, 3),
                        "peak_dbfs": round_to(global.peak_dbfs, 3),
                    })
                },
                |value| {
                    json!({
                        "status": "ebu_r128",
                        "integrated_lufs": value.integrated_lufs,
                        "loudness_range_lu": value.loudness_range_lu,
                        "true_peak_dbtp": value.true_peak_dbtp,
                        "relative_threshold_lufs": value.relative_threshold_lufs,
                    })
                },
            ),
        );
        technical.insert(
            "decoded_sample_rate_hz".to_owned(),
            json!(CONTEXT_SAMPLE_RATE),
        );
        technical.insert(
            "duration_s".to_owned(),
            json!(round_to(global.duration_s, 3)),
        );

        let active_fraction =
            rows.iter().filter(|row| row.loudness > 0.08).count() as f64 / rows.len().max(1) as f64;
        let confidence = if global.duration_s >= 30.0 && active_fraction >= 0.25 {
            "high"
        } else if global.duration_s >= 5.0 {
            "medium"
        } else {
            "low"
        };
        let repeated_sections = sections
            .iter()
            .filter(|section| {
                section
                    .get("repeats_section_ids")
                    .and_then(Value::as_array)
                    .is_some_and(|values| !values.is_empty())
            })
            .count();
        let structure = json!({
            "section_count": sections.len(),
            "major_change_count": sections.len().saturating_sub(1),
            "repeated_section_count": repeated_sections,
            "development": if repeated_sections >= 2 {
                "repetitive"
            } else if sections.len() >= 3 {
                "sectional"
            } else {
                "continuous"
            },
        });
        let intensity = trajectories
            .get("intensity")
            .and_then(Value::as_object)
            .cloned()
            .unwrap_or_default();
        let rhythmic = trajectories
            .get("rhythmic_drive")
            .and_then(Value::as_object)
            .cloned()
            .unwrap_or_default();
        let evidence = vec![
            trajectory_evidence("Intensity", &intensity),
            trajectory_evidence("Rhythmic drive", &rhythmic),
            tempo_evidence(&tempo_summary),
            format!(
                "{} acoustic section{} with {} major transition{}.",
                sections.len(),
                if sections.len() == 1 { "" } else { "s" },
                sections.len().saturating_sub(1),
                if sections.len() == 2 { "" } else { "s" },
            ),
        ];
        let summary = object(json!({
            "schema_version": LOCAL_CONTEXT_ANALYZER_ID,
            "duration_s": round_to(global.duration_s, 3),
            "confidence": confidence,
            "trajectories": trajectories,
            "tempo": tempo_summary,
            "structure": structure,
            "voice": voice_summary,
            "measurement_reliability": {
                "loudness": "medium",
                "intensity": "medium",
                "rhythmic_drive": "medium",
                "brightness": "medium",
                "density": "medium",
                "spectral_flux": "medium",
                "tempo": if tempo_bpms.is_empty() { "low" } else { "medium" },
                "structure": if global.duration_s >= 30.0 { "medium" } else { "low" },
                "voice": voice_reliability,
            },
            "evidence": evidence,
        }));
        let stages = object(json!({
            "decode": {"status": "complete"},
            "signal": {"status": "complete", "frame_seconds": CONTEXT_FRAME_SECONDS},
            "spectrum": {
                "status": "complete",
                "fft_size": FFT_SIZE,
                "bands": SPECTRAL_BANDS,
                "implementation": "rustfft+mel-profile/v2",
            },
            "tempo": {"status": if tempo_bpms.is_empty() { "unresolved" } else { "measured" }},
            "structure": {"status": "complete"},
            "loudness": {"status": if loudness.is_some() { "complete" } else { "proxy" }},
            "voice": voice_stage,
        }));
        stage_seconds.insert(
            "finalize".to_owned(),
            json!(finalize_started.elapsed().as_secs_f64()),
        );
        let elapsed_seconds = analysis_started.elapsed().as_secs_f64();
        Ok(AudioContextDocument {
            confidence,
            completeness,
            summary,
            timeline,
            sections,
            technical,
            stages,
            performance: AudioContextPerformance {
                audio_seconds: global.duration_s,
                elapsed_seconds,
                stage_seconds,
            },
        })
    }
}

#[derive(Debug, Clone)]
struct Spectrum {
    centroid_hz: f64,
    bandwidth_hz: f64,
    rolloff_hz: f64,
    flatness: f64,
    bass_ratio: f64,
    mid_ratio: f64,
    high_ratio: f64,
    peak_concentration: f64,
    spectral_entropy: f64,
    band_coverage: f64,
    profile: [f64; SPECTRAL_BANDS],
}

impl Spectrum {
    const fn silent() -> Self {
        Self {
            centroid_hz: 0.0,
            bandwidth_hz: 0.0,
            rolloff_hz: 0.0,
            flatness: 0.0,
            bass_ratio: 0.0,
            mid_ratio: 0.0,
            high_ratio: 0.0,
            peak_concentration: 0.0,
            spectral_entropy: 0.0,
            band_coverage: 0.0,
            profile: [0.0; SPECTRAL_BANDS],
        }
    }
}

#[derive(Debug, Clone)]
struct Frame {
    start_s: f64,
    duration_s: f64,
    loudness_dbfs: f64,
    spectrum: Spectrum,
}

#[derive(Debug, Clone, Copy)]
struct GlobalMetrics {
    duration_s: f64,
    rms_dbfs: f64,
    peak_dbfs: f64,
}

struct ContextAccumulator {
    sample_rate: u32,
    frame_size: usize,
    short_size: usize,
    maximum_samples: u64,
    total_samples: u64,
    total_squares: f64,
    peak: f64,
    framed_samples: u64,
    frame: Vec<f64>,
    short_samples: usize,
    short_squares: f64,
    short_levels: Vec<f64>,
    frames: Vec<Frame>,
    fft: Arc<dyn Fft<f64>>,
    fft_buffer: Vec<Complex<f64>>,
    fft_scratch: Vec<Complex<f64>>,
    hann: Vec<f64>,
    mel_bins: [Option<usize>; FFT_SIZE / 2],
    spectrum_seconds: f64,
}

impl std::fmt::Debug for ContextAccumulator {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ContextAccumulator")
            .field("sample_rate", &self.sample_rate)
            .field("total_samples", &self.total_samples)
            .field("frames", &self.frames.len())
            .finish_non_exhaustive()
    }
}

impl ContextAccumulator {
    fn new(sample_rate: u32) -> Result<Self, AudioContextError> {
        if sample_rate == 0 {
            return Err(AudioContextError::Decode);
        }
        let frame_size = usize::try_from(sample_rate / 2).map_err(|_| AudioContextError::Decode)?;
        let short_size =
            usize::try_from(sample_rate / 20).map_err(|_| AudioContextError::Decode)?;
        let mut planner = FftPlanner::<f64>::new();
        let fft = planner.plan_fft_forward(FFT_SIZE);
        let scratch_len = fft.get_inplace_scratch_len();
        Ok(Self {
            sample_rate,
            frame_size: frame_size.max(256),
            short_size: short_size.max(64),
            maximum_samples: u64::from(sample_rate).saturating_mul(MAX_AUDIO_SECONDS),
            total_samples: 0,
            total_squares: 0.0,
            peak: 0.0,
            framed_samples: 0,
            frame: Vec::with_capacity(frame_size),
            short_samples: 0,
            short_squares: 0.0,
            short_levels: Vec::new(),
            frames: Vec::new(),
            fft,
            fft_buffer: vec![Complex::new(0.0, 0.0); FFT_SIZE],
            fft_scratch: vec![Complex::new(0.0, 0.0); scratch_len],
            hann: (0..FFT_SIZE)
                .map(|index| {
                    0.5 - 0.5 * (std::f64::consts::TAU * index as f64 / (FFT_SIZE - 1) as f64).cos()
                })
                .collect(),
            mel_bins: spectral_band_bins(sample_rate),
            spectrum_seconds: 0.0,
        })
    }

    fn add_sample(&mut self, raw: i16) -> Result<(), AudioContextError> {
        if self.total_samples >= self.maximum_samples {
            return Err(AudioContextError::TooLong);
        }
        let sample = clamp(f64::from(raw) / 32_768.0, -1.0, 1.0);
        let squared = sample * sample;
        self.total_samples = self.total_samples.saturating_add(1);
        self.total_squares += squared;
        self.peak = self.peak.max(sample.abs());
        self.short_samples = self.short_samples.saturating_add(1);
        self.short_squares += squared;
        if self.short_samples == self.short_size {
            self.finish_short();
        }
        self.frame.push(sample);
        if self.frame.len() == self.frame_size {
            self.finish_frame();
        }
        Ok(())
    }

    fn finish_short(&mut self) {
        if self.short_samples > 0 {
            self.short_levels
                .push((self.short_squares / self.short_samples as f64).sqrt());
        }
        self.short_samples = 0;
        self.short_squares = 0.0;
    }

    fn finish_frame(&mut self) {
        if self.frame.is_empty() {
            return;
        }
        let squares = self.frame.iter().map(|value| value * value).sum::<f64>();
        let rms = (squares / self.frame.len() as f64).sqrt();
        let duration_s = self.frame.len() as f64 / f64::from(self.sample_rate);
        let start_s = self.framed_samples as f64 / f64::from(self.sample_rate);
        let spectrum_started = Instant::now();
        let spectrum = self.spectrum();
        self.spectrum_seconds += spectrum_started.elapsed().as_secs_f64();
        self.frames.push(Frame {
            start_s,
            duration_s,
            loudness_dbfs: dbfs(rms),
            spectrum,
        });
        self.framed_samples = self
            .framed_samples
            .saturating_add(u64::try_from(self.frame.len()).unwrap_or(u64::MAX));
        self.frame.clear();
    }

    fn spectrum(&mut self) -> Spectrum {
        self.fft_buffer.fill(Complex::new(0.0, 0.0));
        let start = self.frame.len().saturating_sub(FFT_SIZE) / 2;
        let selected = &self.frame[start..self.frame.len().min(start.saturating_add(FFT_SIZE))];
        for (index, sample) in selected.iter().enumerate() {
            self.fft_buffer[index].re = *sample * self.hann[index];
        }
        self.fft
            .process_with_scratch(&mut self.fft_buffer, &mut self.fft_scratch);
        let powers = self.fft_buffer[..FFT_SIZE / 2]
            .iter()
            .map(|value| value.norm_sqr().max(1e-18))
            .collect::<Vec<_>>();
        let total = powers.iter().sum::<f64>();
        if total <= 1e-12 {
            return Spectrum::silent();
        }
        let bin_hz = f64::from(self.sample_rate) / FFT_SIZE as f64;
        let magnitudes = powers.iter().map(|value| value.sqrt()).collect::<Vec<_>>();
        let magnitude_total = magnitudes.iter().sum::<f64>();
        let centroid_hz = magnitudes
            .iter()
            .enumerate()
            .map(|(index, value)| index as f64 * bin_hz * value)
            .sum::<f64>()
            / magnitude_total;
        let variance = magnitudes
            .iter()
            .enumerate()
            .map(|(index, value)| {
                let delta = index as f64 * bin_hz - centroid_hz;
                delta * delta * value
            })
            .sum::<f64>()
            / magnitude_total;
        let threshold = magnitude_total * 0.85;
        let mut cumulative = 0.0;
        let mut rolloff_index = powers.len().saturating_sub(1);
        for (index, magnitude) in magnitudes.iter().enumerate() {
            cumulative += magnitude;
            if cumulative >= threshold {
                rolloff_index = index;
                break;
            }
        }
        let geometric =
            (powers.iter().map(|value| value.ln()).sum::<f64>() / powers.len() as f64).exp();
        let flatness = clamp(geometric / (total / powers.len() as f64), 0.0, 1.0);
        let band = |low: f64, high: f64| {
            powers
                .iter()
                .enumerate()
                .filter(|(index, _)| {
                    let frequency = *index as f64 * bin_hz;
                    frequency >= low && frequency < high
                })
                .map(|(_, value)| *value)
                .sum::<f64>()
                / total
        };
        let strongest_count = (powers.len() / 100).max(4);
        let mut ordered_powers = powers.clone();
        ordered_powers.sort_by(f64::total_cmp);
        let peak_concentration = clamp(
            ordered_powers[ordered_powers.len() - strongest_count..]
                .iter()
                .sum::<f64>()
                / total,
            0.0,
            1.0,
        );
        let (profile, spectral_entropy, band_coverage) =
            spectral_profile(&powers, total, &self.mel_bins);
        Spectrum {
            centroid_hz,
            bandwidth_hz: variance.max(0.0).sqrt(),
            rolloff_hz: rolloff_index as f64 * bin_hz,
            flatness,
            bass_ratio: band(20.0, 250.0),
            mid_ratio: band(250.0, 2_000.0),
            high_ratio: band(2_000.0, f64::from(self.sample_rate) / 2.0),
            peak_concentration,
            spectral_entropy,
            band_coverage,
            profile,
        }
    }

    fn finish(mut self) -> Result<(Vec<Frame>, Vec<f64>, GlobalMetrics, f64), AudioContextError> {
        self.finish_short();
        self.finish_frame();
        let duration_s = self.total_samples as f64 / f64::from(self.sample_rate);
        if duration_s < 0.1 || self.frames.is_empty() {
            return Err(AudioContextError::TooShort);
        }
        let metrics = GlobalMetrics {
            duration_s,
            rms_dbfs: dbfs((self.total_squares / self.total_samples as f64).sqrt()),
            peak_dbfs: dbfs(self.peak),
        };
        Ok((
            self.frames,
            self.short_levels,
            metrics,
            self.spectrum_seconds,
        ))
    }
}

fn spectral_band_bins(sample_rate: u32) -> [Option<usize>; FFT_SIZE / 2] {
    let highest = (f64::from(sample_rate) / 2.0).min(8_000.0);
    let low_mel = hz_mel(40.0);
    let high_mel = hz_mel(highest);
    let edges = (0..=SPECTRAL_BANDS)
        .map(|index| mel_hz(low_mel + (high_mel - low_mel) * index as f64 / SPECTRAL_BANDS as f64))
        .collect::<Vec<_>>();
    std::array::from_fn(|index| {
        let frequency = index as f64 * f64::from(sample_rate) / FFT_SIZE as f64;
        let insertion = edges.partition_point(|edge| *edge <= frequency);
        let bin = insertion.checked_sub(1)?;
        (bin < SPECTRAL_BANDS).then_some(bin)
    })
}

fn spectral_profile(
    powers: &[f64],
    total: f64,
    bins: &[Option<usize>; FFT_SIZE / 2],
) -> ([f64; SPECTRAL_BANDS], f64, f64) {
    let mut band_energy = [0.0; SPECTRAL_BANDS];
    for (power, bin) in powers.iter().zip(bins) {
        if let Some(bin) = bin {
            band_energy[*bin] += power;
        }
    }
    let covered_total = band_energy.iter().sum::<f64>();
    if covered_total <= 1e-18_f64.max(total * 1e-12) {
        return ([0.0; SPECTRAL_BANDS], 0.0, 0.0);
    }
    let ratios = band_energy.map(|value| value / covered_total);
    let positive = ratios.iter().filter(|value| **value > 1e-12);
    let positive_count = positive.clone().count();
    let entropy = if positive_count > 1 {
        -positive.map(|value| value * value.ln()).sum::<f64>() / (SPECTRAL_BANDS as f64).ln()
    } else {
        0.0
    };
    let coverage =
        ratios.iter().filter(|value| **value >= 0.01).count() as f64 / SPECTRAL_BANDS as f64;
    let compressed = ratios.map(|value| (value * 1_000.0).ln_1p());
    let compressed_total = compressed.iter().sum::<f64>();
    let profile = if compressed_total > 1e-12 {
        compressed.map(|value| value / compressed_total)
    } else {
        compressed
    };
    (profile, clamp(entropy, 0.0, 1.0), clamp(coverage, 0.0, 1.0))
}

fn decode_context(
    executable: &Path,
    path: &Path,
    cancelled: &AtomicBool,
) -> Result<(Vec<Frame>, Vec<f64>, GlobalMetrics, f64), AudioContextError> {
    let mut command = Command::new(executable);
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
        .arg(CONTEXT_SAMPLE_RATE.to_string())
        .arg("-f")
        .arg("s16le")
        .arg("-acodec")
        .arg("pcm_s16le")
        .arg("pipe:1")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = command.spawn().map_err(AudioContextError::Spawn)?;
    let stdout = child.stdout.take().ok_or(AudioContextError::Decode)?;
    let stderr = child.stderr.take().ok_or(AudioContextError::Decode)?;
    let error_thread = thread::spawn(move || drain(stderr));
    let (sender, receiver) = sync_channel(2);
    let audio_thread = thread::spawn(move || read_audio(stdout, sender));
    let mut accumulator = ContextAccumulator::new(CONTEXT_SAMPLE_RATE)?;
    let mut pending = None;
    let stream_result = 'stream: loop {
        if cancelled.load(Ordering::Relaxed) {
            break Err(AudioContextError::Cancelled);
        }
        match receiver.recv_timeout(Duration::from_millis(100)) {
            Ok(AudioRead::End) => {
                if pending.is_some() {
                    break Err(AudioContextError::Decode);
                }
                break accumulator.finish();
            }
            Ok(AudioRead::Data(bytes)) => {
                let mut offset = 0;
                if let Some(low) = pending.take() {
                    if let Err(error) = accumulator.add_sample(i16::from_le_bytes([low, bytes[0]]))
                    {
                        break 'stream Err(error);
                    }
                    offset = 1;
                }
                while offset + 1 < bytes.len() {
                    if let Err(error) = accumulator
                        .add_sample(i16::from_le_bytes([bytes[offset], bytes[offset + 1]]))
                    {
                        break 'stream Err(error);
                    }
                    offset += 2;
                }
                if offset < bytes.len() {
                    pending = Some(bytes[offset]);
                }
            }
            Ok(AudioRead::Error(error)) => break Err(AudioContextError::Io(error)),
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => break Err(AudioContextError::Decode),
        }
    };
    drop(receiver);
    if stream_result.is_err() && child.try_wait().ok().flatten().is_none() {
        let _ = child.kill();
    }
    let status = child.wait().map_err(AudioContextError::Io)?;
    let _ = audio_thread.join();
    let _ = error_thread.join();
    if matches!(&stream_result, Err(AudioContextError::Cancelled)) {
        return Err(AudioContextError::Cancelled);
    }
    if !status.success() {
        return Err(AudioContextError::Decode);
    }
    stream_result
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

fn drain(mut reader: impl Read) {
    let mut bytes = [0_u8; 4_096];
    loop {
        match reader.read(&mut bytes) {
            Ok(0) | Err(_) => return,
            Ok(_) => {}
        }
    }
}

fn onset_strengths(levels: &[f64]) -> Vec<f64> {
    let rises = levels
        .windows(2)
        .map(|pair| (dbfs(pair[1]) - dbfs(pair[0])).max(0.0))
        .collect::<Vec<_>>();
    if rises.is_empty() {
        return Vec::new();
    }
    let floor = median(&rises);
    let deviations = rises
        .iter()
        .map(|value| (value - floor).abs())
        .collect::<Vec<_>>();
    let threshold = 1.5_f64.max(floor + 2.5 * median(&deviations));
    rises
        .into_iter()
        .map(|value| (value - threshold).max(0.0))
        .collect()
}

fn estimate_tempo(levels: &[f64], windows_per_second: f64) -> (Option<f64>, f64) {
    if levels.len() < python_round_usize(windows_per_second * 12.0) {
        return (None, 0.0);
    }
    let envelope = onset_strengths(levels);
    if envelope.iter().map(|value| value * value).sum::<f64>() <= 1e-10 {
        return (None, 0.0);
    }
    let min_lag = python_round_usize(windows_per_second * 60.0 / 200.0).max(1);
    let max_lag = (envelope.len() / 2).min(python_round_usize(windows_per_second * 60.0 / 40.0));
    let mut candidates = Vec::new();
    for lag in min_lag..=max_lag {
        let left = &envelope[lag..];
        let right = &envelope[..envelope.len() - lag];
        let numerator = left.iter().zip(right).map(|(a, b)| a * b).sum::<f64>();
        let denominator = (left.iter().map(|value| value * value).sum::<f64>()
            * right.iter().map(|value| value * value).sum::<f64>())
        .sqrt();
        if denominator <= 1e-12 {
            continue;
        }
        let correlation = clamp(numerator / denominator, 0.0, 1.0);
        let bpm = 60.0 * windows_per_second / lag as f64;
        let prior = (-0.5 * (bpm / 120.0).log2().powi(2) / 0.75_f64.powi(2)).exp();
        candidates.push((correlation * (0.25 + 0.75 * prior), correlation, lag));
    }
    let Some((_, best_correlation, best_lag)) = candidates
        .iter()
        .copied()
        .max_by(|left, right| tuple_float_cmp(*left, *right))
    else {
        return (None, 0.0);
    };
    let harmonic_rival = candidates
        .iter()
        .filter(|(_, _, lag)| {
            *lag != best_lag
                && (*lag as isize - 2 * best_lag as isize)
                    .abs()
                    .min((2 * *lag as isize - best_lag as isize).abs())
                    <= 1
        })
        .map(|(_, correlation, _)| *correlation)
        .fold(0.0_f64, f64::max);
    let confidence = clamp(best_correlation * (1.0 - 0.35 * harmonic_rival), 0.0, 1.0);
    if best_correlation < 0.2 {
        (None, confidence)
    } else {
        (
            Some(60.0 * windows_per_second / best_lag as f64),
            confidence,
        )
    }
}

fn tuple_float_cmp(left: (f64, f64, usize), right: (f64, f64, usize)) -> CmpOrdering {
    left.0
        .total_cmp(&right.0)
        .then_with(|| left.1.total_cmp(&right.1))
        .then_with(|| left.2.cmp(&right.2))
}

#[derive(Debug, Clone, Copy)]
struct TempoPoint {
    at_fraction: f64,
    bpm: f64,
    confidence: f64,
}

fn tempo_curve(short_levels: &[f64], duration_s: f64) -> Vec<TempoPoint> {
    let windows_per_second = 20.0;
    let window = python_round_usize(windows_per_second * 30.0);
    let hop = python_round_usize(windows_per_second * 15.0);
    if short_levels.len() < window {
        let (tempo, confidence) = estimate_tempo(short_levels, windows_per_second);
        return tempo.map_or_else(Vec::new, |bpm| {
            vec![TempoPoint {
                at_fraction: 0.5,
                bpm: round_to(bpm, 2),
                confidence: round_to(confidence, 5),
            }]
        });
    }
    (0..=short_levels.len() - window)
        .step_by(hop)
        .filter_map(|start| {
            let (tempo, confidence) =
                estimate_tempo(&short_levels[start..start + window], windows_per_second);
            tempo.map(|bpm| TempoPoint {
                at_fraction: round_to(
                    clamp(
                        (start as f64 + window as f64 / 2.0)
                            / windows_per_second
                            / duration_s.max(0.001),
                        0.0,
                        1.0,
                    ),
                    5,
                ),
                bpm: round_to(bpm, 2),
                confidence: round_to(confidence, 5),
            })
        })
        .collect()
}

#[derive(Debug, Clone, Copy)]
struct TimelineRow {
    start_s: f64,
    duration_s: f64,
    loudness: f64,
    intensity: f64,
    rhythmic_drive: f64,
    brightness: f64,
    density: f64,
    spectral_flux: f64,
    bass_ratio: f64,
    mid_ratio: f64,
    high_ratio: f64,
    spectral_flatness: f64,
    peak_concentration: f64,
}

#[derive(Debug, Clone, Copy)]
enum TimelineMetric {
    Loudness,
    Intensity,
    RhythmicDrive,
    Brightness,
    Density,
    SpectralFlux,
}

impl TimelineMetric {
    const TRAJECTORIES: [Self; 6] = [
        Self::Loudness,
        Self::Intensity,
        Self::RhythmicDrive,
        Self::Brightness,
        Self::Density,
        Self::SpectralFlux,
    ];

    const fn name(self) -> &'static str {
        match self {
            Self::Loudness => "loudness",
            Self::Intensity => "intensity",
            Self::RhythmicDrive => "rhythmic_drive",
            Self::Brightness => "brightness",
            Self::Density => "density",
            Self::SpectralFlux => "spectral_flux",
        }
    }

    fn value(self, row: &TimelineRow) -> f64 {
        match self {
            Self::Loudness => row.loudness,
            Self::Intensity => row.intensity,
            Self::RhythmicDrive => row.rhythmic_drive,
            Self::Brightness => row.brightness,
            Self::Density => row.density,
            Self::SpectralFlux => row.spectral_flux,
        }
    }

    fn values(self, rows: &[TimelineRow]) -> Vec<f64> {
        rows.iter().map(|row| self.value(row)).collect()
    }
}

fn timeline_frames(frames: &[Frame], short_levels: &[f64]) -> Vec<TimelineRow> {
    let strengths = onset_strengths(short_levels);
    let short_per_frame = python_round_usize(CONTEXT_FRAME_SECONDS / 0.05).max(1);
    let mut previous_profile = None;
    frames
        .iter()
        .enumerate()
        .map(|(index, frame)| {
            let start = index.saturating_mul(short_per_frame);
            let end = strengths.len().min(start.saturating_add(short_per_frame));
            let local = strengths.get(start..end).unwrap_or(&[]);
            let onset_rate = local.iter().filter(|value| **value > 0.0).count() as f64
                / frame.duration_s.max(0.001);
            let flux = previous_profile.map_or(0.0, |previous: [f64; SPECTRAL_BANDS]| {
                0.5 * frame
                    .spectrum
                    .profile
                    .iter()
                    .zip(previous)
                    .map(|(left, right)| (left - right).abs())
                    .sum::<f64>()
            });
            previous_profile = Some(frame.spectrum.profile);
            let loudness = normalize(frame.loudness_dbfs, -50.0, -10.0);
            let centroid_brightness = normalize(
                frame.spectrum.centroid_hz.max(250.0).log2(),
                250.0_f64.log2(),
                4_000.0_f64.log2(),
            );
            let rolloff_brightness = normalize(
                frame.spectrum.rolloff_hz.max(1_000.0).log2(),
                1_000.0_f64.log2(),
                7_000.0_f64.log2(),
            );
            let brightness = clamp(
                0.78 * centroid_brightness + 0.22 * rolloff_brightness,
                0.0,
                1.0,
            );
            let onset_strength = mean(
                &local
                    .iter()
                    .map(|value| clamp(value / 12.0, 0.0, 1.0))
                    .collect::<Vec<_>>(),
            );
            let rhythmic_drive = clamp(
                0.72 * normalize(onset_rate, 0.0, 5.0) + 0.28 * onset_strength,
                0.0,
                1.0,
            );
            let density = clamp(
                0.45 * frame.spectrum.spectral_entropy
                    + 0.25 * frame.spectrum.band_coverage
                    + 0.20 * normalize(frame.spectrum.bandwidth_hz, 300.0, 3_500.0)
                    + 0.10 * frame.spectrum.flatness,
                0.0,
                1.0,
            );
            let intensity = clamp(
                0.50 * loudness + 0.30 * rhythmic_drive + 0.20 * density,
                0.0,
                1.0,
            );
            TimelineRow {
                start_s: round_to(frame.start_s, 3),
                duration_s: round_to(frame.duration_s, 3),
                loudness: round_to(loudness, 5),
                intensity: round_to(intensity, 5),
                rhythmic_drive: round_to(rhythmic_drive, 5),
                brightness: round_to(brightness, 5),
                density: round_to(density, 5),
                spectral_flux: round_to(flux, 5),
                bass_ratio: round_to(frame.spectrum.bass_ratio, 5),
                mid_ratio: round_to(frame.spectrum.mid_ratio, 5),
                high_ratio: round_to(frame.spectrum.high_ratio, 5),
                spectral_flatness: round_to(frame.spectrum.flatness, 5),
                peak_concentration: round_to(frame.spectrum.peak_concentration, 5),
            }
        })
        .collect()
}

fn trajectory(values: &[f64]) -> Value {
    if values.is_empty() {
        return json!({
            "typical": 0.0, "low": 0.0, "high": 0.0, "range": 0.0,
            "variability": 0.0, "slope": 0.0, "start": 0.0, "end": 0.0,
            "peak_at_fraction": 0.0, "high_fraction": 0.0, "shape": "unknown",
        });
    }
    let count = values.len();
    let edge = (count / 10).max(1);
    let start = mean(&values[..edge]);
    let end = mean(&values[count - edge..]);
    let y_mean = mean(values);
    let denominator = (0..count)
        .map(|index| (index as f64 / count.saturating_sub(1).max(1) as f64 - 0.5).powi(2))
        .sum::<f64>();
    let slope = if denominator > 0.0 {
        values
            .iter()
            .enumerate()
            .map(|(index, value)| {
                (index as f64 / count.saturating_sub(1).max(1) as f64 - 0.5) * (value - y_mean)
            })
            .sum::<f64>()
            / denominator
    } else {
        0.0
    };
    let deltas = values
        .windows(2)
        .map(|pair| (pair[1] - pair[0]).abs())
        .collect::<Vec<_>>();
    let low = quantile(values, 0.1);
    let high = quantile(values, 0.9);
    let variability = clamp(
        0.65 * (high - low) + 0.35 * (median(&deltas) * 6.0).min(1.0),
        0.0,
        1.0,
    );
    let quarters = [(0.0, 0.25), (0.25, 0.5), (0.5, 0.75), (0.75, 1.0)].map(
        |(start_fraction, end_fraction)| {
            let start_index = python_round_usize(count as f64 * start_fraction);
            let end_index = python_round_usize(count as f64 * end_fraction)
                .max(start_index.saturating_add(1))
                .min(count);
            mean(&values[start_index.min(count - 1)..end_index])
        },
    );
    let shape = if high - low < 0.12 && variability < 0.18 {
        "steady"
    } else if variability > 0.58 {
        "volatile"
    } else if quarters[1] > quarters[0] + 0.15 && quarters[2] > quarters[3] + 0.15 {
        "arch"
    } else if quarters[1] < quarters[0] - 0.15 && quarters[2] < quarters[3] - 0.15 {
        "dip_then_recovery"
    } else if end - start > 0.28 {
        if variability < 0.4 {
            "gradual_rise"
        } else {
            "stepped_build"
        }
    } else if start - end > 0.28 {
        if variability < 0.4 {
            "gradual_fall"
        } else {
            "stepped_release"
        }
    } else if variability > 0.32 {
        "alternating"
    } else if slope > 0.12 {
        "rising"
    } else if slope < -0.12 {
        "falling"
    } else {
        "mixed"
    };
    let mut peak_index = 0;
    for (index, value) in values.iter().enumerate().skip(1) {
        if value.total_cmp(&values[peak_index]).is_gt() {
            peak_index = index;
        }
    }
    json!({
        "typical": round_to(median(values), 5),
        "low": round_to(low, 5),
        "high": round_to(high, 5),
        "range": round_to(high - low, 5),
        "variability": round_to(variability, 5),
        "slope": round_to(slope, 5),
        "start": round_to(start, 5),
        "end": round_to(end, 5),
        "peak_at_fraction": round_to(peak_index as f64 / count.saturating_sub(1).max(1) as f64, 5),
        "high_fraction": round_to(values.iter().filter(|value| **value >= 2.0 / 3.0).count() as f64 / count as f64, 5),
        "shape": shape,
    })
}

fn downsample_timeline(rows: &[TimelineRow]) -> Vec<Map<String, Value>> {
    let group_size = python_round_usize(CONTEXT_TIMELINE_SECONDS / CONTEXT_FRAME_SECONDS).max(1);
    rows.chunks(group_size)
        .map(|group| {
            object(json!({
                "start_s": group[0].start_s,
                "duration_s": round_to(group.iter().map(|row| row.duration_s).sum(), 3),
                "loudness": round_to(mean(&group.iter().map(|row| row.loudness).collect::<Vec<_>>()), 5),
                "intensity": round_to(mean(&group.iter().map(|row| row.intensity).collect::<Vec<_>>()), 5),
                "rhythmic_drive": round_to(mean(&group.iter().map(|row| row.rhythmic_drive).collect::<Vec<_>>()), 5),
                "brightness": round_to(mean(&group.iter().map(|row| row.brightness).collect::<Vec<_>>()), 5),
                "density": round_to(mean(&group.iter().map(|row| row.density).collect::<Vec<_>>()), 5),
                "spectral_flux": round_to(mean(&group.iter().map(|row| row.spectral_flux).collect::<Vec<_>>()), 5),
                "bass_ratio": round_to(mean(&group.iter().map(|row| row.bass_ratio).collect::<Vec<_>>()), 5),
                "mid_ratio": round_to(mean(&group.iter().map(|row| row.mid_ratio).collect::<Vec<_>>()), 5),
                "high_ratio": round_to(mean(&group.iter().map(|row| row.high_ratio).collect::<Vec<_>>()), 5),
                "spectral_flatness": round_to(mean(&group.iter().map(|row| row.spectral_flatness).collect::<Vec<_>>()), 5),
                "peak_concentration": round_to(mean(&group.iter().map(|row| row.peak_concentration).collect::<Vec<_>>()), 5),
            }))
        })
        .collect()
}

fn change_boundaries(rows: &[TimelineRow]) -> Vec<usize> {
    if rows.len() < 12 {
        return vec![0, rows.len()];
    }
    let windows = [4, 8, 16]
        .into_iter()
        .filter(|window| rows.len() >= window * 3)
        .collect::<Vec<_>>();
    let windows = if windows.is_empty() { vec![4] } else { windows };
    let largest = windows.iter().copied().max().unwrap_or(4);
    let metrics = [
        TimelineMetric::Intensity,
        TimelineMetric::RhythmicDrive,
        TimelineMetric::Brightness,
        TimelineMetric::Density,
        TimelineMetric::SpectralFlux,
    ];
    let scores = (largest..rows.len().saturating_sub(largest))
        .map(|index| {
            let score = mean(
                &windows
                    .iter()
                    .map(|window| {
                        mean(
                            &metrics
                                .iter()
                                .map(|metric| {
                                    let before = mean(&metric.values(&rows[index - window..index]));
                                    let after = mean(&metric.values(&rows[index..index + window]));
                                    (before - after).abs()
                                })
                                .collect::<Vec<_>>(),
                        )
                    })
                    .collect::<Vec<_>>(),
            );
            (score, index)
        })
        .collect::<Vec<_>>();
    if scores.is_empty() {
        return vec![0, rows.len()];
    }
    let score_values = scores.iter().map(|(score, _)| *score).collect::<Vec<_>>();
    let score_median = median(&score_values);
    let threshold = 0.08_f64.max(
        quantile(&score_values, 0.75)
            + median(
                &score_values
                    .iter()
                    .map(|score| (score - score_median).abs())
                    .collect::<Vec<_>>(),
            ),
    );
    let minimum = python_round_usize(MIN_SECTION_SECONDS / CONTEXT_FRAME_SECONDS).max(2);
    let mut ordered = scores;
    ordered.sort_by(|left, right| {
        right
            .0
            .total_cmp(&left.0)
            .then_with(|| right.1.cmp(&left.1))
    });
    let mut selected = Vec::new();
    for (score, index) in ordered {
        if score < threshold || selected.len() >= MAX_SECTIONS - 1 {
            break;
        }
        if index < minimum || rows.len() - index < minimum {
            continue;
        }
        if selected
            .iter()
            .all(|existing: &usize| index.abs_diff(*existing) >= minimum)
        {
            selected.push(index);
        }
    }
    selected.sort_unstable();
    let mut boundaries = Vec::with_capacity(selected.len() + 2);
    boundaries.push(0);
    boundaries.extend(selected);
    boundaries.push(rows.len());
    boundaries
}

fn section_summary(
    rows: &[TimelineRow],
    boundaries: &[usize],
    tempo_points: &[TempoPoint],
    duration_s: f64,
) -> Vec<Map<String, Value>> {
    let mut sections = Vec::new();
    for (number, pair) in boundaries.windows(2).enumerate() {
        let group = &rows[pair[0]..pair[1]];
        let start_s = group[0].start_s;
        let end_s = group[group.len() - 1].start_s + group[group.len() - 1].duration_s;
        let center_fraction = ((start_s + end_s) / 2.0) / duration_s.max(0.001);
        let nearest = tempo_points.iter().min_by(|left, right| {
            (left.at_fraction - center_fraction)
                .abs()
                .total_cmp(&(right.at_fraction - center_fraction).abs())
        });
        let mut section = object(json!({
            "id": format!("s{}", number + 1),
            "start_s": round_to(start_s, 3),
            "end_s": round_to(end_s, 3),
            "start_fraction": round_to(start_s / duration_s.max(0.001), 5),
            "end_fraction": round_to(end_s / duration_s.max(0.001), 5),
            "intensity": round_to(median(&group.iter().map(|row| row.intensity).collect::<Vec<_>>()), 5),
            "rhythmic_drive": round_to(median(&group.iter().map(|row| row.rhythmic_drive).collect::<Vec<_>>()), 5),
            "brightness": round_to(median(&group.iter().map(|row| row.brightness).collect::<Vec<_>>()), 5),
            "density": round_to(median(&group.iter().map(|row| row.density).collect::<Vec<_>>()), 5),
            "tempo_bpm": nearest.map(|point| point.bpm),
            "tempo_confidence": nearest.map_or(0.0, |point| point.confidence),
            "changes_from_previous": [],
            "repeats_section_ids": [],
        }));
        if let Some(previous) = sections.last() {
            let mut changes = Vec::new();
            for (key, upward, downward) in [
                ("intensity", "more_intense", "less_intense"),
                ("rhythmic_drive", "more_rhythmic", "less_rhythmic"),
                ("brightness", "brighter", "darker_spectrum"),
                ("density", "denser", "sparser"),
            ] {
                let delta = number_value(&section, key) - number_value(previous, key);
                if delta >= 0.14 {
                    changes.push(upward);
                } else if delta <= -0.14 {
                    changes.push(downward);
                }
            }
            section.insert("changes_from_previous".to_owned(), json!(changes));
        }
        let repeated = sections
            .iter()
            .take(sections.len().saturating_sub(1))
            .filter(|earlier| {
                let sum = ["intensity", "rhythmic_drive", "brightness", "density"]
                    .into_iter()
                    .map(|key| (number_value(&section, key) - number_value(earlier, key)).powi(2))
                    .sum::<f64>();
                (sum / 4.0).sqrt() <= 0.10
            })
            .filter_map(|earlier| earlier.get("id").and_then(Value::as_str))
            .collect::<Vec<_>>();
        section.insert("repeats_section_ids".to_owned(), json!(repeated));
        sections.push(section);
    }
    sections
}

fn tempo_summary(points: &[TempoPoint], bpms: &[f64]) -> Value {
    let point_values = points
        .iter()
        .take(20)
        .map(|point| {
            json!({
                "at_fraction": point.at_fraction,
                "bpm": point.bpm,
                "confidence": point.confidence,
            })
        })
        .collect::<Vec<_>>();
    if bpms.is_empty() {
        json!({
            "status": "unresolved",
            "typical_bpm": null,
            "low_bpm": null,
            "high_bpm": null,
            "variability": null,
            "points": point_values,
        })
    } else {
        let low = quantile(bpms, 0.1);
        let high = quantile(bpms, 0.9);
        json!({
            "status": "measured",
            "typical_bpm": round_to(median(bpms), 2),
            "low_bpm": round_to(low, 2),
            "high_bpm": round_to(high, 2),
            "variability": round_to(clamp((high - low) / 60.0, 0.0, 1.0), 5),
            "points": point_values,
        })
    }
}

fn technical_probe(
    executable: &Path,
    path: &Path,
    cancelled: &AtomicBool,
) -> Result<Map<String, Value>, AudioContextError> {
    let extension = path
        .extension()
        .and_then(OsStr::to_str)
        .map_or_else(String::new, |value| format!(".{}", value.to_lowercase()));
    let arguments = [
        "-v",
        "error",
        "-select_streams",
        "a:0",
        "-show_entries",
        "stream=codec_name,sample_rate,channels,channel_layout,bit_rate",
        "-show_entries",
        "format=format_name,duration,bit_rate",
        "-of",
        "json",
    ];
    let output = match run_capture(executable, arguments, path, PROBE_TIMEOUT, cancelled) {
        Ok(output) if output.status.success() => output,
        Err(AudioContextError::Spawn(error)) if error.kind() == io::ErrorKind::NotFound => {
            return Ok(object(json!({
                "probe_status": "unavailable",
                "file_extension": extension,
            })));
        }
        Err(AudioContextError::Cancelled) => return Err(AudioContextError::Cancelled),
        _ => {
            return Ok(object(json!({
                "probe_status": "failed",
                "file_extension": extension,
            })));
        }
    };
    let parsed = serde_json::from_slice::<Value>(&output.stdout).ok();
    let stream = parsed
        .as_ref()
        .and_then(|value| value.get("streams"))
        .and_then(Value::as_array)
        .and_then(|values| values.first())
        .and_then(Value::as_object);
    let format = parsed
        .as_ref()
        .and_then(|value| value.get("format"))
        .and_then(Value::as_object);
    let Some(stream) = stream else {
        return Ok(object(json!({
            "probe_status": "failed",
            "file_extension": extension,
        })));
    };
    let bit_rate = safe_u64(stream.get("bit_rate"))
        .or_else(|| format.and_then(|value| safe_u64(value.get("bit_rate"))));
    Ok(object(json!({
        "probe_status": "complete",
        "file_extension": extension,
        "codec": stream.get("codec_name").cloned().unwrap_or(Value::Null),
        "sample_rate_hz": safe_u64(stream.get("sample_rate")),
        "channels": safe_u64(stream.get("channels")),
        "channel_layout": stream.get("channel_layout").cloned().unwrap_or(Value::Null),
        "bit_rate": bit_rate,
        "container": format.and_then(|value| value.get("format_name")).cloned().unwrap_or(Value::Null),
    })))
}

#[derive(Debug, Clone, Copy)]
struct EbuLoudness {
    integrated_lufs: f64,
    loudness_range_lu: f64,
    true_peak_dbtp: f64,
    relative_threshold_lufs: f64,
}

fn ebu_loudness(
    executable: &Path,
    path: &Path,
    cancelled: &AtomicBool,
) -> Result<Option<EbuLoudness>, AudioContextError> {
    let arguments = ["-hide_banner", "-nostats", "-v", "info", "-nostdin", "-i"];
    let trailing = [
        "-map",
        "0:a:0",
        "-af",
        "loudnorm=I=-24:LRA=7:TP=-2:print_format=json",
        "-f",
        "null",
        "-",
    ];
    let output = match run_capture_split(
        executable,
        &arguments,
        path,
        &trailing,
        LOUDNESS_TIMEOUT,
        cancelled,
    ) {
        Ok(output) if output.status.success() => output,
        Err(AudioContextError::Cancelled) => return Err(AudioContextError::Cancelled),
        _ => return Ok(None),
    };
    let text = String::from_utf8_lossy(&output.stderr);
    let Some(start) = text
        .rfind("{\n\t\"input_i\"")
        .or_else(|| text.rfind("{\r\n\t\"input_i\""))
    else {
        return Ok(None);
    };
    let Some(relative_end) = text[start..].find('}') else {
        return Ok(None);
    };
    let parsed = serde_json::from_str::<Value>(&text[start..=start + relative_end]).ok();
    let number = |key| {
        parsed
            .as_ref()
            .and_then(|value| value.get(key))
            .and_then(|value| match value {
                Value::String(value) => value.parse::<f64>().ok(),
                Value::Number(value) => value.as_f64(),
                _ => None,
            })
            .filter(|value| value.is_finite())
    };
    let Some(integrated_lufs) = number("input_i") else {
        return Ok(None);
    };
    let Some(loudness_range_lu) = number("input_lra") else {
        return Ok(None);
    };
    let Some(true_peak_dbtp) = number("input_tp") else {
        return Ok(None);
    };
    let Some(relative_threshold_lufs) = number("input_thresh") else {
        return Ok(None);
    };
    Ok(Some(EbuLoudness {
        integrated_lufs,
        loudness_range_lu,
        true_peak_dbtp,
        relative_threshold_lufs,
    }))
}

struct CapturedOutput {
    status: ExitStatus,
    stdout: Vec<u8>,
    stderr: Vec<u8>,
}

fn run_capture<const N: usize>(
    executable: &Path,
    arguments: [&str; N],
    path: &Path,
    timeout: Duration,
    cancelled: &AtomicBool,
) -> Result<CapturedOutput, AudioContextError> {
    run_capture_split(executable, &arguments, path, &[], timeout, cancelled)
}

fn run_capture_split(
    executable: &Path,
    before_path: &[&str],
    path: &Path,
    after_path: &[&str],
    timeout: Duration,
    cancelled: &AtomicBool,
) -> Result<CapturedOutput, AudioContextError> {
    let mut command = Command::new(executable);
    command
        .args(before_path)
        .arg(path)
        .args(after_path)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = command.spawn().map_err(AudioContextError::Spawn)?;
    capture_child(&mut child, timeout, cancelled)
}

fn capture_child(
    child: &mut Child,
    timeout: Duration,
    cancelled: &AtomicBool,
) -> Result<CapturedOutput, AudioContextError> {
    let stdout = child.stdout.take().ok_or(AudioContextError::Decode)?;
    let stderr = child.stderr.take().ok_or(AudioContextError::Decode)?;
    let stdout_thread = thread::spawn(move || read_bounded(stdout));
    let stderr_thread = thread::spawn(move || read_bounded(stderr));
    let deadline = Instant::now() + timeout;
    let status = loop {
        if cancelled.load(Ordering::Relaxed) {
            let _ = child.kill();
            let _ = child.wait();
            let _ = stdout_thread.join();
            let _ = stderr_thread.join();
            return Err(AudioContextError::Cancelled);
        }
        if let Some(status) = child.try_wait().map_err(AudioContextError::Io)? {
            break status;
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let status = child.wait().map_err(AudioContextError::Io)?;
            let _ = stdout_thread.join();
            let _ = stderr_thread.join();
            return Ok(CapturedOutput {
                status,
                stdout: Vec::new(),
                stderr: Vec::new(),
            });
        }
        thread::sleep(Duration::from_millis(50));
    };
    let stdout = stdout_thread.join().unwrap_or_default();
    let stderr = stderr_thread.join().unwrap_or_default();
    Ok(CapturedOutput {
        status,
        stdout,
        stderr,
    })
}

fn read_bounded(mut reader: impl Read) -> Vec<u8> {
    let mut output = Vec::new();
    let mut buffer = [0_u8; 4_096];
    loop {
        match reader.read(&mut buffer) {
            Ok(0) | Err(_) => return output,
            Ok(read) => {
                let remaining = CAPTURE_LIMIT.saturating_sub(output.len());
                output.extend_from_slice(&buffer[..read.min(remaining)]);
            }
        }
    }
}

fn voice_placeholders(
    preparation: &VoiceContextPreparation,
) -> (Value, Value, &'static str, &'static str) {
    match preparation {
        VoiceContextPreparation::Deferred => (
            json!({
                "status": "not_classified",
                "voice_probability": null,
                "vocal_coverage": null,
                "note": "Voice detection is waiting for the separate second analysis pass.",
            }),
            json!({
                "status": "pending",
                "required": false,
                "analyzer_id": "essentia-musicnn-voice/v1",
            }),
            "pending",
            "partial",
        ),
        VoiceContextPreparation::NotConfigured => (
            json!({
                "status": "not_classified",
                "voice_probability": null,
                "vocal_coverage": null,
                "note": "Local voice classification is not enabled. Spectral measurements are retained, but they are not presented as voice detection.",
            }),
            json!({
                "status": "not_configured",
                "required": false,
            }),
            "unavailable",
            "full",
        ),
        VoiceContextPreparation::Unavailable { reason } => (
            json!({
                "status": "unavailable",
                "voice_probability": null,
                "vocal_coverage": null,
                "note": "The configured local voice classifier is unavailable. The remaining track context is still available.",
            }),
            json!({
                "status": "unavailable",
                "required": false,
                "analyzer_id": "essentia-musicnn-voice/v1",
                "reason": reason,
                "model_filename": "voice_instrumental-musicnn-msd-2.pb",
            }),
            "unavailable",
            "full",
        ),
    }
}

fn trajectory_evidence(label: &str, trajectory: &Map<String, Value>) -> String {
    format!(
        "{label}: {} trajectory; typical {:.0}%, range {:.0}%-{:.0}%, peak at {:.0}% of the track.",
        trajectory
            .get("shape")
            .and_then(Value::as_str)
            .unwrap_or("unknown"),
        number_value(trajectory, "typical") * 100.0,
        number_value(trajectory, "low") * 100.0,
        number_value(trajectory, "high") * 100.0,
        number_value(trajectory, "peak_at_fraction") * 100.0,
    )
}

fn tempo_evidence(tempo: &Value) -> String {
    let Some(tempo) = tempo.as_object() else {
        return "No sufficiently stable tempo trajectory was resolved.".to_owned();
    };
    if tempo.get("status").and_then(Value::as_str) != Some("measured") {
        return "No sufficiently stable tempo trajectory was resolved.".to_owned();
    }
    format!(
        "Tempo trajectory: {:.1}-{:.1} BPM; typical {:.1} BPM.",
        number_value(tempo, "low_bpm"),
        number_value(tempo, "high_bpm"),
        number_value(tempo, "typical_bpm"),
    )
}

fn safe_u64(value: Option<&Value>) -> Option<u64> {
    value.and_then(|value| match value {
        Value::String(value) => value.parse().ok(),
        Value::Number(value) => value.as_u64(),
        _ => None,
    })
}

fn check_cancelled(cancelled: &AtomicBool) -> Result<(), AudioContextError> {
    if cancelled.load(Ordering::Relaxed) {
        Err(AudioContextError::Cancelled)
    } else {
        Ok(())
    }
}

fn object(value: Value) -> Map<String, Value> {
    value.as_object().cloned().unwrap_or_default()
}

fn number_value(values: &Map<String, Value>, key: &str) -> f64 {
    values.get(key).and_then(Value::as_f64).unwrap_or(0.0)
}

fn clamp(value: f64, low: f64, high: f64) -> f64 {
    value.max(low).min(high)
}

fn normalize(value: f64, low: f64, high: f64) -> f64 {
    clamp((value - low) / (high - low), 0.0, 1.0)
}

fn dbfs(amplitude: f64) -> f64 {
    20.0 * amplitude.max(1e-7).log10()
}

fn mean(values: &[f64]) -> f64 {
    if values.is_empty() {
        0.0
    } else {
        values.iter().sum::<f64>() / values.len() as f64
    }
}

fn quantile(values: &[f64], fraction: f64) -> f64 {
    if values.is_empty() {
        return 0.0;
    }
    let mut ordered = values.to_vec();
    ordered.sort_by(f64::total_cmp);
    let position = clamp(fraction, 0.0, 1.0) * ordered.len().saturating_sub(1) as f64;
    let lower = position.floor() as usize;
    let upper = position.ceil() as usize;
    if lower == upper {
        ordered[lower]
    } else {
        let weight = position - lower as f64;
        ordered[lower] * (1.0 - weight) + ordered[upper] * weight
    }
}

fn median(values: &[f64]) -> f64 {
    quantile(values, 0.5)
}

fn mel_hz(value: f64) -> f64 {
    700.0 * (10_f64.powf(value / 2_595.0) - 1.0)
}

fn hz_mel(value: f64) -> f64 {
    2_595.0 * (1.0 + value.max(0.0) / 700.0).log10()
}

fn round_to(value: f64, digits: i32) -> f64 {
    let factor = 10_f64.powi(digits);
    (value * factor).round_ties_even() / factor
}

fn python_round_usize(value: f64) -> usize {
    value.round_ties_even().max(0.0) as usize
}

#[cfg(test)]
mod tests {
    use std::f64::consts::TAU;
    use std::io::Write;
    use std::sync::atomic::AtomicBool;

    use super::{
        AudioContextAnalyzer, ContextAccumulator, FfmpegContextAnalyzer, TimelineMetric,
        VoiceContextPreparation, estimate_tempo, timeline_frames, trajectory, voice_placeholders,
    };

    #[test]
    fn configured_but_unavailable_voice_backend_is_not_reported_as_disabled() {
        let (summary, stage, reliability, completeness) =
            voice_placeholders(&VoiceContextPreparation::Unavailable {
                reason: "unsupported_model".to_owned(),
            });
        assert_eq!(summary["status"], "unavailable");
        assert_eq!(stage["status"], "unavailable");
        assert_eq!(stage["reason"], "unsupported_model");
        assert_eq!(reliability, "unavailable");
        assert_eq!(completeness, "full");
    }

    #[test]
    fn rustfft_matches_the_v2_two_tone_calibration() -> Result<(), Box<dyn std::error::Error>> {
        let mut accumulator = ContextAccumulator::new(16_000)?;
        for index in 0..8_000_u32 {
            let sample = 0.55 * (TAU * 440.0 * f64::from(index) / 16_000.0).sin()
                + 0.2 * (TAU * 1_700.0 * f64::from(index) / 16_000.0).sin();
            accumulator.frame.push(sample);
        }
        let spectrum = accumulator.spectrum();
        assert!((spectrum.centroid_hz - 777.183_034_151_454_9).abs() < 1e-8);
        assert!((spectrum.bandwidth_hz - 558.048_198_420_262_7).abs() < 1e-8);
        assert_eq!(spectrum.rolloff_hz, 1_695.312_5);
        assert!((spectrum.flatness - 7.769_654_422_762_687e-14).abs() < 1e-20);
        assert!((spectrum.bass_ratio - 7.367_360_027_224_39e-10).abs() < 1e-16);
        assert!((spectrum.mid_ratio - 0.999_999_999_247_039).abs() < 1e-12);
        assert!((spectrum.high_ratio - 1.605_474_666_777_969e-11).abs() < 1e-17);
        assert!((spectrum.peak_concentration - 0.999_825_279_846_876_6).abs() < 1e-12);
        assert!((spectrum.spectral_entropy - 0.113_592_960_950_734_1).abs() < 1e-12);
        assert_eq!(spectrum.band_coverage, 2.0 / 24.0);
        Ok(())
    }

    #[test]
    fn v2_tempo_and_absolute_high_fraction_keep_their_semantics() {
        let levels = (0..600)
            .map(|index| {
                if index % 10 == 0 {
                    if (index / 10) % 2 == 0 { 1.0 } else { 0.35 }
                } else {
                    0.05
                }
            })
            .collect::<Vec<_>>();
        let (tempo, confidence) = estimate_tempo(&levels, 20.0);
        let quieter = levels.iter().map(|value| value * 0.1).collect::<Vec<_>>();
        let (quieter_tempo, quieter_confidence) = estimate_tempo(&quieter, 20.0);
        assert_eq!(tempo, Some(120.0));
        assert_eq!(quieter_tempo, tempo);
        assert!((quieter_confidence - confidence).abs() < 1e-12);
        assert!(confidence > 0.4 && confidence < 0.8);
        assert_eq!(trajectory(&vec![0.2; 100])["high_fraction"], 0.0);
        assert_eq!(trajectory(&vec![0.8; 100])["high_fraction"], 1.0);
        assert_eq!(
            trajectory(
                &(0..100)
                    .map(|index| index as f64 / 99.0)
                    .collect::<Vec<_>>()
            )["high_fraction"],
            0.34
        );
    }

    #[test]
    fn v2_brightness_and_mel_flux_preserve_the_controlled_transition()
    -> Result<(), Box<dyn std::error::Error>> {
        let mut frames = Vec::new();
        for (frame_index, frequency) in [500.0, 1_500.0, 4_000.0].into_iter().enumerate() {
            let mut accumulator = ContextAccumulator::new(16_000)?;
            for index in 0..8_000_u32 {
                accumulator
                    .frame
                    .push(0.5 * (TAU * frequency * f64::from(index) / 16_000.0).sin());
            }
            let spectrum = accumulator.spectrum();
            frames.push(super::Frame {
                start_s: frame_index as f64 * 0.5,
                duration_s: 0.5,
                loudness_dbfs: -10.0,
                spectrum,
            });
        }
        let rows = timeline_frames(&frames, &[0.5; 30]);
        assert!((rows[0].brightness - 0.195).abs() < 0.01);
        assert!((rows[1].brightness - 0.55).abs() < 0.01);
        assert!((rows[2].brightness - 0.937).abs() < 0.01);
        assert!(TimelineMetric::SpectralFlux.value(&rows[1]) > 0.9);
        Ok(())
    }

    #[test]
    fn full_rust_document_matches_the_python_v2_development_probe()
    -> Result<(), Box<dyn std::error::Error>> {
        let directory = tempfile::tempdir()?;
        let path = directory.path().join("developing.wav");
        write_developing_wav(&path)?;
        let ffmpeg = std::env::var_os("MUSIC_TEST_FFMPEG").unwrap_or_else(|| "ffmpeg".into());
        let ffprobe = std::env::var_os("MUSIC_TEST_FFPROBE").unwrap_or_else(|| "ffprobe".into());
        let analyzer = FfmpegContextAnalyzer::new(ffmpeg, ffprobe);
        let document = analyzer.analyze(
            &path,
            &AtomicBool::new(false),
            VoiceContextPreparation::NotConfigured,
        )?;
        assert_eq!(document.summary["schema_version"], "local-context/v2");
        assert_eq!(document.summary["duration_s"], 12.0);
        assert_eq!(document.summary["confidence"], "medium");
        assert_eq!(
            document.summary["trajectories"]["intensity"]["typical"],
            0.542_36
        );
        assert_eq!(
            document.summary["trajectories"]["intensity"]["end"],
            0.516_2
        );
        assert_eq!(
            document.summary["trajectories"]["intensity"]["slope"],
            0.432_03
        );
        assert_eq!(
            document.summary["trajectories"]["rhythmic_drive"]["typical"],
            0.312_6
        );
        assert_eq!(document.summary["tempo"]["typical_bpm"], 120.0);
        assert_eq!(
            document.summary["tempo"]["points"][0]["confidence"],
            0.616_19
        );
        assert_eq!(document.sections.len(), 1);
        assert_eq!(document.sections[0]["intensity"], 0.542_36);
        assert_eq!(document.timeline.len(), 6);
        assert_eq!(document.timeline[3]["brightness"], 0.160_12);
        assert_eq!(document.timeline[3]["intensity"], 0.563_09);
        assert_eq!(document.stages["voice"]["status"], "not_configured");
        assert_eq!(document.completeness, "full");
        Ok(())
    }

    fn write_developing_wav(path: &std::path::Path) -> Result<(), std::io::Error> {
        let sample_rate = 16_000_u32;
        let sample_count = 12_u32.saturating_mul(sample_rate);
        let data_bytes = sample_count.saturating_mul(2);
        let mut output = std::fs::File::create(path)?;
        output.write_all(b"RIFF")?;
        output.write_all(&(36_u32.saturating_add(data_bytes)).to_le_bytes())?;
        output.write_all(b"WAVEfmt ")?;
        output.write_all(&16_u32.to_le_bytes())?;
        output.write_all(&1_u16.to_le_bytes())?;
        output.write_all(&1_u16.to_le_bytes())?;
        output.write_all(&sample_rate.to_le_bytes())?;
        output.write_all(&(sample_rate.saturating_mul(2)).to_le_bytes())?;
        output.write_all(&2_u16.to_le_bytes())?;
        output.write_all(&16_u16.to_le_bytes())?;
        output.write_all(b"data")?;
        output.write_all(&data_bytes.to_le_bytes())?;
        for index in 0..sample_count {
            let fraction = f64::from(index) / f64::from(sample_count);
            let amplitude = if fraction < 0.45 { 0.04 } else { 0.78 };
            let pulse = if fraction >= 0.45 && index % 8_000 >= 1_000 {
                0.25
            } else {
                1.0
            };
            let value =
                amplitude * pulse * (TAU * 440.0 * f64::from(index) / f64::from(sample_rate)).sin();
            let sample = (value.clamp(-1.0, 1.0) * 32_767.0).round_ties_even() as i16;
            output.write_all(&sample.to_le_bytes())?;
        }
        Ok(())
    }
}
