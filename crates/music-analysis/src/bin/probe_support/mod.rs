pub(super) mod cgroup;

use std::error::Error;
use std::ffi::OsString;
use std::io::Read;
use std::path::PathBuf;

use serde_json::{Value, json};

const MAX_INPUT_BYTES: u64 = 4 * 1_024 * 1_024;
const MAX_TRACKS: usize = 512;
pub(super) type ProbeError = Box<dyn Error + Send + Sync>;

#[derive(Debug, Default, PartialEq)]
pub(super) struct ProcessMemory {
    resident_bytes: Option<u64>,
    peak_resident_bytes: Option<u64>,
}

impl ProcessMemory {
    pub(super) fn capture() -> Self {
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

    pub(super) fn json(&self) -> Value {
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

pub(super) fn read_tracks(reader: impl Read) -> Result<Vec<PathBuf>, ProbeError> {
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

pub(super) fn set_once<T>(target: &mut Option<T>, value: T, flag: &str) -> Result<(), String> {
    if target.replace(value).is_some() {
        return Err(format!("{flag} may be specified only once"));
    }
    Ok(())
}

pub(super) fn bounded_number(
    value: &OsString,
    min: u32,
    max: u32,
    flag: &str,
) -> Result<u32, String> {
    value
        .to_str()
        .and_then(|value| value.parse::<u32>().ok())
        .filter(|value| (min..=max).contains(value))
        .ok_or_else(|| format!("{flag} must be an integer from {min} to {max}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

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
}
