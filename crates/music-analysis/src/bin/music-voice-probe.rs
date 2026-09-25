#![forbid(unsafe_code)]

use std::env;
use std::ffi::OsString;
use std::future::Future;
use std::io::{self, Write};
use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use music_analysis::{VoiceAnalysisDocument, VoiceAnalysisError, VoiceBackend};
use serde_json::{Value, json};

mod probe_support;

use probe_support::{ProbeError, ProcessMemory, bounded_number, read_tracks, set_once};

const RECORD_PREFIX: &str = "VOICE_PROBE_JSON ";

#[derive(Debug, Eq, PartialEq)]
struct Arguments {
    model: PathBuf,
    ffmpeg: PathBuf,
    warmup: bool,
    repeat: u32,
    cancel_after: Option<Duration>,
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> ExitCode {
    match parse_arguments(env::args_os().skip(1)) {
        Ok(None) => {
            println!("{}", usage());
            ExitCode::SUCCESS
        }
        Ok(Some(arguments)) => match run(arguments).await {
            Ok(()) => ExitCode::SUCCESS,
            Err(error) => {
                eprintln!("music-voice-probe: {error}");
                ExitCode::FAILURE
            }
        },
        Err(error) => {
            eprintln!("music-voice-probe: {error}\n\n{}", usage());
            ExitCode::FAILURE
        }
    }
}

async fn run(arguments: Arguments) -> Result<(), ProbeError> {
    let tracks = read_tracks(io::stdin().lock())?;
    run_with_output(&arguments, &tracks, &mut io::stdout().lock()).await
}

async fn run_with_output(
    arguments: &Arguments,
    tracks: &[PathBuf],
    output: &mut impl Write,
) -> Result<(), ProbeError> {
    let before = ProcessMemory::capture();
    let started = Instant::now();
    // Readiness itself loads and releases a graph. Account for it separately
    // from the job-scoped workers measured by each subsequent pass.
    let backend = VoiceBackend::initialize(Some(&arguments.model), &arguments.ffmpeg);
    let initialization_seconds = started.elapsed().as_secs_f64();
    let after = ProcessMemory::capture();
    if backend.status.status != "ready" {
        return Err(format!(
            "voice backend is not ready ({})",
            backend.status.reason.as_deref().unwrap_or("unknown reason")
        )
        .into());
    }
    let signature = backend
        .status
        .source_signature
        .as_deref()
        .ok_or("ready voice backend has no source identity")?;
    let factory = backend
        .worker_factory
        .as_ref()
        .ok_or("ready voice backend has no worker factory")?;
    let mut initialization = base_record("initialization", signature);
    initialization["status"] = json!("ready");
    initialization["elapsed_seconds"] = json!(initialization_seconds);
    initialization["process_memory_before"] = before.json();
    initialization["process_memory_after"] = after.json();
    write_record(output, &initialization)?;

    let mut unsuccessful = false;
    for iteration in 0..arguments.repeat {
        let before_start = ProcessMemory::capture();
        let pass_started = Instant::now();
        let worker = factory.start()?;
        let worker_start_seconds = pass_started.elapsed().as_secs_f64();
        let after_start = ProcessMemory::capture();
        let warmup_seconds = if arguments.warmup {
            let started = Instant::now();
            worker
                .analyze(tracks[0].clone(), Arc::new(AtomicBool::new(false)))
                .await
                .map_err(|error| format!("voice warmup failed: {}", error_code(&error)))?;
            Some(started.elapsed().as_secs_f64())
        } else {
            None
        };
        let mut failed_tracks = 0;
        for (index, path) in tracks.iter().enumerate() {
            let observation = observe(
                |cancelled| worker.analyze(path.clone(), cancelled),
                arguments.cancel_after,
            )
            .await;
            if !observation.succeeded(arguments.cancel_after.is_some()) {
                failed_tracks += 1;
            }
            let mut record = observation.record(signature, arguments.cancel_after.is_some());
            record["index"] = json!(index);
            record["iteration"] = json!(iteration);
            record["warmup"] = json!(arguments.warmup);
            record["cancel_after_ms"] =
                json!(arguments.cancel_after.map(|delay| delay.as_millis()));
            write_record(output, &record)?;
        }
        let before_release = ProcessMemory::capture();
        let release_started = Instant::now();
        // This is the only handle. Drop joins the model thread; it is not merely
        // removing a reference while inference or subprocess cleanup continues.
        drop(worker);
        let release_seconds = release_started.elapsed().as_secs_f64();
        let after_release = ProcessMemory::capture();
        let mut pass = base_record("pass", signature);
        pass["iteration"] = json!(iteration);
        pass["status"] = json!(if failed_tracks == 0 {
            "complete"
        } else {
            "error"
        });
        pass["tracks"] = json!(tracks.len());
        pass["failed_tracks"] = json!(failed_tracks);
        pass["cancellation_expected"] = json!(arguments.cancel_after.is_some());
        pass["worker_start_seconds"] = json!(worker_start_seconds);
        pass["warmup_seconds"] = json!(warmup_seconds);
        pass["worker_release_seconds"] = json!(release_seconds);
        pass["elapsed_seconds"] = json!(pass_started.elapsed().as_secs_f64());
        pass["process_memory_before_start"] = before_start.json();
        pass["process_memory_after_start"] = after_start.json();
        pass["process_memory_before_release"] = before_release.json();
        pass["process_memory_after_release"] = after_release.json();
        write_record(output, &pass)?;
        unsuccessful |= failed_tracks != 0;
    }
    if unsuccessful {
        return Err(
            "one or more probes failed or did not observe the requested cancellation".into(),
        );
    }
    Ok(())
}

struct Observation {
    result: Result<VoiceAnalysisDocument, VoiceAnalysisError>,
    elapsed_seconds: f64,
    cancellation_seconds: Option<f64>,
    memory_before: ProcessMemory,
    memory_after: ProcessMemory,
}

impl Observation {
    fn succeeded(&self, expected_cancel: bool) -> bool {
        if expected_cancel {
            self.cancellation_seconds.is_some()
                && matches!(self.result, Err(VoiceAnalysisError::Cancelled))
        } else {
            self.result.is_ok()
        }
    }

    fn record(&self, signature: &str, expected_cancel: bool) -> Value {
        let mut record = base_record("track", signature);
        record["elapsed_seconds"] = json!(self.elapsed_seconds);
        record["cancellation_expected"] = json!(expected_cancel);
        record["cancellation_requested"] = json!(self.cancellation_seconds.is_some());
        record["cancellation_latency_seconds"] = json!(self.cancellation_seconds);
        record["process_memory_before"] = self.memory_before.json();
        record["process_memory_after"] = self.memory_after.json();
        match &self.result {
            Ok(document) if !expected_cancel => {
                record["status"] = json!("classified");
                for field in ["voice_score", "vocal_coverage"] {
                    record[field] = json!(
                        document
                            .summary
                            .get(field)
                            .and_then(Value::as_f64)
                            .filter(|value| value.is_finite())
                    );
                }
                record["prediction_windows"] = json!(document.prediction_windows);
            }
            Ok(_) => record["status"] = json!("cancellation_not_observed"),
            Err(error) => {
                record["status"] = json!(if self.succeeded(expected_cancel) {
                    "cancelled"
                } else {
                    "error"
                });
                record["error_code"] = json!(error_code(error));
            }
        }
        record
    }
}

async fn observe<F: Future<Output = Result<VoiceAnalysisDocument, VoiceAnalysisError>>>(
    work: impl FnOnce(Arc<AtomicBool>) -> F,
    cancel_after: Option<Duration>,
) -> Observation {
    let memory_before = ProcessMemory::capture();
    let started = Instant::now();
    let cancelled = Arc::new(AtomicBool::new(false));
    let work = work(Arc::clone(&cancelled));
    tokio::pin!(work);
    let (result, cancellation_seconds) = if let Some(after) = cancel_after {
        tokio::select! {
            biased;
            result = &mut work => (result, None),
            () = tokio::time::sleep(after) => {
                let requested = Instant::now();
                cancelled.store(true, Ordering::Release);
                // Await the real result, including decoder cleanup. A timer
                // firing or dropping the future is not successful cancellation.
                let result = work.await;
                (result, Some(requested.elapsed().as_secs_f64()))
            }
        }
    } else {
        (work.await, None)
    };
    Observation {
        result,
        elapsed_seconds: started.elapsed().as_secs_f64(),
        cancellation_seconds,
        memory_before,
        memory_after: ProcessMemory::capture(),
    }
}

fn error_code(error: &VoiceAnalysisError) -> &'static str {
    match error {
        VoiceAnalysisError::MissingFile => "missing_file",
        VoiceAnalysisError::Spawn(_) => "process_unavailable",
        VoiceAnalysisError::Decode => "decode_failed",
        VoiceAnalysisError::Io(_) => "read_failed",
        VoiceAnalysisError::TooLong => "too_long",
        VoiceAnalysisError::DeadlineExceeded => "deadline_exceeded",
        VoiceAnalysisError::Cancelled => "cancelled",
        VoiceAnalysisError::Inference => "inference_failed",
        VoiceAnalysisError::WorkerUnavailable => "worker_unavailable",
    }
}

fn base_record(record_type: &str, signature: &str) -> Value {
    json!({
        "schema_version": "voice-probe/v2",
        "record_type": record_type,
        "source_signature": signature,
        "platform": std::env::consts::OS,
        "memory_scope": "Probe process only; excludes FFmpeg children and the server. Peak is process-lifetime, not per-track or per-pass. Null means unavailable.",
    })
}

fn write_record(output: &mut impl Write, record: &Value) -> Result<(), ProbeError> {
    output.write_all(RECORD_PREFIX.as_bytes())?;
    serde_json::to_writer(&mut *output, record)?;
    output.write_all(b"\n")?;
    output.flush()?;
    Ok(())
}

fn parse_arguments(
    arguments: impl IntoIterator<Item = OsString>,
) -> Result<Option<Arguments>, String> {
    let mut model = None;
    let mut ffmpeg = None;
    let mut warmup = false;
    let mut repeat = None;
    let mut cancel_after = None;
    let mut arguments = arguments.into_iter();
    while let Some(argument) = arguments.next() {
        let flag = argument.to_str().ok_or("flags must be valid Unicode")?;
        if matches!(flag, "--help" | "-h") {
            return Ok(None);
        }
        if flag == "--warmup" {
            if warmup {
                return Err("--warmup may be specified only once".to_owned());
            }
            warmup = true;
            continue;
        }
        if !["--model", "--ffmpeg", "--repeat", "--cancel-after-ms"].contains(&flag) {
            return Err("unknown argument".to_owned());
        }
        let value = arguments
            .next()
            .ok_or_else(|| format!("{flag} requires a value"))?;
        match flag {
            "--model" => set_once(&mut model, value, flag)?,
            "--ffmpeg" => set_once(&mut ffmpeg, value, flag)?,
            "--repeat" => set_once(&mut repeat, bounded_number(&value, 1, 20, flag)?, flag)?,
            "--cancel-after-ms" => set_once(
                &mut cancel_after,
                Duration::from_millis(u64::from(bounded_number(&value, 1, 1_800_000, flag)?)),
                flag,
            )?,
            _ => return Err("unknown argument".to_owned()),
        }
    }
    if warmup && cancel_after.is_some() {
        return Err("--warmup cannot be combined with --cancel-after-ms".to_owned());
    }
    Ok(Some(Arguments {
        model: PathBuf::from(model.ok_or("--model is required")?),
        ffmpeg: PathBuf::from(ffmpeg.ok_or("--ffmpeg is required")?),
        warmup,
        repeat: repeat.unwrap_or(1),
        cancel_after,
    }))
}

fn usage() -> &'static str {
    "usage: music-voice-probe --model <model.pb> --ffmpeg <path> [--warmup] [--repeat 1..20] [--cancel-after-ms 1..1800000]\n\
     Reads 1-512 private audio paths as a JSON array from stdin. Starts and releases one voice worker per pass.\n\
     Emits path-free prefixed JSON for initialization, tracks and pass lifecycle measurements. Never writes a library.\n\
     Cancellation mode succeeds only when every input returns the requested typed cancelled result; it excludes warmup."
}

#[cfg(test)]
mod tests {
    use super::*;

    fn arguments(values: &[&str]) -> Result<Option<Arguments>, String> {
        parse_arguments(values.iter().map(OsString::from))
    }

    fn document() -> VoiceAnalysisDocument {
        VoiceAnalysisDocument {
            summary: serde_json::Map::from_iter([
                ("voice_score".to_owned(), json!(0.2)),
                ("vocal_coverage".to_owned(), json!(0.0)),
                ("path".to_owned(), json!("private/source.wav")),
            ]),
            stage: serde_json::Map::from_iter([(
                "model_filename".to_owned(),
                json!("private/model.pb"),
            )]),
            elapsed_seconds: 0.0,
            prediction_windows: 2,
        }
    }

    #[test]
    fn arguments_require_explicit_tools_and_bound_work() -> Result<(), ProbeError> {
        let parsed = arguments(&[
            "--model", "model.pb", "--ffmpeg", "decoder", "--warmup", "--repeat", "3",
        ])?
        .ok_or("expected arguments")?;
        assert_eq!(
            parsed,
            Arguments {
                model: PathBuf::from("model.pb"),
                ffmpeg: PathBuf::from("decoder"),
                warmup: true,
                repeat: 3,
                cancel_after: None,
            }
        );
        let parsed = arguments(&[
            "--model",
            "model.pb",
            "--ffmpeg",
            "decoder",
            "--cancel-after-ms",
            "25",
        ])?
        .ok_or("expected arguments")?;
        assert_eq!(parsed.repeat, 1);
        assert_eq!(parsed.cancel_after, Some(Duration::from_millis(25)));
        for invalid in [
            vec![],
            vec!["--model", "model.pb"],
            vec!["--ffmpeg", "decoder"],
            vec!["--repeat", "0"],
            vec!["--repeat", "21"],
            vec!["--repeat", "NaN"],
            vec!["--cancel-after-ms", "0"],
            vec!["--cancel-after-ms", "1800001"],
            vec!["--model", "a.pb", "--model", "b.pb"],
            vec!["--ffmpeg", "a", "--ffmpeg", "b"],
            vec!["--repeat", "2", "--repeat", "3"],
            vec!["--cancel-after-ms", "5", "--cancel-after-ms", "10"],
            vec!["--warmup", "--warmup"],
            vec![
                "--model",
                "a.pb",
                "--ffmpeg",
                "decoder",
                "--warmup",
                "--cancel-after-ms",
                "5",
            ],
            vec!["private/source.wav"],
        ] {
            let error = arguments(&invalid)
                .err()
                .ok_or("invalid arguments accepted")?;
            assert!(!error.contains("private"));
        }
        assert!(arguments(&["--help"])?.is_none());
        Ok(())
    }

    #[tokio::test(flavor = "current_thread")]
    async fn completing_before_the_timer_is_not_cancellation_or_a_score_record() {
        let observation = observe(
            |_| std::future::ready(Ok(document())),
            Some(Duration::from_secs(1)),
        )
        .await;
        assert!(!observation.succeeded(true));
        assert!(observation.cancellation_seconds.is_none());
        let record = observation.record("test-signature", true);
        assert_eq!(record["status"], "cancellation_not_observed");
        assert_eq!(record["cancellation_requested"], false);
        assert!(record.get("prediction_windows").is_none());
        assert!(record.get("voice_score").is_none());
        assert!(!record.to_string().contains("private"));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn cancellation_waits_for_cleanup_and_requires_a_cancelled_result()
    -> Result<(), ProbeError> {
        let cleaned = AtomicBool::new(false);
        let cleanup_flag = &cleaned;
        let observation = tokio::time::timeout(
            Duration::from_secs(2),
            observe(
                |cancelled| async move {
                    while !cancelled.load(Ordering::Acquire) {
                        tokio::time::sleep(Duration::from_millis(1)).await;
                    }
                    tokio::time::sleep(Duration::from_millis(5)).await;
                    cleanup_flag.store(true, Ordering::Release);
                    Err(VoiceAnalysisError::Cancelled)
                },
                Some(Duration::from_millis(5)),
            ),
        )
        .await?;
        assert!(cleaned.load(Ordering::Acquire));
        assert!(observation.succeeded(true));
        assert!(
            observation
                .cancellation_seconds
                .is_some_and(|seconds| seconds >= 0.005)
        );
        let record = observation.record("test-signature", true);
        assert_eq!(record["status"], "cancelled");
        assert_eq!(record["cancellation_requested"], true);
        assert!(record.get("voice_score").is_none());

        let observation = tokio::time::timeout(
            Duration::from_secs(2),
            observe(
                |cancelled| async move {
                    while !cancelled.load(Ordering::Acquire) {
                        tokio::time::sleep(Duration::from_millis(1)).await;
                    }
                    Ok(document())
                },
                Some(Duration::from_millis(1)),
            ),
        )
        .await?;
        assert!(!observation.succeeded(true));
        assert!(observation.cancellation_seconds.is_some());
        assert_eq!(
            observation.record("test-signature", true)["status"],
            "cancellation_not_observed"
        );
        Ok(())
    }

    #[tokio::test(flavor = "current_thread")]
    async fn reports_expose_only_numeric_scores_or_typed_failures() {
        let observation = observe(|_| std::future::ready(Ok(document())), None).await;
        assert!(observation.succeeded(false));
        let record = observation.record("test-signature", false);
        assert_eq!(record["schema_version"], "voice-probe/v2");
        assert_eq!(record["status"], "classified");
        assert_eq!(record["voice_score"], 0.2);
        assert_eq!(record["prediction_windows"], 2);
        assert!(!record.to_string().contains("private"));

        for error in [
            VoiceAnalysisError::Io(io::Error::other("private/source.wav")),
            VoiceAnalysisError::Spawn(io::Error::other("private/decoder.exe")),
            VoiceAnalysisError::Cancelled,
        ] {
            let observation = observe(|_| std::future::ready(Err(error)), None).await;
            assert!(!observation.succeeded(false));
            assert!(!observation.succeeded(true));
            let record = observation.record("test-signature", false);
            assert_eq!(record["status"], "error");
            assert!(record.get("error_code").is_some());
            assert!(record.get("voice_score").is_none());
            assert!(!record.to_string().contains("private"));
        }
    }

    #[tokio::test(flavor = "current_thread")]
    async fn optional_real_model_repeats_tracks_and_releases_each_pass() -> Result<(), ProbeError> {
        let Some(model) = env::var_os("MUSIC_TEST_VOICE_MODEL") else {
            return Ok(());
        };
        let ffmpeg = env::var_os("MUSIC_TEST_FFMPEG").unwrap_or_else(|| "ffmpeg".into());
        let directory = tempfile::tempdir()?;
        let path = directory.path().join("private.wav");
        write_wav(&path)?;
        let arguments = Arguments {
            model: model.into(),
            ffmpeg: ffmpeg.into(),
            warmup: true,
            repeat: 2,
            cancel_after: None,
        };
        let mut output = Vec::new();
        // The missing file must not prevent the next input or next worker pass.
        assert!(
            run_with_output(
                &arguments,
                &[path.clone(), directory.path().join("missing.wav"), path],
                &mut output
            )
            .await
            .is_err()
        );
        let text = String::from_utf8(output)?;
        assert!(!text.contains("private"));
        assert!(!text.contains("missing.wav"));
        let records = text
            .lines()
            .map(|line| {
                serde_json::from_str::<Value>(
                    line.strip_prefix(RECORD_PREFIX).ok_or("missing prefix")?,
                )
                .map_err(ProbeError::from)
            })
            .collect::<Result<Vec<_>, _>>()?;
        assert_eq!(records.len(), 9);
        assert_eq!(records[0]["record_type"], "initialization");
        assert_eq!(records[0]["status"], "ready");
        for (iteration, pass) in records[1..].chunks_exact(4).enumerate() {
            assert_eq!(pass[0]["index"], 0);
            assert_eq!(pass[0]["status"], "classified");
            assert!(
                pass[0]["prediction_windows"]
                    .as_u64()
                    .is_some_and(|count| count > 0)
            );
            assert_eq!(pass[1]["error_code"], "missing_file");
            assert!(pass[1].get("voice_score").is_none());
            assert_eq!(pass[2]["index"], 2);
            assert_eq!(pass[2]["voice_score"], pass[0]["voice_score"]);
            assert_eq!(pass[2]["prediction_windows"], pass[0]["prediction_windows"]);
            assert_eq!(pass[3]["record_type"], "pass");
            assert_eq!(pass[3]["iteration"], iteration);
            assert_eq!(pass[3]["status"], "error");
            assert_eq!(pass[3]["failed_tracks"], 1);
            assert!(pass[3]["warmup_seconds"].as_f64().is_some());
            assert!(pass[3]["worker_release_seconds"].as_f64().is_some());
            assert!(pass[3].get("process_memory_after_release").is_some());
        }
        Ok(())
    }

    fn write_wav(path: &std::path::Path) -> io::Result<()> {
        let samples = 4 * 16_000_u32;
        let bytes = samples * 2;
        let mut output = std::fs::File::create(path)?;
        output.write_all(b"RIFF")?;
        output.write_all(&(36 + bytes).to_le_bytes())?;
        output.write_all(b"WAVEfmt ")?;
        output.write_all(&16_u32.to_le_bytes())?;
        output.write_all(&1_u16.to_le_bytes())?;
        output.write_all(&1_u16.to_le_bytes())?;
        output.write_all(&16_000_u32.to_le_bytes())?;
        output.write_all(&32_000_u32.to_le_bytes())?;
        output.write_all(&2_u16.to_le_bytes())?;
        output.write_all(&16_u16.to_le_bytes())?;
        output.write_all(b"data")?;
        output.write_all(&bytes.to_le_bytes())?;
        for sample in 0..samples {
            output.write_all(
                &(if sample % 32 < 16 {
                    8_000_i16
                } else {
                    -8_000_i16
                })
                .to_le_bytes(),
            )?;
        }
        Ok(())
    }
}
