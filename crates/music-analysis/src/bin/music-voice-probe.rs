#![forbid(unsafe_code)]

use std::env;
use std::error::Error;
use std::ffi::OsString;
use std::io::{self, Read, Write};
use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;

use music_analysis::VoiceBackend;
use serde_json::{Value, json};

const MAX_INPUT_BYTES: u64 = 4 * 1_024 * 1_024;
const MAX_TRACKS: usize = 512;
const RECORD_PREFIX: &str = "VOICE_PROBE_JSON ";
type ProbeError = Box<dyn Error + Send + Sync>;

#[derive(Debug, Eq, PartialEq)]
struct Arguments {
    model: PathBuf,
    ffmpeg: PathBuf,
    warmup: bool,
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
    let tracks = read_tracks()?;
    let backend = VoiceBackend::initialize(Some(&arguments.model), arguments.ffmpeg);
    if backend.status.status != "ready" {
        return Err(format!(
            "voice backend is not ready ({})",
            backend.status.reason.as_deref().unwrap_or("unknown reason")
        )
        .into());
    }
    let worker = backend
        .worker_factory
        .ok_or("ready voice backend has no worker factory")?
        .start()?;

    if arguments.warmup {
        worker
            .analyze(tracks[0].clone(), Arc::new(AtomicBool::new(false)))
            .await
            .map_err(|error| format!("voice warmup failed: {error}"))?;
    }

    let stdout = io::stdout();
    let mut output = stdout.lock();
    let mut failed = false;
    for (index, path) in tracks.into_iter().enumerate() {
        let record = match worker.analyze(path, Arc::new(AtomicBool::new(false))).await {
            Ok(document) => json!({
                "schema_version": "voice-probe/v1",
                "index": index,
                "status": document.summary.get("status").and_then(Value::as_str),
                "voice_score": document.summary.get("voice_probability").and_then(Value::as_f64),
                "vocal_coverage": document.summary.get("vocal_coverage").and_then(Value::as_f64),
                "prediction_windows": document.prediction_windows,
                "model_sha256": document.stage.get("model_sha256").and_then(Value::as_str),
                "elapsed_seconds": document.elapsed_seconds,
            }),
            Err(error) => {
                failed = true;
                json!({
                    "schema_version": "voice-probe/v1",
                    "index": index,
                    "status": "error",
                    "error": error.to_string(),
                })
            }
        };
        output.write_all(RECORD_PREFIX.as_bytes())?;
        serde_json::to_writer(&mut output, &record)?;
        output.write_all(b"\n")?;
        output.flush()?;
    }
    if failed {
        return Err("one or more voice probes failed".into());
    }
    Ok(())
}

fn read_tracks() -> Result<Vec<PathBuf>, ProbeError> {
    let mut bytes = Vec::new();
    io::stdin()
        .lock()
        .take(MAX_INPUT_BYTES + 1)
        .read_to_end(&mut bytes)?;
    if bytes.len() as u64 > MAX_INPUT_BYTES {
        return Err("stdin JSON exceeds the four-MiB limit".into());
    }
    let values = serde_json::from_slice::<Vec<String>>(&bytes)?;
    if values.is_empty() {
        return Err("stdin JSON must contain at least one track path".into());
    }
    if values.len() > MAX_TRACKS {
        return Err(format!("stdin JSON contains more than {MAX_TRACKS} tracks").into());
    }
    Ok(values.into_iter().map(PathBuf::from).collect())
}

fn parse_arguments(
    arguments: impl IntoIterator<Item = OsString>,
) -> Result<Option<Arguments>, String> {
    let mut model = None;
    let mut ffmpeg = None;
    let mut warmup = false;
    let mut arguments = arguments.into_iter();
    while let Some(argument) = arguments.next() {
        let Some(flag) = argument.to_str() else {
            return Err("flags must be valid Unicode".to_owned());
        };
        match flag {
            "--help" | "-h" => return Ok(None),
            "--model" => set_once(
                &mut model,
                arguments
                    .next()
                    .ok_or_else(|| "--model requires a path".to_owned())?,
                "--model",
            )?,
            "--ffmpeg" => set_once(
                &mut ffmpeg,
                arguments
                    .next()
                    .ok_or_else(|| "--ffmpeg requires a path".to_owned())?,
                "--ffmpeg",
            )?,
            "--warmup" if !warmup => warmup = true,
            "--warmup" => return Err("--warmup may be specified only once".to_owned()),
            _ => return Err(format!("unknown argument: {flag}")),
        }
    }
    Ok(Some(Arguments {
        model: PathBuf::from(model.ok_or_else(|| "--model is required".to_owned())?),
        ffmpeg: PathBuf::from(ffmpeg.ok_or_else(|| "--ffmpeg is required".to_owned())?),
        warmup,
    }))
}

fn set_once(target: &mut Option<OsString>, value: OsString, flag: &str) -> Result<(), String> {
    if target.replace(value).is_some() {
        return Err(format!("{flag} may be specified only once"));
    }
    Ok(())
}

fn usage() -> &'static str {
    "usage: music-voice-probe --model <model.pb> --ffmpeg <ffmpeg> [--warmup]\n\
     Reads a JSON array of private audio paths from stdin and emits path-free prefixed JSON records."
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parser_accepts_the_bounded_acceptance_shape() -> Result<(), String> {
        let parsed = parse_arguments([
            OsString::from("--model"),
            OsString::from("model.pb"),
            OsString::from("--ffmpeg"),
            OsString::from("/usr/bin/ffmpeg"),
            OsString::from("--warmup"),
        ])?
        .ok_or_else(|| "parser unexpectedly returned help".to_owned())?;
        assert_eq!(
            parsed,
            Arguments {
                model: PathBuf::from("model.pb"),
                ffmpeg: PathBuf::from("/usr/bin/ffmpeg"),
                warmup: true,
            }
        );
        Ok(())
    }

    #[test]
    fn parser_rejects_missing_duplicate_and_unknown_arguments() {
        assert!(parse_arguments([]).is_err());
        assert!(
            parse_arguments([
                OsString::from("--model"),
                OsString::from("a.pb"),
                OsString::from("--model"),
                OsString::from("b.pb"),
            ])
            .is_err()
        );
        assert!(parse_arguments([OsString::from("--private-path")]).is_err());
    }
}
