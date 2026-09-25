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
use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};
use tokio::sync::{mpsc, oneshot};
use tract_tensorflow::prelude::*;
use tract_tensorflow::tract_hir::internal::{
    InferenceOp, IntoExp, Solver, TensorProxy, TractErrorContext, bail, check_input_arity,
    check_output_arity, ensure, inference_wrap,
};

use crate::decoder_process::{DecoderWaitError, wait_for_decoder};
use crate::musicnn::{FRAME_SIZE, MEL_BANDS, MusicNnPreprocessor, SAMPLE_RATE};

const FRAME_HOP: usize = 256;
const PATCH_FRAMES: usize = 187;
const PATCH_HOP: usize = 93;
const MAX_AUDIO_SECONDS: u64 = 24 * 60 * 60;
const FRAMES_PER_CHUNK: usize = 8_192;
const VOICE_REQUEST_CAPACITY: usize = 1;
const VOICE_ANALYSIS_TIMEOUT: Duration = Duration::from_secs(30 * 60);
// The only supported graph is about 3.1 MiB; bound input before protobuf parsing.
const MAX_VOICE_MODEL_BYTES: u64 = 4 * 1_024 * 1_024;
const TRACT_RUNTIME_ID: &str =
    "tract-tensorflow/0.23.7+musicnn-compat/v1+preprocess/v1+decode/v2+windows/v2+artifact/v2";

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
                // A normalized model score, not a calibrated probability.
                "voice_score": round_five(voice_score),
                "vocal_coverage": round_five(vocal_coverage),
                "analyzed_windows": prediction_windows,
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
                "voice_score": null,
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
    DeadlineExceeded,
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
            Self::DeadlineExceeded => "TimeoutError",
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
            Self::DeadlineExceeded => "voice analysis exceeded its 30-minute deadline",
            Self::Cancelled => "voice analysis was cancelled",
            Self::Inference => "voice classifier inference failed",
            Self::WorkerUnavailable => "voice-analysis worker is unavailable",
        })
    }
}

impl From<DecoderWaitError> for VoiceAnalysisError {
    fn from(error: DecoderWaitError) -> Self {
        match error {
            DecoderWaitError::Cancelled => Self::Cancelled,
            DecoderWaitError::DeadlineExceeded => Self::DeadlineExceeded,
            DecoderWaitError::Io(error) => Self::Io(error),
        }
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
    pub worker_factory: Option<VoiceWorkerFactory>,
}

impl VoiceBackend {
    #[must_use]
    pub fn initialize(model_path: Option<&Path>, ffmpeg: impl Into<PathBuf>) -> Self {
        let Some(model_path) = model_path else {
            return Self {
                status: VoiceAnalyzerStatus::not_configured(),
                worker_factory: None,
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
        let worker_factory = VoiceWorkerFactory {
            model_path: model_path.to_owned(),
            ffmpeg: ffmpeg.into(),
        };
        if worker_factory.start().is_err() {
            return unavailable_backend("runtime_missing", &model_hash);
        }
        let signature = format!("{VOICE_ANALYZER_ID}:{model_hash}:{TRACT_RUNTIME_ID}");
        Self {
            status: VoiceAnalyzerStatus::ready(signature),
            worker_factory: Some(worker_factory),
        }
    }
}

fn unavailable_backend(reason: &'static str, model_identity: &str) -> VoiceBackend {
    let signature = format!("{VOICE_ANALYZER_ID}:{model_identity}:{TRACT_RUNTIME_ID}:{reason}");
    VoiceBackend {
        status: VoiceAnalyzerStatus::unavailable_with_signature(reason, signature),
        worker_factory: None,
    }
}

#[derive(Debug, Clone)]
/// Creates job-scoped voice workers after startup readiness has been verified.
pub struct VoiceWorkerFactory {
    model_path: PathBuf,
    ffmpeg: PathBuf,
}

impl VoiceWorkerFactory {
    /// Start one model-owning worker for a bounded voice-analysis pass.
    /// Dropping the returned worker joins its thread and releases the compiled
    /// graph instead of retaining model memory for the server's lifetime.
    pub fn start(&self) -> Result<VoiceWorker, VoiceAnalysisError> {
        VoiceWorker::start(self.model_path.clone(), self.ffmpeg.clone())
            .map_err(|()| VoiceAnalysisError::WorkerUnavailable)
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
    let (voice_score, vocal_coverage) = output.summary.means()?;
    Ok(VoiceAnalysisDocument::classified(
        voice_score,
        vocal_coverage,
        output.summary.windows,
        started.elapsed().as_secs_f64(),
    ))
}

fn decode_and_predict(
    model: &mut impl VoicePredictor,
    executable: &Path,
    path: &Path,
    cancelled: &AtomicBool,
) -> Result<PipelineOutput, VoiceAnalysisError> {
    decode_and_predict_until(
        model,
        executable,
        path,
        cancelled,
        Instant::now() + VOICE_ANALYSIS_TIMEOUT,
    )
}

fn decode_and_predict_until(
    model: &mut impl VoicePredictor,
    executable: &Path,
    path: &Path,
    cancelled: &AtomicBool,
    deadline: Instant,
) -> Result<PipelineOutput, VoiceAnalysisError> {
    if !path.is_file() {
        return Err(VoiceAnalysisError::MissingFile);
    }
    let mut command = Command::new(executable);
    command
        .arg("-v")
        .arg("error")
        .arg("-nostdin")
        .arg("-filter_threads")
        .arg("1")
        .arg("-filter_complex_threads")
        .arg("1")
        .arg("-threads")
        .arg("1")
        .arg("-i")
        .arg(path)
        .arg("-map")
        .arg("0:a:0")
        .arg("-vn")
        // Floating-point FFmpeg downmix otherwise boosts in-phase stereo by 3 dB.
        // Normalize the matrix to match the reference MonoMixer for mono/stereo.
        .arg("-af")
        .arg(format!(
            "aresample={SAMPLE_RATE}:out_chlayout=mono:rematrix_maxval=1"
        ))
        .arg("-ac")
        .arg("1")
        .arg("-ar")
        .arg(SAMPLE_RATE.to_string())
        .arg("-f")
        .arg("f32le")
        .arg("-acodec")
        .arg("pcm_f32le")
        .arg("-threads")
        .arg("1")
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
    let mut pipeline = VoicePipeline::new(deadline);
    let mut pending = Vec::with_capacity(3);
    let stream_result = 'stream: loop {
        if cancelled.load(Ordering::Relaxed) {
            break Err(VoiceAnalysisError::Cancelled);
        }
        if Instant::now() >= deadline {
            break Err(VoiceAnalysisError::DeadlineExceeded);
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
    let status =
        wait_for_decoder(&mut child, deadline, cancelled).map_err(VoiceAnalysisError::from);
    let _ = audio_thread.join();
    let _ = error_thread.join();
    match stream_result {
        Err(error) => Err(error),
        Ok(output) if status?.success() => Ok(output),
        Ok(_) => Err(VoiceAnalysisError::Decode),
    }
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
    summary: PredictionSummary,
    #[cfg(test)]
    emitted_frames: usize,
}

struct VoicePipeline {
    frame: [f32; FRAME_SIZE],
    filled: usize,
    frame_start: i64,
    total_samples: u64,
    emitted_frames: usize,
    last_predicted_frame: usize,
    mel_frames: VecDeque<[f32; MEL_BANDS]>,
    patch: Vec<f32>,
    summary: PredictionSummary,
    preprocessor: MusicNnPreprocessor,
    deadline: Instant,
}

impl VoicePipeline {
    fn new(deadline: Instant) -> Self {
        Self {
            frame: [0.0; FRAME_SIZE],
            // Center the first frame before the stream with deterministic zero padding.
            filled: FRAME_SIZE / 2,
            frame_start: -(FRAME_SIZE as i64 / 2),
            total_samples: 0,
            emitted_frames: 0,
            last_predicted_frame: 0,
            mel_frames: VecDeque::with_capacity(PATCH_FRAMES),
            patch: Vec::with_capacity(PATCH_FRAMES * MEL_BANDS),
            summary: PredictionSummary::default(),
            preprocessor: MusicNnPreprocessor::new(),
            deadline,
        }
    }

    fn check_control(&self, cancelled: &AtomicBool) -> Result<(), VoiceAnalysisError> {
        if cancelled.load(Ordering::Relaxed) {
            Err(VoiceAnalysisError::Cancelled)
        } else if Instant::now() >= self.deadline {
            Err(VoiceAnalysisError::DeadlineExceeded)
        } else {
            Ok(())
        }
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
        if !sample.is_finite() {
            return Err(VoiceAnalysisError::Decode);
        }
        if self.total_samples.is_multiple_of(4_096) {
            self.check_control(cancelled)?;
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
        self.check_control(cancelled)?;
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
        if self.mel_frames.len() == PATCH_FRAMES && self.last_predicted_frame != self.emitted_frames
        {
            // Anchor one full window at the ending. Retain actual preceding frames
            // rather than repeating a short tail, and never duplicate an aligned window.
            self.predict_window(model, cancelled)?;
        }
        if self.summary.windows == 0 {
            return Err(VoiceAnalysisError::Inference);
        }
        self.check_control(cancelled)?;
        Ok(PipelineOutput {
            summary: self.summary,
            #[cfg(test)]
            emitted_frames: self.emitted_frames,
        })
    }

    fn emit_frame(
        &mut self,
        model: &mut impl VoicePredictor,
        cancelled: &AtomicBool,
    ) -> Result<(), VoiceAnalysisError> {
        self.check_control(cancelled)?;
        if self.mel_frames.len() == PATCH_FRAMES {
            let _ = self.mel_frames.pop_front();
        }
        self.mel_frames
            .push_back(self.preprocessor.transform(&self.frame));
        self.emitted_frames = self.emitted_frames.saturating_add(1);
        if self.emitted_frames >= PATCH_FRAMES
            && (self.emitted_frames - PATCH_FRAMES).is_multiple_of(PATCH_HOP)
        {
            self.predict_window(model, cancelled)?;
        }
        Ok(())
    }

    fn predict_window(
        &mut self,
        model: &mut impl VoicePredictor,
        cancelled: &AtomicBool,
    ) -> Result<(), VoiceAnalysisError> {
        self.check_control(cancelled)?;
        self.patch.clear();
        for frame in &self.mel_frames {
            self.patch.extend_from_slice(frame);
        }
        let prediction = model.predict(&self.patch)?;
        // Cancellation or expiry during inference cannot produce a complete result.
        self.check_control(cancelled)?;
        self.summary.add(prediction)?;
        self.last_predicted_frame = self.emitted_frames;
        Ok(())
    }

    fn advance_frame(&mut self) {
        self.frame.copy_within(FRAME_HOP..FRAME_SIZE, 0);
        self.filled = FRAME_SIZE - FRAME_HOP;
        self.frame_start = self.frame_start.saturating_add(FRAME_HOP as i64);
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
        let mut graph = {
            // Hash and parse one immutable snapshot. Reopening or mapping the path
            // after verification could execute different bytes under the pinned identity.
            let bytes = read_voice_model_bytes(File::open(path)?)?;
            ensure!(
                format!("{:x}", Sha256::digest(&bytes)) == VOICE_MODEL_SHA256,
                "voice model checksum does not match the supported artifact"
            );
            tensorflow.read_frozen_model(&mut bytes.as_slice())?
        };
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

#[derive(Default)]
struct PredictionSummary {
    score_total: f64,
    voice_leading: usize,
    windows: usize,
}

impl PredictionSummary {
    fn add(&mut self, [instrumental, voice]: [f32; 2]) -> Result<(), VoiceAnalysisError> {
        if !instrumental.is_finite()
            || !voice.is_finite()
            || !(0.0..=1.0).contains(&instrumental)
            || !(0.0..=1.0).contains(&voice)
        {
            return Err(VoiceAnalysisError::Inference);
        }
        let total = f64::from(instrumental) + f64::from(voice);
        if total <= 1e-9 {
            return Err(VoiceAnalysisError::Inference);
        }
        let score = f64::from(voice) / total;
        self.score_total += score;
        self.voice_leading = self.voice_leading.saturating_add(usize::from(score >= 0.5));
        self.windows = self.windows.saturating_add(1);
        Ok(())
    }

    fn means(&self) -> Result<(f64, f64), VoiceAnalysisError> {
        if self.windows == 0 {
            return Err(VoiceAnalysisError::Inference);
        }
        // These remain window statistics, not calibrated probabilities or vocal seconds.
        Ok((
            self.score_total / self.windows as f64,
            self.voice_leading as f64 / self.windows as f64,
        ))
    }
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

fn read_voice_model_bytes(source: impl Read) -> io::Result<Vec<u8>> {
    let mut bytes = Vec::new();
    source
        .take(MAX_VOICE_MODEL_BYTES + 1)
        .read_to_end(&mut bytes)?;
    if bytes.len() as u64 > MAX_VOICE_MODEL_BYTES {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "voice model exceeds the supported artifact size limit",
        ));
    }
    Ok(bytes)
}

fn sha256_file(path: &Path) -> io::Result<String> {
    let bytes = read_voice_model_bytes(File::open(path)?)?;
    Ok(format!("{:x}", Sha256::digest(&bytes)))
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
        assert!(disabled.worker_factory.is_none());

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
        assert!(signature.contains("+artifact/v2:"));
        Ok(())
    }

    #[test]
    fn predictions_use_the_bounded_normalization_contract() -> Result<(), VoiceAnalysisError> {
        let mut summary = PredictionSummary::default();
        for prediction in [[0.1, 0.9], [0.3, 0.7], [0.8, 0.2]] {
            summary.add(prediction)?;
        }
        let (score, coverage) = summary.means()?;
        assert!((score - 0.6).abs() < 1e-6);
        assert!((coverage - 2.0 / 3.0).abs() < 1e-6);
        let document = VoiceAnalysisDocument::classified(score, coverage, 3, 1.0);
        assert_eq!(document.summary["voice_score"], 0.6);
        assert_eq!(document.summary["vocal_coverage"], 0.666_67);
        assert!(
            document.summary["note"]
                .as_str()
                .is_some_and(|note| note.contains("Mean normalized voice score 60%"))
        );
        assert_eq!(document.stage["prediction_windows"], 3);
        assert_eq!(document.summary["analyzed_windows"], 3);
        Ok(())
    }

    #[derive(Default)]
    struct RecordingPredictor {
        patches: Vec<Vec<f32>>,
    }

    impl VoicePredictor for RecordingPredictor {
        fn predict(&mut self, patch: &[f32]) -> Result<[f32; 2], VoiceAnalysisError> {
            assert_eq!(patch.len(), PATCH_FRAMES * MEL_BANDS);
            self.patches.push(patch.to_vec());
            Ok([0.75, 0.25])
        }
    }

    #[test]
    fn the_ending_reaches_a_final_full_prediction_window() -> Result<(), VoiceAnalysisError> {
        let cancelled = AtomicBool::new(false);
        let mut model = RecordingPredictor::default();
        let mut pipeline = VoicePipeline::new(Instant::now() + VOICE_ANALYSIS_TIMEOUT);
        let sample_count = 64_000;
        for index in 0..sample_count {
            let sample = if index < 48_000 {
                0.0
            } else {
                0.5 * (TAU * 4_000.0 * index as f32 / SAMPLE_RATE as f32).sin()
            };
            pipeline.add_sample(sample, &mut model, &cancelled)?;
        }
        pipeline.finish(&mut model, &cancelled)?;
        assert_eq!(
            model.patches.len(),
            2,
            "the regular window misses the final second"
        );
        assert!(model.patches[0].iter().all(|value| *value == 0.0));
        let expected_final_frame = std::array::from_fn(|index| {
            if index < FRAME_HOP {
                let sample_index = sample_count - FRAME_HOP + index;
                0.5 * (TAU * 4_000.0 * sample_index as f32 / SAMPLE_RATE as f32).sin()
            } else {
                0.0
            }
        });
        let expected = MusicNnPreprocessor::new().transform(&expected_final_frame);
        let actual = &model.patches[1][(PATCH_FRAMES - 1) * MEL_BANDS..];
        assert_eq!(actual, expected);
        assert!(actual.iter().any(|value| *value > 1.0));
        Ok(())
    }

    #[test]
    fn tail_windows_are_complete_and_not_duplicated_at_patch_boundaries()
    -> Result<(), VoiceAnalysisError> {
        let cancelled = AtomicBool::new(false);
        // Fixed expectations include the centered first/last frames.
        for (samples, windows) in [
            (0, 0),
            (256, 0),
            (47_360, 0),
            (47_361, 1),
            (47_616, 1),
            (47_617, 2),
            (64_000, 2),
            (71_424, 2),
            (71_425, 3),
            (95_232, 3),
            (95_233, 4),
        ] {
            let mut model = FixedPredictor::default();
            let mut pipeline = VoicePipeline::new(Instant::now() + VOICE_ANALYSIS_TIMEOUT);
            for _ in 0..samples {
                pipeline.add_sample(0.25, &mut model, &cancelled)?;
                assert!(pipeline.mel_frames.len() <= PATCH_FRAMES);
            }
            let result = pipeline.finish(&mut model, &cancelled);
            assert_eq!(model.calls, windows, "sample count {samples}");
            if windows == 0 {
                assert!(matches!(result, Err(VoiceAnalysisError::Inference)));
            } else {
                result?;
            }
        }
        Ok(())
    }

    #[test]
    fn invalid_predictions_never_become_a_partial_voice_summary() -> Result<(), VoiceAnalysisError>
    {
        for invalid in [
            [f32::NAN, 0.5],
            [0.5, f32::INFINITY],
            [-0.1, 0.5],
            [0.5, 1.1],
            [0.0, 0.0],
            [1e-12, 1e-12],
        ] {
            let mut summary = PredictionSummary::default();
            summary.add([0.2, 0.8])?;
            assert!(matches!(
                summary.add(invalid),
                Err(VoiceAnalysisError::Inference)
            ));
            assert_eq!(summary.windows, 1);
        }
        assert!(matches!(
            PredictionSummary::default().means(),
            Err(VoiceAnalysisError::Inference)
        ));

        struct InvalidTail {
            calls: usize,
        }
        impl VoicePredictor for InvalidTail {
            fn predict(&mut self, _: &[f32]) -> Result<[f32; 2], VoiceAnalysisError> {
                self.calls += 1;
                Ok(if self.calls == 1 {
                    [0.8, 0.2]
                } else {
                    [f32::NAN, 0.5]
                })
            }
        }
        let cancelled = AtomicBool::new(false);
        let mut model = InvalidTail { calls: 0 };
        let mut pipeline = VoicePipeline::new(Instant::now() + VOICE_ANALYSIS_TIMEOUT);
        for _ in 0..64_000 {
            pipeline.add_sample(0.0, &mut model, &cancelled)?;
        }
        let error = pipeline
            .finish(&mut model, &cancelled)
            .err()
            .ok_or(VoiceAnalysisError::Inference)?;
        assert!(matches!(error, VoiceAnalysisError::Inference));
        assert_eq!(model.calls, 2);
        let unavailable = VoiceAnalysisDocument::unavailable(&error, 1.0);
        assert_eq!(unavailable.summary["status"], "unavailable");
        assert!(unavailable.summary["voice_score"].is_null());
        assert_eq!(unavailable.prediction_windows, 0);
        Ok(())
    }

    struct CancellingPredictor<'a> {
        calls: usize,
        cancel_on: usize,
        cancelled: &'a AtomicBool,
    }

    impl VoicePredictor for CancellingPredictor<'_> {
        fn predict(&mut self, _: &[f32]) -> Result<[f32; 2], VoiceAnalysisError> {
            self.calls += 1;
            if self.calls == self.cancel_on {
                self.cancelled.store(true, Ordering::Relaxed);
            }
            Ok([0.2, 0.8])
        }
    }

    #[test]
    fn cancellation_during_the_tail_and_expiry_before_it_cannot_complete()
    -> Result<(), VoiceAnalysisError> {
        let cancelled = AtomicBool::new(false);
        let mut model = CancellingPredictor {
            calls: 0,
            cancel_on: 2,
            cancelled: &cancelled,
        };
        let mut pipeline = VoicePipeline::new(Instant::now() + VOICE_ANALYSIS_TIMEOUT);
        for _ in 0..64_000 {
            pipeline.add_sample(0.0, &mut model, &cancelled)?;
        }
        assert_eq!(model.calls, 1);
        assert!(matches!(
            pipeline.finish(&mut model, &cancelled),
            Err(VoiceAnalysisError::Cancelled)
        ));
        assert_eq!(model.calls, 2);

        let cancelled = AtomicBool::new(false);
        let mut model = FixedPredictor::default();
        let mut pipeline = VoicePipeline::new(Instant::now() + VOICE_ANALYSIS_TIMEOUT);
        for _ in 0..64_000 {
            pipeline.add_sample(0.0, &mut model, &cancelled)?;
        }
        pipeline.deadline = Instant::now();
        assert!(matches!(
            pipeline.finish(&mut model, &cancelled),
            Err(VoiceAnalysisError::DeadlineExceeded)
        ));
        assert_eq!(model.calls, 1);
        Ok(())
    }

    #[test]
    fn non_finite_decoded_audio_is_rejected() {
        for sample in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY] {
            let mut pipeline = VoicePipeline::new(Instant::now() + VOICE_ANALYSIS_TIMEOUT);
            let mut model = FixedPredictor::default();
            assert!(matches!(
                pipeline.add_sample(sample, &mut model, &AtomicBool::new(false)),
                Err(VoiceAnalysisError::Decode)
            ));
            assert_eq!(model.calls, 0);
        }
    }

    fn test_ffmpeg() -> Option<PathBuf> {
        if let Some(path) = std::env::var_os("MUSIC_TEST_FFMPEG") {
            return Some(path.into());
        }
        if Command::new("ffmpeg")
            .arg("-version")
            .output()
            .is_ok_and(|output| output.status.success())
        {
            Some(PathBuf::from("ffmpeg"))
        } else {
            eprintln!("FFmpeg decoder acceptance was not exercised: executable unavailable");
            None
        }
    }

    #[test]
    fn decoder_preserves_native_samples_and_the_ending_when_available() -> Result<(), Box<dyn Error>>
    {
        let Some(ffmpeg) = test_ffmpeg() else {
            return Ok(());
        };
        let directory = tempfile::tempdir()?;
        let path = directory.path().join("ending.wav");
        let samples: Vec<i16> = (0..64_000)
            .map(|index| {
                if index < 48_000 {
                    0
                } else {
                    (16_384.0 * (TAU * 4_000.0 * index as f32 / SAMPLE_RATE as f32).sin()).round()
                        as i16
                }
            })
            .collect();
        write_pcm_wav(&path, SAMPLE_RATE, 1, &samples)?;
        let mut model = RecordingPredictor::default();
        let output = decode_and_predict(&mut model, &ffmpeg, &path, &AtomicBool::new(false))?;
        assert_eq!(output.emitted_frames, 251);
        assert_eq!(output.summary.windows, 2);
        assert!(model.patches[0].iter().all(|value| *value == 0.0));
        let frame = std::array::from_fn(|index| {
            if index < FRAME_HOP {
                f32::from(samples[samples.len() - FRAME_HOP + index]) / 32_768.0
            } else {
                0.0
            }
        });
        let expected = MusicNnPreprocessor::new().transform(&frame);
        assert_eq!(
            &model.patches[1][(PATCH_FRAMES - 1) * MEL_BANDS..],
            expected
        );
        Ok(())
    }

    #[test]
    fn decoder_downmixes_and_resamples_stereo_when_available() -> Result<(), Box<dyn Error>> {
        let Some(ffmpeg) = test_ffmpeg() else {
            return Ok(());
        };
        let directory = tempfile::tempdir()?;
        let path = directory.path().join("stereo.wav");
        let expected_dc = MusicNnPreprocessor::new().transform(&[0.25; FRAME_SIZE]);
        for (rate, opposite_phase) in [(48_000, false), (44_100, false), (44_100, true)] {
            let samples: Vec<i16> = (0..rate * 4)
                .flat_map(|_| [8_192, if opposite_phase { -8_192 } else { 8_192 }])
                .collect();
            write_pcm_wav(&path, rate, 2, &samples)?;
            let mut model = RecordingPredictor::default();
            let output = decode_and_predict(&mut model, &ffmpeg, &path, &AtomicBool::new(false))?;
            assert_eq!(output.emitted_frames, 251, "rate {rate}");
            assert_eq!(output.summary.windows, 2);
            if opposite_phase {
                assert!(
                    model
                        .patches
                        .iter()
                        .flatten()
                        .all(|value| value.abs() < 0.000_1)
                );
            } else {
                let interior = &model.patches[0][50 * MEL_BANDS..51 * MEL_BANDS];
                for (actual, expected) in interior.iter().zip(expected_dc) {
                    assert!(
                        (actual - expected).abs() < 0.000_1,
                        "rate {rate}: {actual} versus {expected}"
                    );
                }
            }
        }
        Ok(())
    }

    #[test]
    fn decoder_preserves_stereo_tone_spectra_at_common_rates_when_available()
    -> Result<(), Box<dyn Error>> {
        let Some(ffmpeg) = test_ffmpeg() else {
            return Ok(());
        };
        let directory = tempfile::tempdir()?;
        let path = directory.path().join("stereo-tones.wav");
        let cancelled = AtomicBool::new(false);
        let mut preprocessor = MusicNnPreprocessor::new();
        for rate in [44_100, 48_000] {
            // Distinct channels catch dropped channels and an incorrect mono matrix.
            // f64 generation avoids phase drift in the independent analytic reference.
            let samples: Vec<i16> = (0..rate * 4)
                .flat_map(|index| {
                    [1_000.0, 4_000.0].map(|hz| {
                        (8_192.0
                            * (std::f64::consts::TAU * hz * f64::from(index) / f64::from(rate))
                                .sin())
                        .round() as i16
                    })
                })
                .collect();
            write_pcm_wav(&path, rate, 2, &samples)?;
            let mut model = RecordingPredictor::default();
            let output = decode_and_predict(&mut model, &ffmpeg, &path, &cancelled)?;
            assert_eq!(output.emitted_frames, 251);
            assert_eq!(output.summary.windows, 2);
            // Interior frames avoid treating a resampler's edge-padding policy as
            // passband distortion. Beginning and ending coverage have separate tests.
            for frame_index in [50, 100, 150] {
                let start = frame_index * FRAME_HOP - FRAME_SIZE / 2;
                let frame = std::array::from_fn(|index| {
                    [1_000.0, 4_000.0]
                        .iter()
                        .map(|hz| {
                            0.125
                                * (std::f64::consts::TAU * hz * (start + index) as f64
                                    / f64::from(SAMPLE_RATE))
                                .sin()
                        })
                        .sum::<f64>() as f32
                });
                let expected = preprocessor.transform(&frame);
                let actual =
                    &model.patches[0][frame_index * MEL_BANDS..(frame_index + 1) * MEL_BANDS];
                let maximum = actual
                    .iter()
                    .zip(expected)
                    .map(|(actual, expected)| (actual - expected).abs())
                    .fold(0.0_f32, f32::max);
                eprintln!("rate {rate}, frame {frame_index}: max stereo mel error {maximum:.8}");
                // 0.02 log10 units is about 0.2 dB in (1 + 10000 * mel power).
                // This is an analytic signal gate, not exact upstream SRC parity.
                assert!(
                    maximum <= 0.02,
                    "rate {rate}, frame {frame_index}: max mel error {maximum}"
                );
            }
        }
        Ok(())
    }

    #[test]
    fn decoder_rejects_out_of_band_energy_before_voice_features_when_available()
    -> Result<(), Box<dyn Error>> {
        let Some(ffmpeg) = test_ffmpeg() else {
            return Ok(());
        };
        let directory = tempfile::tempdir()?;
        let path = directory.path().join("out-of-band.wav");
        for rate in [44_100, 48_000] {
            for hz in [9_000.0, 12_000.0] {
                let samples: Vec<i16> = (0..rate * 4)
                    .map(|index| {
                        (8_192.0
                            * (std::f64::consts::TAU * hz * f64::from(index) / f64::from(rate))
                                .sin())
                        .round() as i16
                    })
                    .collect();
                write_pcm_wav(&path, rate, 1, &samples)?;
                let mut model = RecordingPredictor::default();
                let output =
                    decode_and_predict(&mut model, &ffmpeg, &path, &AtomicBool::new(false))?;
                assert_eq!(output.emitted_frames, 251);
                assert_eq!(output.summary.windows, 2);
                let maximum = model.patches[0][50 * MEL_BANDS..151 * MEL_BANDS]
                    .iter()
                    .copied()
                    .fold(0.0_f32, f32::max);
                // Compare linear mel power with an unfiltered aliased tone of
                // the same amplitude. Absolute log-feature values are not dB.
                let unfiltered = std::array::from_fn(|index| {
                    (0.25
                        * (std::f64::consts::TAU * hz * index as f64 / f64::from(SAMPLE_RATE))
                            .sin()) as f32
                });
                let reference = MusicNnPreprocessor::new()
                    .transform(&unfiltered)
                    .into_iter()
                    .fold(0.0_f32, f32::max);
                let attenuation_db = 10.0
                    * ((10.0_f64.powf(f64::from(maximum)) - 1.0)
                        / (10.0_f64.powf(f64::from(reference)) - 1.0))
                        .log10();
                assert!(
                    attenuation_db <= -60.0,
                    "rate {rate}, tone {hz}: alias attenuation {attenuation_db} dB"
                );
                eprintln!("rate {rate}, tone {hz}: alias attenuation {attenuation_db:.2} dB");
            }
        }
        Ok(())
    }

    #[test]
    fn decoder_cleans_up_after_cancellation_when_available() -> Result<(), Box<dyn Error>> {
        let Some(ffmpeg) = test_ffmpeg() else {
            return Ok(());
        };
        let directory = tempfile::tempdir()?;
        let path = directory.path().join("cancel.wav");
        write_tone_wav(&path, 30)?;
        let cancelled = AtomicBool::new(false);
        let mut model = CancellingPredictor {
            calls: 0,
            cancel_on: 1,
            cancelled: &cancelled,
        };
        let started = Instant::now();
        assert!(matches!(
            decode_and_predict(&mut model, &ffmpeg, &path, &cancelled),
            Err(VoiceAnalysisError::Cancelled)
        ));
        assert_eq!(model.calls, 1);
        assert!(started.elapsed() < Duration::from_secs(5));

        let mut model = FixedPredictor::default();
        assert!(matches!(
            decode_and_predict_until(
                &mut model,
                &ffmpeg,
                &path,
                &AtomicBool::new(false),
                Instant::now()
            ),
            Err(VoiceAnalysisError::DeadlineExceeded)
        ));
        assert_eq!(model.calls, 0);
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
    fn centered_frames_and_overlapping_patches_match_musicnn_counts()
    -> Result<(), VoiceAnalysisError> {
        let sample_count = 100_000_usize;
        let expected_frames = sample_count.div_ceil(FRAME_HOP).saturating_add(1);
        let expected_patches = expected_frames
            .saturating_sub(PATCH_FRAMES)
            .div_ceil(PATCH_HOP)
            .saturating_add(1);
        let cancelled = AtomicBool::new(false);
        let mut model = FixedPredictor::default();
        let mut pipeline = VoicePipeline::new(Instant::now() + VOICE_ANALYSIS_TIMEOUT);
        for index in 0..sample_count {
            let sample = (TAU * 440.0 * index as f32 / SAMPLE_RATE as f32).sin() * 0.2;
            pipeline.add_sample(sample, &mut model, &cancelled)?;
        }
        let output = pipeline.finish(&mut model, &cancelled)?;
        assert_eq!(output.emitted_frames, expected_frames);
        assert_eq!(output.summary.windows, expected_patches);
        assert_eq!(model.calls, expected_patches);
        Ok(())
    }

    #[test]
    fn model_snapshot_reads_are_bounded_and_preserve_io_errors() -> Result<(), Box<dyn Error>> {
        struct EndlessModel {
            bytes_read: u64,
        }
        impl Read for EndlessModel {
            fn read(&mut self, output: &mut [u8]) -> io::Result<usize> {
                output.fill(0);
                self.bytes_read += output.len() as u64;
                Ok(output.len())
            }
        }
        let mut source = EndlessModel { bytes_read: 0 };
        let error = read_voice_model_bytes(&mut source)
            .err()
            .ok_or("oversized model accepted")?;
        assert_eq!(error.kind(), io::ErrorKind::InvalidData);
        assert_eq!(source.bytes_read, MAX_VOICE_MODEL_BYTES + 1);

        struct UnreadableModel;
        impl Read for UnreadableModel {
            fn read(&mut self, _: &mut [u8]) -> io::Result<usize> {
                Err(io::Error::from(io::ErrorKind::PermissionDenied))
            }
        }
        let error = read_voice_model_bytes(UnreadableModel)
            .err()
            .ok_or("read failure lost")?;
        assert_eq!(error.kind(), io::ErrorKind::PermissionDenied);
        assert_eq!(
            read_voice_model_bytes(b"complete input".as_slice())?,
            b"complete input"
        );
        Ok(())
    }

    #[test]
    fn unverified_graph_is_rejected_before_tensorflow_import() -> Result<(), Box<dyn Error>> {
        let directory = tempfile::tempdir()?;
        let path = directory.path().join("empty-graph.pb");
        tract_tensorflow::tfpb::graph().save_to(&path)?;
        let error = TractVoiceModel::load(&path)
            .err()
            .ok_or("unverified graph accepted")?;
        assert_eq!(
            error.to_string(),
            "voice model checksum does not match the supported artifact"
        );
        Ok(())
    }

    #[test]
    fn worker_rejects_model_replaced_after_startup_when_available() -> Result<(), Box<dyn Error>> {
        let Some(model_path) = std::env::var_os("MUSIC_TEST_VOICE_MODEL").map(PathBuf::from) else {
            return Ok(());
        };
        let directory = tempfile::tempdir()?;
        let path = directory.path().join(VOICE_MODEL_FILENAME);
        let original = std::fs::read(&model_path)?;
        std::fs::write(&path, &original)?;
        let backend = VoiceBackend::initialize(Some(&path), "unused-ffmpeg");
        assert_eq!(backend.status.status, "ready");
        let factory = backend
            .worker_factory
            .ok_or("missing ready worker factory")?;

        let mut graph =
            tract_tensorflow::tensorflow().read_frozen_model(&mut original.as_slice())?;
        let bias = graph
            .node
            .iter_mut()
            .find(|node| node.name == "dense_2/bias")
            .ok_or("official graph has no output bias")?;
        let Some(tract_tensorflow::tfpb::tensorflow::attr_value::Value::Tensor(tensor)) = bias
            .attr
            .get_mut("value")
            .and_then(|attribute| attribute.value.as_mut())
        else {
            return Err("official output bias is not a constant tensor".into());
        };
        assert_eq!(tensor.tensor_content.len(), 8);
        tensor.tensor_content[..4].copy_from_slice(&100.0_f32.to_le_bytes());
        graph.save_to(&path)?;
        assert_ne!(sha256_file(&path)?, VOICE_MODEL_SHA256);

        assert!(
            matches!(factory.start(), Err(VoiceAnalysisError::WorkerUnavailable)),
            "a replacement graph must not run under the startup model's identity"
        );
        let unavailable = VoiceBackend::initialize(Some(&path), "unused-ffmpeg");
        assert_eq!(
            unavailable.status.reason.as_deref(),
            Some("unsupported_model")
        );

        std::fs::write(&path, original)?;
        let recovered = factory.start()?;
        assert!(recovered.is_alive());
        drop(recovered);
        std::fs::remove_file(&path)?;
        assert!(matches!(
            factory.start(),
            Err(VoiceAnalysisError::WorkerUnavailable)
        ));
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
        let mut fixed = FixedPredictor::default();
        let deadline = decode_and_predict_until(
            &mut fixed,
            &ffmpeg,
            &track_path,
            &AtomicBool::new(false),
            Instant::now(),
        )
        .err()
        .ok_or("an already-expired request unexpectedly completed")?;
        assert!(matches!(deadline, VoiceAnalysisError::DeadlineExceeded));
        assert_eq!(fixed.calls, 0);

        let backend = VoiceBackend::initialize(Some(&model_path), ffmpeg);
        assert_eq!(backend.status.status, "ready");
        let signature = backend
            .status
            .source_signature
            .as_deref()
            .ok_or("ready backend has no source signature")?;
        assert!(!signature.contains(&model_path.display().to_string()));
        let worker = backend
            .worker_factory
            .ok_or("ready backend has no worker factory")?
            .start()?;
        let cancelled = worker
            .analyze(track_path.clone(), Arc::new(AtomicBool::new(true)))
            .await
            .err()
            .ok_or("an already-cancelled request unexpectedly completed")?;
        assert!(matches!(cancelled, VoiceAnalysisError::Cancelled));
        assert!(worker.is_alive());
        let document = worker
            .analyze(track_path, Arc::new(AtomicBool::new(false)))
            .await?;
        assert_eq!(document.summary["status"], "classified");
        assert_eq!(document.stage["status"], "complete");
        assert_eq!(document.prediction_windows, 2);
        assert_eq!(document.summary["analyzed_windows"], 2);
        assert!(document.elapsed_seconds > 0.0);
        let shutdown_started = Instant::now();
        drop(worker);
        assert!(shutdown_started.elapsed() < Duration::from_secs(5));
        Ok(())
    }

    #[cfg(target_os = "linux")]
    #[tokio::test]
    #[ignore = "requires the licensed model, FFmpeg, and a three-CPU/four-GiB cgroup"]
    async fn repeated_voice_inference_stays_inside_the_production_resource_envelope()
    -> Result<(), Box<dyn Error>> {
        const RESOURCE_LIMIT_BYTES: u64 = 4 * 1_024 * 1_024 * 1_024;
        const RSS_ACCEPTANCE_BYTES: u64 = 3 * 1_024 * 1_024 * 1_024;
        const RSS_TREND_ALLOWANCE_BYTES: u64 = 64 * 1_024 * 1_024;
        const IDLE_RSS_ALLOWANCE_BYTES: u64 = 512 * 1_024 * 1_024;

        let model_path = std::env::var_os("MUSIC_TEST_VOICE_MODEL")
            .map(PathBuf::from)
            .ok_or("MUSIC_TEST_VOICE_MODEL is required")?;
        let ffmpeg = std::env::var_os("MUSIC_TEST_FFMPEG")
            .map(PathBuf::from)
            .ok_or("MUSIC_TEST_FFMPEG is required")?;
        let iterations = std::env::var("MUSIC_TEST_VOICE_SOAK_ITERATIONS")
            .ok()
            .map(|value| value.parse::<usize>())
            .transpose()?
            .unwrap_or(24);
        if iterations < 12 {
            return Err("MUSIC_TEST_VOICE_SOAK_ITERATIONS must be at least 12".into());
        }
        let memory_limit = cgroup_memory_limit_bytes()?;
        if memory_limit > RESOURCE_LIMIT_BYTES {
            return Err(format!(
                "voice soak must run in a cgroup capped at four GiB; detected {memory_limit} bytes"
            )
            .into());
        }
        let cpu_limit = cgroup_cpu_limit()?;
        if cpu_limit > 3.0 {
            return Err(format!(
                "voice soak must run in a cgroup capped at three CPUs; detected {cpu_limit:.2}"
            )
            .into());
        }

        let baseline_rss = resident_set_bytes()?;
        let directory = tempfile::tempdir()?;
        let track_path = directory.path().join("voice-soak.wav");
        write_tone_wav(&track_path, 4)?;
        let backend = VoiceBackend::initialize(Some(&model_path), ffmpeg);
        assert_eq!(backend.status.status, "ready");
        let idle_after_preflight = resident_set_bytes()?;
        assert!(
            idle_after_preflight <= baseline_rss.saturating_add(IDLE_RSS_ALLOWANCE_BYTES),
            "startup voice preflight retained {idle_after_preflight} bytes from a {baseline_rss}-byte baseline"
        );
        let worker = backend
            .worker_factory
            .ok_or("ready backend has no worker factory")?
            .start()?;

        for _ in 0..3 {
            worker
                .analyze(track_path.clone(), Arc::new(AtomicBool::new(false)))
                .await?;
        }
        let mut rss_samples = Vec::with_capacity(iterations);
        for _ in 0..iterations {
            worker
                .analyze(track_path.clone(), Arc::new(AtomicBool::new(false)))
                .await?;
            rss_samples.push(resident_set_bytes()?);
        }

        let peak_rss = rss_samples.iter().copied().max().unwrap_or_default();
        let segment_length = (rss_samples.len() / 3).max(1);
        let early_median = median_bytes(&rss_samples[..segment_length]);
        let late_median = median_bytes(&rss_samples[rss_samples.len() - segment_length..]);
        eprintln!(
            "voice soak: iterations={iterations} peak_rss={peak_rss} early_median={early_median} late_median={late_median}"
        );
        assert!(
            peak_rss <= RSS_ACCEPTANCE_BYTES,
            "voice inference peak RSS {peak_rss} exceeded the three-GiB acceptance margin"
        );
        assert!(
            late_median <= early_median.saturating_add(RSS_TREND_ALLOWANCE_BYTES),
            "voice inference late median RSS {late_median} grew beyond early median {early_median} plus 64 MiB"
        );

        let cancellation_started = Instant::now();
        let cancelled = worker
            .analyze(track_path, Arc::new(AtomicBool::new(true)))
            .await
            .err()
            .ok_or("an already-cancelled request unexpectedly completed")?;
        assert!(matches!(cancelled, VoiceAnalysisError::Cancelled));
        assert!(cancellation_started.elapsed() < Duration::from_secs(5));
        let shutdown_started = Instant::now();
        drop(worker);
        assert!(shutdown_started.elapsed() < Duration::from_secs(5));
        let idle_after_pass = resident_set_bytes()?;
        eprintln!(
            "voice idle RSS: baseline={baseline_rss} after_preflight={idle_after_preflight} after_pass={idle_after_pass}"
        );
        assert!(
            idle_after_pass <= baseline_rss.saturating_add(IDLE_RSS_ALLOWANCE_BYTES),
            "completed voice pass retained {idle_after_pass} bytes from a {baseline_rss}-byte baseline"
        );
        Ok(())
    }

    #[cfg(target_os = "linux")]
    fn resident_set_bytes() -> io::Result<u64> {
        let status = std::fs::read_to_string("/proc/self/status")?;
        let line = status
            .lines()
            .find(|line| line.starts_with("VmRSS:"))
            .ok_or_else(|| io::Error::other("/proc/self/status has no VmRSS entry"))?;
        let kibibytes = line
            .split_ascii_whitespace()
            .nth(1)
            .ok_or_else(|| io::Error::other("VmRSS has no numeric value"))?
            .parse::<u64>()
            .map_err(io::Error::other)?;
        Ok(kibibytes.saturating_mul(1_024))
    }

    #[cfg(target_os = "linux")]
    fn cgroup_memory_limit_bytes() -> io::Result<u64> {
        read_cgroup_limit(&[
            "/sys/fs/cgroup/memory.max",
            "/sys/fs/cgroup/memory/memory.limit_in_bytes",
        ])
    }

    #[cfg(target_os = "linux")]
    fn cgroup_cpu_limit() -> io::Result<f64> {
        let value = std::fs::read_to_string("/sys/fs/cgroup/cpu.max")?;
        let mut parts = value.split_ascii_whitespace();
        let quota = parts
            .next()
            .ok_or_else(|| io::Error::other("cpu.max has no quota"))?;
        if quota == "max" {
            return Err(io::Error::other("the CPU cgroup is unlimited"));
        }
        let period = parts
            .next()
            .ok_or_else(|| io::Error::other("cpu.max has no period"))?;
        let quota = quota.parse::<f64>().map_err(io::Error::other)?;
        let period = period.parse::<f64>().map_err(io::Error::other)?;
        if period <= 0.0 {
            return Err(io::Error::other("cpu.max period is not positive"));
        }
        Ok(quota / period)
    }

    #[cfg(target_os = "linux")]
    fn read_cgroup_limit(candidates: &[&str]) -> io::Result<u64> {
        for candidate in candidates {
            let Ok(value) = std::fs::read_to_string(candidate) else {
                continue;
            };
            let value = value.trim();
            if value == "max" {
                return Err(io::Error::other("the memory cgroup is unlimited"));
            }
            return value.parse::<u64>().map_err(io::Error::other);
        }
        Err(io::Error::new(
            io::ErrorKind::NotFound,
            "no supported memory cgroup limit file was found",
        ))
    }

    #[cfg(target_os = "linux")]
    fn median_bytes(values: &[u64]) -> u64 {
        let mut sorted = values.to_vec();
        sorted.sort_unstable();
        sorted[sorted.len() / 2]
    }

    fn write_tone_wav(path: &Path, seconds: u32) -> io::Result<()> {
        let sample_count = seconds.saturating_mul(SAMPLE_RATE);
        let samples: Vec<i16> = (0..sample_count)
            .map(|index| {
                (0.2 * (TAU * 440.0 * index as f32 / SAMPLE_RATE as f32).sin()
                    * f32::from(i16::MAX))
                .round() as i16
            })
            .collect();
        write_pcm_wav(path, SAMPLE_RATE, 1, &samples)
    }

    fn write_pcm_wav(
        path: &Path,
        sample_rate: u32,
        channels: u16,
        samples: &[i16],
    ) -> io::Result<()> {
        let data_bytes = (samples.len() as u32).saturating_mul(2);
        let block_align = channels.saturating_mul(2);
        let mut output = File::create(path)?;
        output.write_all(b"RIFF")?;
        output.write_all(&(36_u32.saturating_add(data_bytes)).to_le_bytes())?;
        output.write_all(b"WAVEfmt ")?;
        output.write_all(&16_u32.to_le_bytes())?;
        output.write_all(&1_u16.to_le_bytes())?;
        output.write_all(&channels.to_le_bytes())?;
        output.write_all(&sample_rate.to_le_bytes())?;
        output.write_all(
            &sample_rate
                .saturating_mul(u32::from(block_align))
                .to_le_bytes(),
        )?;
        output.write_all(&block_align.to_le_bytes())?;
        output.write_all(&16_u16.to_le_bytes())?;
        output.write_all(b"data")?;
        output.write_all(&data_bytes.to_le_bytes())?;
        for sample in samples {
            output.write_all(&sample.to_le_bytes())?;
        }
        Ok(())
    }
}
