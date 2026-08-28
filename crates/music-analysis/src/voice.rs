use std::collections::VecDeque;
use std::error::Error;
use std::fmt::{self, Display, Formatter};
use std::fs::File;
use std::io::{self, Read};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{RecvTimeoutError, sync_channel};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use music_application::assistant::{
    VOICE_ANALYZER_ID, VOICE_MODEL_FILENAME, VOICE_MODEL_SHA256, VoiceAnalyzerStatus,
};
use rustfft::num_complex::Complex;
use rustfft::{Fft, FftPlanner};
use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};
use tokio::sync::{mpsc, oneshot};
use tract_tensorflow::prelude::*;
use tract_tensorflow::tract_hir::internal::{
    InferenceOp, IntoExp, Solver, TensorProxy, TractErrorContext, bail, check_input_arity,
    check_output_arity, ensure, inference_wrap,
};

const SAMPLE_RATE: u32 = 16_000;
const FRAME_SIZE: usize = 512;
const FRAME_HOP: usize = 256;
const MEL_BANDS: usize = 96;
const PATCH_FRAMES: usize = 187;
const PATCH_HOP: usize = 93;
const SPECTRUM_BINS: usize = FRAME_SIZE / 2 + 1;
const MAX_AUDIO_SECONDS: u64 = 24 * 60 * 60;
const FRAMES_PER_CHUNK: usize = 8_192;
const VOICE_REQUEST_CAPACITY: usize = 1;
const TRACT_RUNTIME_ID: &str = "tract-tensorflow/0.23.5+musicnn-compat/v1+preprocess/v1";

#[derive(Debug, Clone, PartialEq)]
pub struct VoiceAnalysisDocument {
    pub summary: Map<String, Value>,
    pub stage: Map<String, Value>,
    pub elapsed_seconds: f64,
    pub prediction_windows: usize,
}

impl VoiceAnalysisDocument {
    fn classified(
        voice_score: f64,
        vocal_coverage: f64,
        prediction_windows: usize,
        elapsed_seconds: f64,
    ) -> Self {
        Self {
            summary: object(json!({
                "status": "classified",
                // This key is retained for wire compatibility. The value is a
                // normalized model score, not a calibrated probability.
                "voice_probability": round_five(voice_score),
                "vocal_coverage": round_five(vocal_coverage),
                "note": classification_note(voice_score, vocal_coverage),
            })),
            stage: object(json!({
                "status": "complete",
                "required": false,
                "analyzer_id": VOICE_ANALYZER_ID,
                "model_sha256": VOICE_MODEL_SHA256,
                "prediction_windows": prediction_windows,
                "classes": ["instrumental", "voice"],
            })),
            elapsed_seconds,
            prediction_windows,
        }
    }

    #[must_use]
    pub fn unavailable(error: &VoiceAnalysisError, elapsed_seconds: f64) -> Self {
        Self {
            summary: object(json!({
                "status": "unavailable",
                "voice_probability": null,
                "vocal_coverage": null,
                "note": if matches!(error, VoiceAnalysisError::WorkerUnavailable) {
                    "The supported voice model is configured, but its isolated inference worker is unavailable."
                } else {
                    "The local voice classifier failed; the remaining track context is still available."
                },
            })),
            stage: object(json!({
                "status": "unavailable",
                "required": false,
                "analyzer_id": VOICE_ANALYZER_ID,
                "reason": if matches!(error, VoiceAnalysisError::WorkerUnavailable) {
                    "runtime_missing"
                } else {
                    "inference_failed"
                },
                "model_filename": VOICE_MODEL_FILENAME,
                "error_type": error.kind(),
            })),
            elapsed_seconds,
            prediction_windows: 0,
        }
    }
}

#[derive(Debug)]
pub enum VoiceAnalysisError {
    MissingFile,
    Spawn(io::Error),
    Decode,
    Io(io::Error),
    TooLong,
    Cancelled,
    Inference,
    WorkerUnavailable,
}

impl VoiceAnalysisError {
    const fn kind(&self) -> &'static str {
        match self {
            Self::MissingFile => "MissingFile",
            Self::Spawn(_) => "SpawnError",
            Self::Decode => "DecodeError",
            Self::Io(_) => "IoError",
            Self::TooLong => "TooLong",
            Self::Cancelled => "Cancelled",
            Self::Inference => "InferenceError",
            Self::WorkerUnavailable => "WorkerUnavailable",
        }
    }
}

impl Display for VoiceAnalysisError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::MissingFile => "audio file is missing",
            Self::Spawn(_) => "voice-analysis decoder could not start",
            Self::Decode => "voice-analysis audio could not be decoded",
            Self::Io(_) => "voice-analysis audio could not be read",
            Self::TooLong => "voice-analysis audio exceeds the 24-hour limit",
            Self::Cancelled => "voice analysis was cancelled",
            Self::Inference => "voice classifier inference failed",
            Self::WorkerUnavailable => "voice-analysis worker is unavailable",
        })
    }
}

impl Error for VoiceAnalysisError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Spawn(error) | Self::Io(error) => Some(error),
            _ => None,
        }
    }
}

#[derive(Debug)]
pub struct VoiceBackend {
    pub status: VoiceAnalyzerStatus,
    pub worker: Option<VoiceWorker>,
}

impl VoiceBackend {
    #[must_use]
    pub fn initialize(model_path: Option<&Path>, ffmpeg: impl Into<PathBuf>) -> Self {
        let Some(model_path) = model_path else {
            return Self {
                status: VoiceAnalyzerStatus::not_configured(),
                worker: None,
            };
        };
        if !model_path.is_file() {
            return unavailable_backend("model_missing", "missing");
        }
        let Ok(model_hash) = sha256_file(model_path) else {
            return unavailable_backend("model_unreadable", "unreadable");
        };
        if model_hash != VOICE_MODEL_SHA256 {
            return unavailable_backend("unsupported_model", &model_hash);
        }
        let Ok(worker) = VoiceWorker::start(model_path.to_owned(), ffmpeg.into()) else {
            return unavailable_backend("runtime_missing", &model_hash);
        };
        let signature = format!("{VOICE_ANALYZER_ID}:{model_hash}:{TRACT_RUNTIME_ID}");
        Self {
            status: VoiceAnalyzerStatus::ready(signature),
            worker: Some(worker),
        }
    }
}

fn unavailable_backend(reason: &'static str, model_identity: &str) -> VoiceBackend {
    let signature = format!("{VOICE_ANALYZER_ID}:{model_identity}:{TRACT_RUNTIME_ID}:{reason}");
    VoiceBackend {
        status: VoiceAnalyzerStatus::unavailable_with_signature(reason, signature),
        worker: None,
    }
}

#[derive(Clone)]
pub struct VoiceWorker {
    inner: Arc<VoiceWorkerInner>,
}

impl fmt::Debug for VoiceWorker {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("VoiceWorker")
            .field("alive", &self.is_alive())
            .finish_non_exhaustive()
    }
}

impl VoiceWorker {
    fn start(model_path: PathBuf, ffmpeg: PathBuf) -> Result<Self, ()> {
        let (sender, mut receiver) = mpsc::channel::<VoiceRequest>(VOICE_REQUEST_CAPACITY);
        let (startup_sender, startup_receiver) = sync_channel(1);
        let alive = Arc::new(AtomicBool::new(false));
        let thread_alive = Arc::clone(&alive);
        let handle = thread::Builder::new()
            .name("music-voice-analysis".to_owned())
            .spawn(move || {
                let run = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    let Ok(mut model) = TractVoiceModel::load(&model_path) else {
                        let _ = startup_sender.send(false);
                        return;
                    };
                    thread_alive.store(true, Ordering::Release);
                    if startup_sender.send(true).is_err() {
                        return;
                    }
                    while let Some(request) = receiver.blocking_recv() {
                        let result = analyze_voice_file(
                            &mut model,
                            &ffmpeg,
                            &request.path,
                            &request.cancelled,
                        );
                        let _ = request.response.send(result);
                    }
                }));
                thread_alive.store(false, Ordering::Release);
                let _ = run;
            })
            .map_err(|_| ())?;
        if startup_receiver.recv() != Ok(true) {
            drop(sender);
            let _ = handle.join();
            return Err(());
        }
        Ok(Self {
            inner: Arc::new(VoiceWorkerInner {
                sender: Mutex::new(Some(sender)),
                handle: Mutex::new(Some(handle)),
                alive,
            }),
        })
    }

    #[must_use]
    pub fn is_alive(&self) -> bool {
        self.inner.alive.load(Ordering::Acquire)
    }

    pub async fn analyze(
        &self,
        path: PathBuf,
        cancelled: Arc<AtomicBool>,
    ) -> Result<VoiceAnalysisDocument, VoiceAnalysisError> {
        if !self.is_alive() {
            return Err(VoiceAnalysisError::WorkerUnavailable);
        }
        let sender = self
            .inner
            .sender
            .lock()
            .ok()
            .and_then(|sender| sender.as_ref().cloned())
            .ok_or(VoiceAnalysisError::WorkerUnavailable)?;
        let (response, result) = oneshot::channel();
        sender
            .send(VoiceRequest {
                path,
                cancelled,
                response,
            })
            .await
            .map_err(|_| VoiceAnalysisError::WorkerUnavailable)?;
        result
            .await
            .map_err(|_| VoiceAnalysisError::WorkerUnavailable)?
    }
}

struct VoiceWorkerInner {
    sender: Mutex<Option<mpsc::Sender<VoiceRequest>>>,
    handle: Mutex<Option<thread::JoinHandle<()>>>,
    alive: Arc<AtomicBool>,
}

impl Drop for VoiceWorkerInner {
    fn drop(&mut self) {
        if let Ok(sender) = self.sender.get_mut() {
            let _ = sender.take();
        }
        if let Ok(handle) = self.handle.get_mut()
            && let Some(handle) = handle.take()
        {
            let _ = handle.join();
        }
    }
}

struct VoiceRequest {
    path: PathBuf,
    cancelled: Arc<AtomicBool>,
    response: oneshot::Sender<Result<VoiceAnalysisDocument, VoiceAnalysisError>>,
}

fn analyze_voice_file(
    model: &mut impl VoicePredictor,
    ffmpeg: &Path,
    path: &Path,
    cancelled: &AtomicBool,
) -> Result<VoiceAnalysisDocument, VoiceAnalysisError> {
    let started = Instant::now();
    let output = decode_and_predict(model, ffmpeg, path, cancelled)?;
    let (voice_score, vocal_coverage) = summarize_predictions(&output.predictions)?;
    Ok(VoiceAnalysisDocument::classified(
        voice_score,
        vocal_coverage,
        output.predictions.len(),
        started.elapsed().as_secs_f64(),
    ))
}

fn decode_and_predict(
    model: &mut impl VoicePredictor,
    executable: &Path,
    path: &Path,
    cancelled: &AtomicBool,
) -> Result<PipelineOutput, VoiceAnalysisError> {
    if !path.is_file() {
        return Err(VoiceAnalysisError::MissingFile);
    }
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
        .arg(SAMPLE_RATE.to_string())
        .arg("-f")
        .arg("f32le")
        .arg("-acodec")
        .arg("pcm_f32le")
        .arg("pipe:1")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = command.spawn().map_err(VoiceAnalysisError::Spawn)?;
    let stdout = child.stdout.take().ok_or(VoiceAnalysisError::Decode)?;
    let stderr = child.stderr.take().ok_or(VoiceAnalysisError::Decode)?;
    let error_thread = thread::spawn(move || drain(stderr));
    let (sender, receiver) = sync_channel(2);
    let audio_thread = thread::spawn(move || read_audio(stdout, sender));
    let mut pipeline = VoicePipeline::new()?;
    let mut pending = Vec::with_capacity(3);
    let stream_result = 'stream: loop {
        if cancelled.load(Ordering::Relaxed) {
            break Err(VoiceAnalysisError::Cancelled);
        }
        match receiver.recv_timeout(Duration::from_millis(100)) {
            Ok(VoiceAudioRead::End) => {
                if !pending.is_empty() {
                    break Err(VoiceAnalysisError::Decode);
                }
                break pipeline.finish(model, cancelled);
            }
            Ok(VoiceAudioRead::Data(bytes)) => {
                let mut combined = Vec::with_capacity(pending.len().saturating_add(bytes.len()));
                combined.extend_from_slice(&pending);
                combined.extend_from_slice(&bytes);
                let complete_bytes = combined.len() / 4 * 4;
                for sample in combined[..complete_bytes].chunks_exact(4) {
                    let value = f32::from_le_bytes([sample[0], sample[1], sample[2], sample[3]]);
                    if let Err(error) = pipeline.add_sample(value, model, cancelled) {
                        break 'stream Err(error);
                    }
                }
                pending.clear();
                pending.extend_from_slice(&combined[complete_bytes..]);
            }
            Ok(VoiceAudioRead::Error(error)) => break Err(VoiceAnalysisError::Io(error)),
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => break Err(VoiceAnalysisError::Decode),
        }
    };
    drop(receiver);
    if stream_result.is_err() && child.try_wait().ok().flatten().is_none() {
        let _ = child.kill();
    }
    let status = child.wait().map_err(VoiceAnalysisError::Io)?;
    let _ = audio_thread.join();
    let _ = error_thread.join();
    if matches!(&stream_result, Err(VoiceAnalysisError::Cancelled)) {
        return Err(VoiceAnalysisError::Cancelled);
    }
    if !status.success() {
        return Err(VoiceAnalysisError::Decode);
    }
    stream_result
}

enum VoiceAudioRead {
    Data(Vec<u8>),
    End,
    Error(io::Error),
}

fn read_audio(mut stdout: impl Read, sender: std::sync::mpsc::SyncSender<VoiceAudioRead>) {
    let mut bytes = vec![0_u8; FRAMES_PER_CHUNK.saturating_mul(4)];
    loop {
        match stdout.read(&mut bytes) {
            Ok(0) => {
                let _ = sender.send(VoiceAudioRead::End);
                return;
            }
            Ok(read) => {
                if sender
                    .send(VoiceAudioRead::Data(bytes[..read].to_vec()))
                    .is_err()
                {
                    return;
                }
            }
            Err(error) => {
                let _ = sender.send(VoiceAudioRead::Error(error));
                return;
            }
        }
    }
}

fn drain(mut stderr: impl Read) {
    let mut bytes = [0_u8; 4_096];
    loop {
        match stderr.read(&mut bytes) {
            Ok(0) | Err(_) => return,
            Ok(_) => {}
        }
    }
}

struct PipelineOutput {
    predictions: Vec<[f32; 2]>,
    #[cfg(test)]
    emitted_frames: usize,
}

struct VoicePipeline {
    frame: [f32; FRAME_SIZE],
    filled: usize,
    frame_start: i64,
    total_samples: u64,
    emitted_frames: usize,
    mel_frames: VecDeque<[f32; MEL_BANDS]>,
    predictions: Vec<[f32; 2]>,
    preprocessor: MusicNnPreprocessor,
}

impl VoicePipeline {
    fn new() -> Result<Self, VoiceAnalysisError> {
        Ok(Self {
            frame: [0.0; FRAME_SIZE],
            // MusiCNN's centered first frame begins 256 samples before the
            // stream. Deterministic zero padding replaces Essentia's optional
            // random silence dither without changing non-silent recordings.
            filled: FRAME_SIZE / 2,
            frame_start: -(FRAME_SIZE as i64 / 2),
            total_samples: 0,
            emitted_frames: 0,
            mel_frames: VecDeque::with_capacity(PATCH_FRAMES),
            predictions: Vec::new(),
            preprocessor: MusicNnPreprocessor::new()?,
        })
    }

    fn add_sample(
        &mut self,
        sample: f32,
        model: &mut impl VoicePredictor,
        cancelled: &AtomicBool,
    ) -> Result<(), VoiceAnalysisError> {
        if self.total_samples >= u64::from(SAMPLE_RATE).saturating_mul(MAX_AUDIO_SECONDS) {
            return Err(VoiceAnalysisError::TooLong);
        }
        if self.total_samples.is_multiple_of(4_096) && cancelled.load(Ordering::Relaxed) {
            return Err(VoiceAnalysisError::Cancelled);
        }
        self.frame[self.filled] = sample;
        self.filled = self.filled.saturating_add(1);
        self.total_samples = self.total_samples.saturating_add(1);
        if self.filled == FRAME_SIZE {
            self.emit_frame(model, cancelled)?;
            self.advance_frame();
        }
        Ok(())
    }

    fn finish(
        mut self,
        model: &mut impl VoicePredictor,
        cancelled: &AtomicBool,
    ) -> Result<PipelineOutput, VoiceAnalysisError> {
        if self.total_samples == 0 {
            return Err(VoiceAnalysisError::Inference);
        }
        loop {
            self.frame[self.filled..].fill(0.0);
            self.emit_frame(model, cancelled)?;
            if self.frame_start.saturating_add(FRAME_HOP as i64)
                >= i64::try_from(self.total_samples).unwrap_or(i64::MAX)
            {
                break;
            }
            self.advance_frame();
        }
        if self.predictions.is_empty() {
            return Err(VoiceAnalysisError::Inference);
        }
        Ok(PipelineOutput {
            predictions: self.predictions,
            #[cfg(test)]
            emitted_frames: self.emitted_frames,
        })
    }

    fn emit_frame(
        &mut self,
        model: &mut impl VoicePredictor,
        cancelled: &AtomicBool,
    ) -> Result<(), VoiceAnalysisError> {
        if cancelled.load(Ordering::Relaxed) {
            return Err(VoiceAnalysisError::Cancelled);
        }
        let mel = self.preprocessor.transform(&self.frame);
        self.mel_frames.push_back(mel);
        self.emitted_frames = self.emitted_frames.saturating_add(1);
        if self.mel_frames.len() == PATCH_FRAMES {
            let mut patch = Vec::with_capacity(PATCH_FRAMES.saturating_mul(MEL_BANDS));
            for frame in &self.mel_frames {
                patch.extend_from_slice(frame);
            }
            self.predictions.push(model.predict(&patch)?);
            for _ in 0..PATCH_HOP {
                let _ = self.mel_frames.pop_front();
            }
        }
        Ok(())
    }

    fn advance_frame(&mut self) {
        self.frame.copy_within(FRAME_HOP..FRAME_SIZE, 0);
        self.filled = FRAME_SIZE - FRAME_HOP;
        self.frame_start = self.frame_start.saturating_add(FRAME_HOP as i64);
    }
}

struct MusicNnPreprocessor {
    fft: Arc<dyn Fft<f32>>,
    window: [f32; FRAME_SIZE],
    filters: Vec<[f32; SPECTRUM_BINS]>,
    spectrum: Vec<Complex<f32>>,
}

impl MusicNnPreprocessor {
    fn new() -> Result<Self, VoiceAnalysisError> {
        let mut planner = FftPlanner::<f32>::new();
        let fft = planner.plan_fft_forward(FRAME_SIZE);
        let window = std::array::from_fn(|index| {
            0.5 - 0.5 * (std::f32::consts::TAU * index as f32 / (FRAME_SIZE - 1) as f32).cos()
        });
        Ok(Self {
            fft,
            window,
            filters: mel_filter_bank()?,
            spectrum: vec![Complex::new(0.0, 0.0); FRAME_SIZE],
        })
    }

    fn transform(&mut self, frame: &[f32; FRAME_SIZE]) -> [f32; MEL_BANDS] {
        for (index, sample) in frame.iter().enumerate() {
            self.spectrum[index] = Complex::new(*sample * self.window[index], 0.0);
        }
        self.fft.process(&mut self.spectrum);
        let magnitudes =
            std::array::from_fn::<_, SPECTRUM_BINS, _>(|index| self.spectrum[index].norm());
        std::array::from_fn(|band| {
            let energy = magnitudes
                .iter()
                .zip(&self.filters[band])
                .map(|(magnitude, weight)| magnitude * magnitude * weight)
                .sum::<f32>();
            energy.mul_add(10_000.0, 1.0).log10()
        })
    }
}

fn mel_filter_bank() -> Result<Vec<[f32; SPECTRUM_BINS]>, VoiceAnalysisError> {
    let edges = mel_edges();
    let bin_hz = SAMPLE_RATE as f32 / FRAME_SIZE as f32;
    let mut filters = Vec::with_capacity(MEL_BANDS);
    for band in 0..MEL_BANDS {
        let left = edges[band];
        let center = edges[band + 1];
        let right = edges[band + 2];
        let rising = center - left;
        let falling = right - center;
        let area = (rising + falling) / 2.0;
        if rising <= 0.0 || falling <= 0.0 || area <= 0.0 {
            return Err(VoiceAnalysisError::Inference);
        }
        let mut coefficients = [0.0; SPECTRUM_BINS];
        let first = (left / bin_hz).ceil().max(0.0) as usize;
        let last = (right / bin_hz).floor().max(0.0) as usize;
        for (index, coefficient) in coefficients
            .iter_mut()
            .enumerate()
            .take(last.min(SPECTRUM_BINS - 1).saturating_add(1))
            .skip(first)
        {
            let frequency = index as f32 * bin_hz;
            let triangle = if frequency < center {
                (frequency - left) / rising
            } else {
                (right - frequency) / falling
            };
            *coefficient = triangle / area;
        }
        filters.push(coefficients);
    }
    Ok(filters)
}

fn mel_edges() -> [f32; MEL_BANDS + 2] {
    let low = hz_to_slaney_mel(0.0);
    let high = hz_to_slaney_mel(SAMPLE_RATE as f32 / 2.0);
    let increment = (high - low) / (MEL_BANDS + 1) as f32;
    let mut mel = low;
    std::array::from_fn(|_| {
        let frequency = slaney_mel_to_hz(mel);
        mel += increment;
        frequency
    })
}

fn hz_to_slaney_mel(frequency: f32) -> f32 {
    const MIN_LOG_HZ: f32 = 1_000.0;
    const LINEAR_SLOPE: f32 = 3.0 / 200.0;
    if frequency < MIN_LOG_HZ {
        frequency * LINEAR_SLOPE
    } else {
        const MIN_LOG_MEL: f32 = MIN_LOG_HZ * LINEAR_SLOPE;
        let log_step = 6.4_f32.ln() / 27.0;
        MIN_LOG_MEL + (frequency / MIN_LOG_HZ).ln() / log_step
    }
}

fn slaney_mel_to_hz(mel: f32) -> f32 {
    const MIN_LOG_HZ: f32 = 1_000.0;
    const LINEAR_SLOPE: f32 = 3.0 / 200.0;
    const MIN_LOG_MEL: f32 = MIN_LOG_HZ * LINEAR_SLOPE;
    if mel < MIN_LOG_MEL {
        mel / LINEAR_SLOPE
    } else {
        let log_step = 6.4_f32.ln() / 27.0;
        MIN_LOG_HZ * ((mel - MIN_LOG_MEL) * log_step).exp()
    }
}

trait VoicePredictor {
    fn predict(&mut self, patch: &[f32]) -> Result<[f32; 2], VoiceAnalysisError>;
}

struct TractVoiceModel {
    plan: Arc<TypedRunnableModel>,
}

impl TractVoiceModel {
    fn load(path: &Path) -> TractResult<Self> {
        let mut tensorflow = tract_tensorflow::tensorflow();
        tensorflow
            .op_register
            .insert("MusicMusiCnnPad", fixed_musicnn_pad);
        let mut graph = tensorflow.read_frozen_from_path(path)?;
        normalize_musicnn_graph(&mut graph)?;
        let mut model = tensorflow.model_for_proto_model(&graph)?;
        model.set_input_names(["model/Placeholder"])?;
        model.select_outputs_by_name(["model/Sigmoid"])?;
        model.set_input_fact(0, f32::fact([1, PATCH_FRAMES, MEL_BANDS]).into())?;
        let model = model.into_optimized()?.into_runnable()?;
        Ok(Self { plan: model })
    }
}

impl VoicePredictor for TractVoiceModel {
    fn predict(&mut self, patch: &[f32]) -> Result<[f32; 2], VoiceAnalysisError> {
        if patch.len() != PATCH_FRAMES.saturating_mul(MEL_BANDS) {
            return Err(VoiceAnalysisError::Inference);
        }
        let input = Tensor::from_shape(&[1, PATCH_FRAMES, MEL_BANDS], patch)
            .map_err(|_| VoiceAnalysisError::Inference)?;
        let outputs = self
            .plan
            .run(tvec![input.into()])
            .map_err(|_| VoiceAnalysisError::Inference)?;
        let output = outputs
            .first()
            .ok_or(VoiceAnalysisError::Inference)?
            .to_plain_array_view::<f32>()
            .map_err(|_| VoiceAnalysisError::Inference)?;
        let values = output.iter().copied().collect::<Vec<_>>();
        let pair = match values.as_slice() {
            [instrumental, voice]
                if instrumental.is_finite()
                    && voice.is_finite()
                    && (0.0..=1.0).contains(instrumental)
                    && (0.0..=1.0).contains(voice) =>
            {
                [*instrumental, *voice]
            }
            _ => return Err(VoiceAnalysisError::Inference),
        };
        Ok(pair)
    }
}

fn normalize_musicnn_graph(
    graph: &mut tract_tensorflow::tfpb::tensorflow::GraphDef,
) -> TractResult<()> {
    let pad_sources = graph
        .node
        .iter()
        .filter(|node| node.op == "Pad")
        .map(|node| {
            node.input
                .get(1)
                .cloned()
                .with_context(|| format!("MusiCNN Pad node {} has no paddings input", node.name))
        })
        .collect::<TractResult<Vec<_>>>()?;
    ensure!(
        pad_sources.len() == 4,
        "supported MusiCNN graph must contain exactly four fixed Pad nodes"
    );
    for source in &pad_sources {
        validate_musicnn_padding(graph, source)?;
    }
    for node in &mut graph.node {
        match node.op.as_str() {
            "Pad" => {
                node.op = "MusicMusiCnnPad".to_owned();
                node.input.truncate(1);
            }
            "FusedBatchNormV3" => node.op = "FusedBatchNorm".to_owned(),
            _ => {}
        }
    }
    Ok(())
}

fn validate_musicnn_padding(
    graph: &tract_tensorflow::tfpb::tensorflow::GraphDef,
    source: &str,
) -> TractResult<()> {
    let name = source
        .trim_start_matches('^')
        .split(':')
        .next()
        .with_context(|| "MusiCNN Pad source name is empty")?;
    let node = graph
        .node
        .iter()
        .find(|node| node.name == name)
        .with_context(|| format!("MusiCNN Pad source {name} is missing"))?;
    ensure!(node.op == "Const", "MusiCNN Pad source must be constant");
    let tensor = node.get_attr_tensor("value")?;
    ensure!(tensor.shape() == [4, 2], "MusiCNN Pad shape changed");
    let expected = [0_i64, 0, 3, 3, 0, 0, 0, 0];
    let values = if tensor.datum_type() == DatumType::I32 {
        tensor
            .try_as_plain()?
            .to_array_view::<i32>()?
            .iter()
            .map(|value| i64::from(*value))
            .collect::<Vec<_>>()
    } else if tensor.datum_type() == DatumType::I64 {
        tensor
            .try_as_plain()?
            .to_array_view::<i64>()?
            .iter()
            .copied()
            .collect()
    } else {
        bail!("MusiCNN Pad values have an unsupported type");
    };
    ensure!(values == expected, "MusiCNN Pad values changed");
    Ok(())
}

fn fixed_musicnn_pad(
    _context: &tract_tensorflow::model::ParsingContext,
    _node: &tract_tensorflow::tfpb::tensorflow::NodeDef,
) -> TractResult<Box<dyn InferenceOp>> {
    let op = tract_core::ops::array::Pad {
        pads: vec![(0, 0), (3, 3), (0, 0), (0, 0)],
        mode: tract_core::ops::array::PadMode::Constant(Arc::new(0.0_f32.into())),
    };
    Ok(inference_wrap(
        op,
        1,
        |_op, solver: &mut Solver<'_>, inputs: &[TensorProxy], outputs: &[TensorProxy]| {
            check_input_arity(inputs, 1)?;
            check_output_arity(outputs, 1)?;
            solver.equals(&outputs[0].datum_type, &inputs[0].datum_type)?;
            solver.equals(&inputs[0].rank, 4)?;
            solver.equals(&outputs[0].rank, 4)?;
            for (axis, extra) in [0_i64, 6, 0, 0].into_iter().enumerate() {
                solver.equals(
                    &outputs[0].shape[axis],
                    inputs[0].shape[axis].bex() + extra.to_dim(),
                )?;
            }
            Ok(())
        },
    ))
}

fn summarize_predictions(predictions: &[[f32; 2]]) -> Result<(f64, f64), VoiceAnalysisError> {
    let mut score_total = 0.0_f64;
    let mut voice_leading = 0_usize;
    let mut valid = 0_usize;
    for [instrumental, voice] in predictions {
        if !instrumental.is_finite()
            || !voice.is_finite()
            || !(0.0..=1.0).contains(instrumental)
            || !(0.0..=1.0).contains(voice)
        {
            continue;
        }
        let total = f64::from(*instrumental) + f64::from(*voice);
        if total <= 1e-9 {
            continue;
        }
        let score = f64::from(*voice) / total;
        score_total += score;
        voice_leading = voice_leading.saturating_add(usize::from(score >= 0.5));
        valid = valid.saturating_add(1);
    }
    if valid == 0 {
        return Err(VoiceAnalysisError::Inference);
    }
    Ok((
        score_total / valid as f64,
        voice_leading as f64 / valid as f64,
    ))
}

fn classification_note(voice_score: f64, vocal_coverage: f64) -> String {
    let label = if voice_score >= 0.65 && vocal_coverage >= 0.6 {
        "Voice is present across most analyzed windows."
    } else if voice_score >= 0.55 && vocal_coverage >= 0.2 {
        "Voice is present in part of the recording."
    } else if voice_score <= 0.35 && vocal_coverage <= 0.2 {
        "The recording is predominantly instrumental."
    } else {
        "The classifier found mixed or uncertain voice evidence."
    };
    format!(
        "{label} Mean normalized voice score {:.0}%; voice-leading window coverage {:.0}%.",
        voice_score * 100.0,
        vocal_coverage * 100.0,
    )
}

fn sha256_file(path: &Path) -> io::Result<String> {
    let mut source = File::open(path)?;
    let mut digest = Sha256::new();
    let mut bytes = vec![0_u8; 1024 * 1024];
    loop {
        let read = source.read(&mut bytes)?;
        if read == 0 {
            break;
        }
        digest.update(&bytes[..read]);
    }
    Ok(format!("{:x}", digest.finalize()))
}

fn round_five(value: f64) -> f64 {
    (value * 100_000.0).round_ties_even() / 100_000.0
}

fn object(value: Value) -> Map<String, Value> {
    value.as_object().cloned().unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use std::f32::consts::TAU;
    use std::io::Write;

    use super::*;

    #[derive(Default)]
    struct FixedPredictor {
        calls: usize,
    }

    impl VoicePredictor for FixedPredictor {
        fn predict(&mut self, patch: &[f32]) -> Result<[f32; 2], VoiceAnalysisError> {
            assert_eq!(patch.len(), PATCH_FRAMES * MEL_BANDS);
            self.calls = self.calls.saturating_add(1);
            Ok([0.25, 0.75])
        }
    }

    #[test]
    fn disabled_and_invalid_model_statuses_are_explicit_and_path_free() -> Result<(), Box<dyn Error>>
    {
        let disabled = VoiceBackend::initialize(None, "ffmpeg");
        assert_eq!(disabled.status.status, "not_configured");
        assert_eq!(disabled.status.reason, None);
        assert!(disabled.worker.is_none());

        let directory = tempfile::tempdir()?;
        let missing_path = directory.path().join(VOICE_MODEL_FILENAME);
        let missing = VoiceBackend::initialize(Some(&missing_path), "ffmpeg");
        assert_eq!(missing.status.reason.as_deref(), Some("model_missing"));
        assert!(!format!("{:?}", missing.status).contains(&directory.path().display().to_string()));

        let unsupported_path = directory.path().join("unsupported.pb");
        std::fs::write(&unsupported_path, b"not the supported model")?;
        let unsupported = VoiceBackend::initialize(Some(&unsupported_path), "ffmpeg");
        assert_eq!(
            unsupported.status.reason.as_deref(),
            Some("unsupported_model")
        );
        let signature = unsupported
            .status
            .source_signature
            .as_deref()
            .ok_or("configured unavailable backend has no source signature")?;
        assert!(!signature.contains(&unsupported_path.display().to_string()));
        Ok(())
    }

    #[test]
    fn predictions_use_the_legacy_bounded_normalization_contract() -> Result<(), VoiceAnalysisError>
    {
        let (score, coverage) = summarize_predictions(&[
            [0.1, 0.9],
            [0.3, 0.7],
            [0.8, 0.2],
            [0.0, 0.0],
            [f32::NAN, 0.5],
        ])?;
        assert!((score - 0.6).abs() < 1e-6);
        assert!((coverage - 2.0 / 3.0).abs() < 1e-6);
        let document = VoiceAnalysisDocument::classified(score, coverage, 3, 1.0);
        assert_eq!(document.summary["voice_probability"], 0.6);
        assert_eq!(document.summary["vocal_coverage"], 0.666_67);
        assert!(
            document.summary["note"]
                .as_str()
                .is_some_and(|note| note.contains("Mean normalized voice score 60%"))
        );
        assert_eq!(document.stage["prediction_windows"], 3);
        Ok(())
    }

    #[test]
    fn classification_notes_preserve_conservative_thresholds() {
        assert!(classification_note(0.65, 0.6).starts_with("Voice is present across most"));
        assert!(classification_note(0.55, 0.2).starts_with("Voice is present in part"));
        assert!(classification_note(0.35, 0.2).starts_with("The recording is predominantly"));
        assert!(classification_note(0.5, 0.5).starts_with("The classifier found mixed"));
    }

    #[test]
    fn slaney_scale_round_trips_and_edges_are_strictly_increasing() {
        for frequency in [0.0, 100.0, 999.0, 1_000.0, 4_000.0, 8_000.0] {
            let round_trip = slaney_mel_to_hz(hz_to_slaney_mel(frequency));
            assert!((round_trip - frequency).abs() < 0.01);
        }
        let edges = mel_edges();
        assert!((edges[0] - 0.0).abs() < f32::EPSILON);
        assert!((edges[MEL_BANDS + 1] - 8_000.0).abs() < 0.02);
        assert!(edges.windows(2).all(|edge| edge[0] < edge[1]));
    }

    #[test]
    fn preprocessing_maps_silence_to_zero_and_tone_to_its_filter() -> Result<(), VoiceAnalysisError>
    {
        let mut preprocessor = MusicNnPreprocessor::new()?;
        let silence = preprocessor.transform(&[0.0; FRAME_SIZE]);
        assert_eq!(silence, [0.0; MEL_BANDS]);

        let tone =
            std::array::from_fn(|index| (TAU * 1_000.0 * index as f32 / SAMPLE_RATE as f32).sin());
        let bands = preprocessor.transform(&tone);
        let strongest = bands
            .iter()
            .enumerate()
            .max_by(|left, right| left.1.total_cmp(right.1))
            .map(|(index, _)| index)
            .ok_or(VoiceAnalysisError::Inference)?;
        let edges = mel_edges();
        assert!(edges[strongest] <= 1_000.0);
        assert!(edges[strongest + 2] >= 1_000.0);
        Ok(())
    }

    #[test]
    fn centered_frames_and_overlapping_patches_match_musicnn_counts()
    -> Result<(), VoiceAnalysisError> {
        let sample_count = 100_000_usize;
        let expected_frames = sample_count.div_ceil(FRAME_HOP).saturating_add(1);
        let expected_patches = expected_frames
            .saturating_sub(PATCH_FRAMES)
            .checked_div(PATCH_HOP)
            .unwrap_or_default()
            .saturating_add(1);
        let cancelled = AtomicBool::new(false);
        let mut model = FixedPredictor::default();
        let mut pipeline = VoicePipeline::new()?;
        for index in 0..sample_count {
            let sample = (TAU * 440.0 * index as f32 / SAMPLE_RATE as f32).sin() * 0.2;
            pipeline.add_sample(sample, &mut model, &cancelled)?;
        }
        let output = pipeline.finish(&mut model, &cancelled)?;
        assert_eq!(output.emitted_frames, expected_frames);
        assert_eq!(output.predictions.len(), expected_patches);
        assert_eq!(model.calls, expected_patches);
        Ok(())
    }

    #[test]
    fn official_graph_compatibility_is_exercised_when_the_model_is_available()
    -> Result<(), Box<dyn Error>> {
        let Some(path) = std::env::var_os("MUSIC_TEST_VOICE_MODEL").map(PathBuf::from) else {
            return Ok(());
        };
        assert_eq!(sha256_file(&path)?, VOICE_MODEL_SHA256);
        let mut model = TractVoiceModel::load(&path)?;
        let prediction = model.predict(&vec![0.0; PATCH_FRAMES * MEL_BANDS])?;
        assert!((prediction[0] - 0.378_066).abs() < 0.000_1);
        assert!((prediction[1] - 0.338_944_23).abs() < 0.000_1);
        Ok(())
    }

    #[tokio::test]
    async fn configured_worker_decodes_and_classifies_when_tools_are_available()
    -> Result<(), Box<dyn Error>> {
        let Some(model_path) = std::env::var_os("MUSIC_TEST_VOICE_MODEL").map(PathBuf::from) else {
            return Ok(());
        };
        let Some(ffmpeg) = std::env::var_os("MUSIC_TEST_FFMPEG").map(PathBuf::from) else {
            return Ok(());
        };
        let directory = tempfile::tempdir()?;
        let track_path = directory.path().join("voice-worker.wav");
        write_tone_wav(&track_path, 4)?;
        let backend = VoiceBackend::initialize(Some(&model_path), ffmpeg);
        assert_eq!(backend.status.status, "ready");
        let signature = backend
            .status
            .source_signature
            .as_deref()
            .ok_or("ready backend has no source signature")?;
        assert!(!signature.contains(&model_path.display().to_string()));
        let worker = backend.worker.ok_or("ready backend has no worker")?;
        let document = worker
            .analyze(track_path, Arc::new(AtomicBool::new(false)))
            .await?;
        assert_eq!(document.summary["status"], "classified");
        assert_eq!(document.stage["status"], "complete");
        assert!(document.prediction_windows >= 1);
        assert!(document.elapsed_seconds > 0.0);
        Ok(())
    }

    fn write_tone_wav(path: &Path, seconds: u32) -> io::Result<()> {
        let sample_count = seconds.saturating_mul(SAMPLE_RATE);
        let data_bytes = sample_count.saturating_mul(2);
        let mut output = File::create(path)?;
        output.write_all(b"RIFF")?;
        output.write_all(&(36_u32.saturating_add(data_bytes)).to_le_bytes())?;
        output.write_all(b"WAVEfmt ")?;
        output.write_all(&16_u32.to_le_bytes())?;
        output.write_all(&1_u16.to_le_bytes())?;
        output.write_all(&1_u16.to_le_bytes())?;
        output.write_all(&SAMPLE_RATE.to_le_bytes())?;
        output.write_all(&SAMPLE_RATE.saturating_mul(2).to_le_bytes())?;
        output.write_all(&2_u16.to_le_bytes())?;
        output.write_all(&16_u16.to_le_bytes())?;
        output.write_all(b"data")?;
        output.write_all(&data_bytes.to_le_bytes())?;
        for index in 0..sample_count {
            let value = (0.2
                * (TAU * 440.0 * index as f32 / SAMPLE_RATE as f32).sin()
                * f32::from(i16::MAX))
            .round() as i16;
            output.write_all(&value.to_le_bytes())?;
        }
        Ok(())
    }
}
