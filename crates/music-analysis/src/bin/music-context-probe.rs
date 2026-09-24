#![forbid(unsafe_code)]

use std::env;
use std::error::Error;
use std::ffi::OsString;
use std::io::{self, Read, Write};
use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use music_analysis::{
    AnalysisExecutor, AudioContextAnalyzer, AudioContextDocument, AudioContextError,
    FfmpegContextAnalyzer, VoiceContextPreparation,
};
use serde_json::{Value, json};

const MAX_INPUT_BYTES: u64 = 4 * 1_024 * 1_024;
const MAX_TRACKS: usize = 512;
const RECORD_PREFIX: &str = "CONTEXT_PROBE_JSON ";
type ProbeError = Box<dyn Error + Send + Sync>;

#[derive(Debug, Eq, PartialEq)]
struct Arguments {
    ffmpeg: PathBuf,
    ffprobe: PathBuf,
    repeat: u32,
    cancel_after: Option<Duration>,
}

#[derive(Debug, Default, PartialEq)]
struct ProcessMemory {
    resident_bytes: Option<u64>,
    peak_resident_bytes: Option<u64>,
}

impl ProcessMemory {
    fn capture() -> Self {
        // These are process-local Linux observations, never container totals.
        // Unsupported platforms and inaccessible counters stay unknown.
        if cfg!(target_os = "linux") {
            let mut status = String::new();
            if let Ok(file) = std::fs::File::open("/proc/self/status")
                && file.take(64 * 1_024).read_to_string(&mut status).is_ok()
            {
                return parse_process_memory(&status);
            }
        }
        Self::default()
    }

    fn json(&self) -> Value {
        json!({
            "resident_bytes": self.resident_bytes,
            "peak_resident_bytes": self.peak_resident_bytes,
        })
    }
}

fn parse_process_memory(status: &str) -> ProcessMemory {
    let bytes = |key: &str| {
        let value = status.lines().find_map(|line| line.strip_prefix(key))?;
        let mut fields = value.split_whitespace();
        let kib = fields.next()?.parse::<u64>().ok()?;
        if fields.next()? != "kB" || fields.next().is_some() {
            return None;
        }
        kib.checked_mul(1_024)
    };
    ProcessMemory {
        resident_bytes: bytes("VmRSS:"),
        peak_resident_bytes: bytes("VmHWM:"),
    }
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
                eprintln!("music-context-probe: {error}");
                ExitCode::FAILURE
            }
        },
        Err(error) => {
            eprintln!("music-context-probe: {error}\n\n{}", usage());
            ExitCode::FAILURE
        }
    }
}

async fn run(arguments: Arguments) -> Result<(), ProbeError> {
    let tracks = read_tracks(io::stdin().lock())?;
    let analyzer: Arc<dyn AudioContextAnalyzer> = Arc::new(FfmpegContextAnalyzer::new(
        arguments.ffmpeg,
        arguments.ffprobe,
    ));
    let executor = AnalysisExecutor::new(1)?;
    let stdout = io::stdout();
    let mut output = stdout.lock();
    let mut unsuccessful = false;
    for iteration in 0..arguments.repeat {
        for (index, path) in tracks.iter().enumerate() {
            let before = ProcessMemory::capture();
            let observation = analyze_once(
                &executor,
                Arc::clone(&analyzer),
                path.clone(),
                arguments.cancel_after,
            )
            .await?;
            let after = ProcessMemory::capture();
            let expected_cancel = arguments.cancel_after.is_some();
            let success = if expected_cancel {
                matches!(&observation.result, Err(AudioContextError::Cancelled))
            } else {
                observation.result.is_ok()
            };
            unsuccessful |= !success;
            let record = probe_record(
                index,
                iteration,
                &*analyzer,
                &observation.result,
                observation.elapsed_seconds,
                observation.cancellation_seconds,
                expected_cancel,
            );
            let mut record = record.as_object().cloned().ok_or("invalid probe record")?;
            record.insert("process_memory_before".to_owned(), before.json());
            record.insert("process_memory_after".to_owned(), after.json());
            record.insert(
                "memory_scope".to_owned(),
                json!("Probe process only; excludes FFmpeg/ffprobe children and the server. Peak is process-lifetime, not per-track. Null means unavailable."),
            );
            output.write_all(RECORD_PREFIX.as_bytes())?;
            serde_json::to_writer(&mut output, &record)?;
            output.write_all(b"\n")?;
            output.flush()?;
        }
    }
    // Dropping the fixed executor joins its workers. The probe never starts an
    // application server, opens a library database or retains generated results.
    drop(executor);
    if unsuccessful {
        return Err(
            "one or more probes failed or did not observe the requested cancellation".into(),
        );
    }
    Ok(())
}

struct Observation {
    result: Result<AudioContextDocument, AudioContextError>,
    elapsed_seconds: f64,
    cancellation_seconds: Option<f64>,
}

async fn analyze_once(
    executor: &AnalysisExecutor,
    analyzer: Arc<dyn AudioContextAnalyzer>,
    path: PathBuf,
    cancel_after: Option<Duration>,
) -> Result<Observation, music_analysis::AnalysisExecutorError> {
    let started = Instant::now();
    let cancelled = Arc::new(AtomicBool::new(false));
    let task_cancelled = Arc::clone(&cancelled);
    let work = executor.execute(move || {
        analyzer.analyze(
            &path,
            &task_cancelled,
            VoiceContextPreparation::NotConfigured,
        )
    });
    tokio::pin!(work);
    let (result, cancellation_seconds) = if let Some(after) = cancel_after {
        tokio::select! {
            biased;
            result = &mut work => (result?, None),
            () = tokio::time::sleep(after) => {
                cancelled.store(true, Ordering::Release);
                let requested = Instant::now();
                let result = work.await?;
                (result, Some(requested.elapsed().as_secs_f64()))
            }
        }
    } else {
        (work.await?, None)
    };
    Ok(Observation {
        result,
        elapsed_seconds: started.elapsed().as_secs_f64(),
        cancellation_seconds,
    })
}

fn probe_record(
    index: usize,
    iteration: u32,
    analyzer: &dyn AudioContextAnalyzer,
    result: &Result<AudioContextDocument, AudioContextError>,
    elapsed_seconds: f64,
    cancellation_seconds: Option<f64>,
    expected_cancel: bool,
) -> Value {
    let mut record = json!({
        "schema_version": "context-probe/v2",
        "index": index,
        "iteration": iteration,
        "analyzer_id": analyzer.analyzer_id(),
        "implementation_id": analyzer.implementation_id(),
        "platform": env::consts::OS,
        "elapsed_seconds": elapsed_seconds,
        "cancellation_expected": expected_cancel,
        "cancellation_requested": cancellation_seconds.is_some(),
        "cancellation_latency_seconds": cancellation_seconds,
        "voice": "not_configured",
    });
    match result {
        Ok(document) => {
            record["status"] = json!(if expected_cancel {
                "cancellation_not_observed"
            } else {
                "complete"
            });
            record["audio_seconds"] = json!(document.performance.audio_seconds);
            record["processing_seconds_per_audio_second"] = json!(
                (document.performance.audio_seconds > 0.0)
                    .then(|| elapsed_seconds / document.performance.audio_seconds)
            );
            record["stage_seconds"] = json!(document.performance.stage_seconds);
            record["coverage"] = document
                .summary
                .get("coverage")
                .cloned()
                .unwrap_or(Value::Null);
            record["timeline_points"] = json!(document.timeline.len());
            record["sections"] = json!(document.sections.len());
            record["last_section_end_s"] = document
                .sections
                .last()
                .and_then(|section| section.get("end_s"))
                .cloned()
                .unwrap_or(Value::Null);
            record["loudness"] = loudness_observation(document.technical.get("loudness"));
        }
        Err(error) => {
            record["status"] = json!(if matches!(error, AudioContextError::Cancelled) {
                "cancelled"
            } else {
                "error"
            });
            record["error_code"] = json!(match error {
                AudioContextError::MissingFile => "missing_file",
                AudioContextError::Spawn(_) => "process_unavailable",
                AudioContextError::Decode => "decode_failed",
                AudioContextError::Io(_) => "read_failed",
                AudioContextError::TooShort => "too_short",
                AudioContextError::TooLong => "too_long",
                AudioContextError::Cancelled => "cancelled",
            });
        }
    }
    record
}

fn loudness_observation(value: Option<&Value>) -> Value {
    let status = value
        .and_then(|value| value.get("status"))
        .and_then(Value::as_str);
    let (status, fields): (&str, &[&str]) = match status {
        Some("ebu_r128") => (
            "ebu_r128",
            &[
                "integrated_lufs",
                "loudness_range_lu",
                "true_peak_dbtp",
                "relative_threshold_lufs",
            ],
        ),
        Some("dbfs_proxy") => ("dbfs_proxy", &["rms_dbfs", "peak_dbfs"]),
        _ => return json!({"status": "unavailable"}),
    };
    let mut observation = json!({"status": status});
    for field in fields {
        observation[*field] = json!(
            value
                .and_then(|value| value.get(*field))
                .and_then(Value::as_f64)
                .filter(|number| number.is_finite())
        );
    }
    observation
}

fn read_tracks(reader: impl Read) -> Result<Vec<PathBuf>, ProbeError> {
    let mut bytes = Vec::new();
    reader.take(MAX_INPUT_BYTES + 1).read_to_end(&mut bytes)?;
    if bytes.len() as u64 > MAX_INPUT_BYTES {
        return Err("stdin JSON exceeds the four-MiB limit".into());
    }
    let paths = serde_json::from_slice::<Vec<String>>(&bytes)
        .map_err(|_| "stdin must be a JSON array of audio paths")?;
    if paths.is_empty() || paths.len() > MAX_TRACKS {
        return Err(format!("stdin must contain 1-{MAX_TRACKS} audio paths").into());
    }
    if paths
        .iter()
        .any(|path| path.is_empty() || path.contains('\0'))
    {
        return Err("audio paths must be nonempty and contain no NUL characters".into());
    }
    Ok(paths.into_iter().map(PathBuf::from).collect())
}

fn parse_arguments(
    arguments: impl IntoIterator<Item = OsString>,
) -> Result<Option<Arguments>, String> {
    let mut ffmpeg = None;
    let mut ffprobe = None;
    let mut repeat = None;
    let mut cancel_after = None;
    let mut arguments = arguments.into_iter();
    while let Some(argument) = arguments.next() {
        let flag = argument.to_str().ok_or("flags must be valid Unicode")?;
        if matches!(flag, "--help" | "-h") {
            return Ok(None);
        }
        if !["--ffmpeg", "--ffprobe", "--repeat", "--cancel-after-ms"].contains(&flag) {
            return Err("unknown argument".to_owned());
        }
        let value = arguments
            .next()
            .ok_or_else(|| format!("{flag} requires a value"))?;
        match flag {
            "--ffmpeg" => set_once(&mut ffmpeg, value, flag)?,
            "--ffprobe" => set_once(&mut ffprobe, value, flag)?,
            "--repeat" => set_once(&mut repeat, bounded_number(&value, 1, 20, flag)?, flag)?,
            "--cancel-after-ms" => set_once(
                &mut cancel_after,
                Duration::from_millis(u64::from(bounded_number(&value, 1, 1_800_000, flag)?)),
                flag,
            )?,
            _ => return Err("unknown argument".to_owned()),
        }
    }
    Ok(Some(Arguments {
        ffmpeg: PathBuf::from(ffmpeg.ok_or("--ffmpeg is required")?),
        ffprobe: PathBuf::from(ffprobe.ok_or("--ffprobe is required")?),
        repeat: repeat.unwrap_or(1),
        cancel_after,
    }))
}

fn set_once<T>(target: &mut Option<T>, value: T, flag: &str) -> Result<(), String> {
    if target.replace(value).is_some() {
        return Err(format!("{flag} may be specified only once"));
    }
    Ok(())
}

fn bounded_number(value: &OsString, min: u32, max: u32, flag: &str) -> Result<u32, String> {
    value
        .to_str()
        .and_then(|value| value.parse::<u32>().ok())
        .filter(|value| (min..=max).contains(value))
        .ok_or_else(|| format!("{flag} must be an integer from {min} to {max}"))
}

fn usage() -> &'static str {
    "usage: music-context-probe --ffmpeg <path> --ffprobe <path> [--repeat 1..20] [--cancel-after-ms 1..1800000]\n\
     Reads 1-512 private audio paths as a JSON array from stdin. Runs the real factual extractor on one fixed worker.\n\
     Emits path-free prefixed JSON for coverage, numeric loudness, stage timing and process-local memory. Never writes a library.\n\
     Cancellation mode succeeds only when every input returns the typed cancelled result. Voice has its separate probe."
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    fn arguments(values: &[&str]) -> Result<Option<Arguments>, String> {
        parse_arguments(values.iter().map(OsString::from))
    }

    #[test]
    fn arguments_bound_repetitions_and_require_explicit_tools() -> Result<(), ProbeError> {
        let parsed = arguments(&[
            "--ffmpeg",
            "decoder",
            "--ffprobe",
            "inspector",
            "--repeat",
            "3",
            "--cancel-after-ms",
            "25",
        ])?
        .ok_or("expected arguments")?;
        assert_eq!(parsed.repeat, 3);
        assert_eq!(parsed.cancel_after, Some(Duration::from_millis(25)));
        assert_eq!(parsed.ffmpeg, PathBuf::from("decoder"));
        assert!(arguments(&[]).is_err());
        assert!(arguments(&["--ffmpeg", "decoder", "--repeat", "0"]).is_err());
        assert!(arguments(&["--ffmpeg", "decoder", "--repeat", "21"]).is_err());
        assert!(arguments(&["--ffmpeg", "decoder", "--cancel-after-ms", "1800001"]).is_err());
        assert!(arguments(&["--ffmpeg", "decoder", "--ffmpeg", "other"]).is_err());
        assert!(arguments(&["--repeat", "2", "--repeat", "3"]).is_err());
        assert!(arguments(&["--unknown"]).is_err());
        assert!(arguments(&["--help"])?.is_none());
        Ok(())
    }

    #[test]
    fn stdin_is_bounded_and_does_not_echo_invalid_private_inputs() -> Result<(), ProbeError> {
        assert_eq!(
            read_tracks(Cursor::new(br#"["private.wav"]"#))?,
            vec![PathBuf::from("private.wav")]
        );
        for input in [
            r"[]",
            r#"[""]"#,
            r#"[null]"#,
            r#"["a\u0000b"]"#,
            r#"{"secret_path":"private.wav"}"#,
        ] {
            let error = read_tracks(Cursor::new(input))
                .err()
                .ok_or("input accepted")?
                .to_string();
            assert!(!error.contains("private.wav"));
        }
        let too_many = serde_json::to_vec(&vec!["file"; MAX_TRACKS + 1])?;
        assert!(read_tracks(Cursor::new(too_many)).is_err());
        let too_big = vec![b' '; usize::try_from(MAX_INPUT_BYTES)? + 1];
        assert!(read_tracks(Cursor::new(too_big)).is_err());
        Ok(())
    }

    #[test]
    fn memory_counts_are_process_local_or_unknown_never_invented_zero() {
        assert_eq!(
            parse_process_memory("VmRSS:\t12 kB\nVmHWM: 20 kB\n"),
            ProcessMemory {
                resident_bytes: Some(12 * 1_024),
                peak_resident_bytes: Some(20 * 1_024),
            }
        );
        for input in [
            "",
            "VmRSS: unknown kB",
            "VmRSS: 12 MB",
            "VmRSS: 18446744073709551615 kB",
            "VmRSS: 12 kB extra",
        ] {
            assert_eq!(parse_process_memory(input), ProcessMemory::default());
        }
    }

    #[test]
    fn loudness_observations_include_only_numeric_measurements_and_known_status() {
        let measured = json!({
            "status": "ebu_r128",
            "integrated_lufs": -20.0,
            "loudness_range_lu": 3.2,
            "true_peak_dbtp": -1.3,
            "relative_threshold_lufs": -30.0,
            "path": "private/source.wav",
            "rms_dbfs": -10.0,
        });
        assert_eq!(
            loudness_observation(Some(&measured)),
            json!({
                "status": "ebu_r128",
                "integrated_lufs": -20.0,
                "loudness_range_lu": 3.2,
                "true_peak_dbtp": -1.3,
                "relative_threshold_lufs": -30.0,
            })
        );
        assert_eq!(
            loudness_observation(Some(&json!({
                "status": "dbfs_proxy", "rms_dbfs": -23.0, "peak_dbfs": "private/source.wav",
                "integrated_lufs": -20.0,
            }))),
            json!({"status": "dbfs_proxy", "rms_dbfs": -23.0, "peak_dbfs": null})
        );
        assert_eq!(loudness_observation(None), json!({"status": "unavailable"}));
        assert_eq!(
            loudness_observation(Some(&json!({"status": "private/source.wav"}))),
            json!({"status": "unavailable"})
        );
    }

    #[test]
    fn failure_records_are_path_free_and_never_contain_completed_coverage() {
        let analyzer = FfmpegContextAnalyzer::new("unused", "unused");
        let result = Err(AudioContextError::Io(io::Error::other(
            "private/source.wav",
        )));
        let record = probe_record(2, 1, &analyzer, &result, 0.2, None, false);
        assert_eq!(record["error_code"], "read_failed");
        assert!(!record.to_string().contains("private"));
        assert!(record.get("coverage").is_none());
        assert!(record.get("loudness").is_none());
        let cancelled = probe_record(
            2,
            1,
            &analyzer,
            &Err(AudioContextError::Cancelled),
            0.2,
            Some(0.01),
            true,
        );
        assert_eq!(cancelled["status"], "cancelled");
        assert_eq!(cancelled["cancellation_latency_seconds"], 0.01);
        assert!(cancelled.get("audio_seconds").is_none());
        assert!(cancelled.get("loudness").is_none());
    }

    #[test]
    fn real_pcm_tail_is_retained_and_finishing_early_is_not_a_cancellation()
    -> Result<(), ProbeError> {
        let directory = tempfile::tempdir()?;
        let path = directory.path().join("private.wav");
        write_wav(&path, 10_000)?;
        let ffmpeg = env::var_os("MUSIC_TEST_FFMPEG").unwrap_or_else(|| "ffmpeg".into());
        let ffprobe = env::var_os("MUSIC_TEST_FFPROBE").unwrap_or_else(|| "ffprobe".into());
        let analyzer = FfmpegContextAnalyzer::new(ffmpeg, ffprobe);
        let document = analyzer.analyze(
            &path,
            &AtomicBool::new(false),
            VoiceContextPreparation::NotConfigured,
        )?;
        let record = probe_record(0, 0, &analyzer, &Ok(document.clone()), 0.1, None, false);
        assert_eq!(record["schema_version"], "context-probe/v2");
        assert!(record.get("loudness_status").is_none());
        assert_eq!(record["loudness"]["status"], "ebu_r128");
        assert!(record["loudness"]["integrated_lufs"].as_f64().is_some());
        assert_eq!(record["audio_seconds"], 0.625);
        assert_eq!(record["last_section_end_s"], 0.625);
        assert_eq!(record["coverage"]["scope"], "whole_track");
        assert_eq!(record["coverage"]["decoded_seconds"], 0.625);
        assert_eq!(record["voice"], "not_configured");
        let unobserved = probe_record(0, 0, &analyzer, &Ok(document), 0.1, None, true);
        assert_eq!(unobserved["status"], "cancellation_not_observed");
        assert_eq!(unobserved["cancellation_requested"], false);
        assert_eq!(unobserved["cancellation_latency_seconds"], Value::Null);
        assert!(!record.to_string().contains("private"));
        Ok(())
    }

    #[derive(Debug)]
    struct WaitForCancellation;

    impl AudioContextAnalyzer for WaitForCancellation {
        fn analyzer_id(&self) -> &'static str {
            "test"
        }
        fn implementation_id(&self) -> &'static str {
            "test"
        }
        fn analyze(
            &self,
            _path: &std::path::Path,
            cancelled: &AtomicBool,
            _voice: VoiceContextPreparation,
        ) -> Result<AudioContextDocument, AudioContextError> {
            let deadline = Instant::now() + Duration::from_secs(1);
            while !cancelled.load(Ordering::Acquire) {
                if Instant::now() >= deadline {
                    return Err(AudioContextError::Decode);
                }
                std::thread::sleep(Duration::from_millis(1));
            }
            Err(AudioContextError::Cancelled)
        }
    }

    #[tokio::test(flavor = "current_thread")]
    async fn cancellation_is_awaited_and_the_same_worker_remains_usable() -> Result<(), ProbeError>
    {
        let executor = AnalysisExecutor::new(1)?;
        for _ in 0..2 {
            let observation = tokio::time::timeout(
                Duration::from_secs(3),
                analyze_once(
                    &executor,
                    Arc::new(WaitForCancellation),
                    PathBuf::from("unused"),
                    Some(Duration::from_millis(5)),
                ),
            )
            .await??;
            assert!(matches!(
                observation.result,
                Err(AudioContextError::Cancelled)
            ));
            assert!(observation.cancellation_seconds.is_some());
            assert!(observation.elapsed_seconds >= 0.005);
        }
        let thread_name = executor
            .execute(|| std::thread::current().name().map(str::to_owned))
            .await?;
        assert_eq!(thread_name.as_deref(), Some("music-analysis-0"));
        Ok(())
    }

    fn write_wav(path: &std::path::Path, samples: u32) -> io::Result<()> {
        let mut output = std::fs::File::create(path)?;
        let bytes = samples * 2;
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
