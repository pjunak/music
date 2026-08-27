use std::collections::BTreeMap;
use std::ffi::OsString;
use std::fs::{self, OpenOptions};
use std::io::{self, Read};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use serde::Deserialize;

use super::{
    AudioMetadata, MetadataError, StagedTagUpdate, TagField, TagPatch, TagValue, asf,
    coerce_number, metadata_from_tagged_file, read_tagged_file, validate_stage_request,
    verify_patch,
};

const MAX_TOOL_OUTPUT_BYTES: usize = 1024 * 1024;
const DEFAULT_PROBE_TIMEOUT: Duration = Duration::from_secs(10);
const DEFAULT_MEDIA_TIMEOUT: Duration = Duration::from_secs(120);
const WAIT_POLL_INTERVAL: Duration = Duration::from_millis(10);

#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

#[derive(Debug, Clone)]
pub struct FfmpegTools {
    ffmpeg: PathBuf,
    ffprobe: PathBuf,
    probe_timeout: Duration,
    media_timeout: Duration,
}

impl FfmpegTools {
    pub fn new(ffmpeg: impl Into<PathBuf>, ffprobe: impl Into<PathBuf>) -> Self {
        Self {
            ffmpeg: ffmpeg.into(),
            ffprobe: ffprobe.into(),
            probe_timeout: DEFAULT_PROBE_TIMEOUT,
            media_timeout: DEFAULT_MEDIA_TIMEOUT,
        }
    }

    #[must_use]
    pub const fn with_timeouts(mut self, probe_timeout: Duration, media_timeout: Duration) -> Self {
        self.probe_timeout = probe_timeout;
        self.media_timeout = media_timeout;
        self
    }
}

pub(super) fn read_wma_metadata(
    path: &Path,
    tools: &FfmpegTools,
) -> Result<AudioMetadata, MetadataError> {
    Ok(probe_wma(path, tools)?.metadata)
}

pub(super) fn read_aac_metadata(
    path: &Path,
    tools: &FfmpegTools,
) -> Result<AudioMetadata, MetadataError> {
    let tagged_file = read_tagged_file(path)?;
    let mut metadata = metadata_from_tagged_file(&tagged_file);
    metadata.duration = probe_audio_duration(path, tools)?;
    Ok(metadata)
}

pub(super) fn stage_wma_tag_update(
    source: &Path,
    staged: &Path,
    patch: &TagPatch,
    tools: &FfmpegTools,
) -> Result<StagedTagUpdate, MetadataError> {
    validate_stage_request(source, staged, patch)?;
    let source = fs::canonicalize(source).map_err(|source| MetadataError::Io {
        action: "canonicalize WMA metadata source",
        source,
    })?;
    let staged = absolute_new_path(staged)?;

    let result = stage_wma_tag_update_inner(&source, &staged, patch, tools);
    if result.is_err() {
        let _ = fs::remove_file(&staged);
    }
    result
}

fn stage_wma_tag_update_inner(
    source: &Path,
    staged: &Path,
    patch: &TagPatch,
    tools: &FfmpegTools,
) -> Result<StagedTagUpdate, MetadataError> {
    let before = probe_wma(source, tools)?;
    let source_stream_hash = stream_hash(source, tools)?;

    let mut arguments = vec![
        OsString::from("-n"),
        OsString::from("-nostdin"),
        OsString::from("-hide_banner"),
        OsString::from("-loglevel"),
        OsString::from("error"),
        OsString::from("-protocol_whitelist"),
        OsString::from("file"),
        OsString::from("-i"),
        source.as_os_str().to_owned(),
        OsString::from("-map"),
        OsString::from("0"),
        OsString::from("-map_metadata"),
        OsString::from("0"),
        OsString::from("-map_chapters"),
        OsString::from("0"),
        OsString::from("-c"),
        OsString::from("copy"),
    ];
    for (&field, value) in &patch.changes {
        arguments.push(OsString::from("-metadata"));
        arguments.push(OsString::from(format!(
            "{}={}",
            ffmpeg_key(field),
            value.as_ref().map_or_else(String::new, tag_value_text)
        )));
    }
    arguments.extend([
        OsString::from("-threads"),
        OsString::from("1"),
        OsString::from("-f"),
        OsString::from("asf"),
        staged.as_os_str().to_owned(),
    ]);
    run_tool(&tools.ffmpeg, "ffmpeg", &arguments, tools.media_timeout)?;

    OpenOptions::new()
        .write(true)
        .open(staged)
        .and_then(|file| file.sync_all())
        .map_err(|source| MetadataError::Io {
            action: "synchronize staged WMA metadata file",
            source,
        })?;

    let after = probe_wma(staged, tools)?;
    if before.format_name != after.format_name {
        return Err(MetadataError::FormatChanged);
    }
    if before.audio_codecs != after.audio_codecs
        || source_stream_hash != stream_hash(staged, tools)?
    {
        return Err(MetadataError::CodecChanged);
    }
    if before.metadata.duration != after.metadata.duration {
        return Err(MetadataError::DurationChanged);
    }
    verify_patch(&after.metadata, patch)?;

    Ok(StagedTagUpdate::new(staged.to_path_buf(), after.metadata))
}

fn absolute_new_path(path: &Path) -> Result<PathBuf, MetadataError> {
    let parent = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    let parent = fs::canonicalize(parent).map_err(|source| MetadataError::Io {
        action: "canonicalize WMA staging directory",
        source,
    })?;
    let file_name = path
        .file_name()
        .ok_or_else(|| MetadataError::InvalidAsf("staged WMA path has no file name".to_owned()))?;
    Ok(parent.join(file_name))
}

fn stream_hash(path: &Path, tools: &FfmpegTools) -> Result<String, MetadataError> {
    let arguments = [
        OsString::from("-nostdin"),
        OsString::from("-hide_banner"),
        OsString::from("-loglevel"),
        OsString::from("error"),
        OsString::from("-protocol_whitelist"),
        OsString::from("file,pipe"),
        OsString::from("-i"),
        path.as_os_str().to_owned(),
        OsString::from("-map"),
        OsString::from("0:a"),
        OsString::from("-c"),
        OsString::from("copy"),
        OsString::from("-f"),
        OsString::from("hash"),
        OsString::from("-hash"),
        OsString::from("sha256"),
        OsString::from("pipe:1"),
    ];
    let output = run_tool(&tools.ffmpeg, "ffmpeg", &arguments, tools.media_timeout)?;
    let text = std::str::from_utf8(&output)
        .map_err(|error| MetadataError::Parse(error.to_string()))?
        .trim();
    let hash = text.strip_prefix("SHA256=").ok_or_else(|| {
        MetadataError::Parse("ffmpeg did not return a SHA-256 stream hash".to_owned())
    })?;
    if hash.len() != 64 || !hash.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(MetadataError::Parse(
            "ffmpeg returned an invalid SHA-256 stream hash".to_owned(),
        ));
    }
    Ok(hash.to_ascii_lowercase())
}

#[derive(Debug)]
struct WmaProbe {
    metadata: AudioMetadata,
    format_name: String,
    audio_codecs: Vec<String>,
}

fn probe_wma(path: &Path, tools: &FfmpegTools) -> Result<WmaProbe, MetadataError> {
    let arguments = [
        OsString::from("-v"),
        OsString::from("error"),
        OsString::from("-protocol_whitelist"),
        OsString::from("file"),
        OsString::from("-show_entries"),
        OsString::from("format=format_name:format_tags:stream=index,codec_type,codec_name"),
        OsString::from("-of"),
        OsString::from("json"),
        path.as_os_str().to_owned(),
    ];
    let output = run_tool(&tools.ffprobe, "ffprobe", &arguments, tools.probe_timeout)?;
    let document: ProbeDocument =
        serde_json::from_slice(&output).map_err(|error| MetadataError::Parse(error.to_string()))?;
    let format = document
        .format
        .ok_or_else(|| MetadataError::Parse("ffprobe omitted format data".to_owned()))?;
    if !format
        .format_name
        .split(',')
        .any(|name| name.eq_ignore_ascii_case("asf"))
    {
        return Err(MetadataError::UnsupportedFormat {
            extension: format.format_name,
        });
    }
    let audio_codecs: Vec<String> = document
        .streams
        .into_iter()
        .filter(|stream| stream.codec_type.eq_ignore_ascii_case("audio"))
        .map(|stream| stream.codec_name)
        .collect();
    if audio_codecs.is_empty() {
        return Err(MetadataError::MissingAudioStream);
    }

    let tags = format.tags;
    Ok(WmaProbe {
        metadata: AudioMetadata {
            title: tag_text(&tags, &["title"]),
            artist: tag_text(&tags, &["artist", "author"]),
            album_artist: tag_text(&tags, &["albumartist", "album_artist", "WM/AlbumArtist"]),
            album: tag_text(&tags, &["album", "WM/AlbumTitle"]),
            track_no: tag_number(&tags, &["tracknumber", "track", "WM/TrackNumber"]),
            disc_no: tag_number(&tags, &["discnumber", "disc", "WM/PartOfSet"]),
            year: tag_number(&tags, &["date", "year", "WM/Year"]),
            genre: tag_text(&tags, &["genre", "WM/Genre"]),
            bpm: tag_number(&tags, &["bpm", "WM/BeatsPerMinute"]),
            duration: asf::duration(path)?,
            artwork: None,
        },
        format_name: format.format_name,
        audio_codecs,
    })
}

fn probe_audio_duration(path: &Path, tools: &FfmpegTools) -> Result<Duration, MetadataError> {
    let arguments = [
        OsString::from("-v"),
        OsString::from("error"),
        OsString::from("-protocol_whitelist"),
        OsString::from("file"),
        OsString::from("-show_entries"),
        OsString::from("format=duration:stream=codec_type,duration"),
        OsString::from("-of"),
        OsString::from("json"),
        path.as_os_str().to_owned(),
    ];
    let output = run_tool(&tools.ffprobe, "ffprobe", &arguments, tools.probe_timeout)?;
    let document: ProbeDocument =
        serde_json::from_slice(&output).map_err(|error| MetadataError::Parse(error.to_string()))?;
    let duration = document
        .streams
        .iter()
        .find(|stream| stream.codec_type.eq_ignore_ascii_case("audio"))
        .and_then(|stream| stream.duration.as_deref())
        .or_else(|| {
            document
                .format
                .as_ref()
                .and_then(|format| format.duration.as_deref())
        })
        .ok_or_else(|| MetadataError::Parse("ffprobe omitted audio duration".to_owned()))?;
    let seconds: f64 = duration
        .parse()
        .map_err(|_| MetadataError::Parse("ffprobe returned invalid audio duration".to_owned()))?;
    Duration::try_from_secs_f64(seconds)
        .map_err(|_| MetadataError::Parse("ffprobe returned invalid audio duration".to_owned()))
}

fn tag_text(tags: &BTreeMap<String, String>, candidates: &[&str]) -> String {
    candidates
        .iter()
        .find_map(|candidate| {
            tags.iter()
                .find(|(key, _)| key.eq_ignore_ascii_case(candidate))
                .map(|(_, value)| value.clone())
        })
        .unwrap_or_default()
}

fn tag_number(tags: &BTreeMap<String, String>, candidates: &[&str]) -> Option<u32> {
    coerce_number(&tag_text(tags, candidates))
}

const fn ffmpeg_key(field: TagField) -> &'static str {
    match field {
        TagField::Title => "title",
        TagField::Artist => "artist",
        TagField::AlbumArtist => "albumartist",
        TagField::Album => "album",
        TagField::TrackNumber => "tracknumber",
        TagField::DiscNumber => "discnumber",
        TagField::Year => "date",
        TagField::Genre => "genre",
        TagField::Bpm => "bpm",
    }
}

fn tag_value_text(value: &TagValue) -> String {
    match value {
        TagValue::Text(value) => value.clone(),
        TagValue::Number(value) => value.to_string(),
    }
}

#[derive(Debug, Deserialize)]
struct ProbeDocument {
    #[serde(default)]
    streams: Vec<ProbeStream>,
    format: Option<ProbeFormat>,
}

#[derive(Debug, Deserialize)]
struct ProbeStream {
    #[serde(default)]
    codec_type: String,
    #[serde(default)]
    codec_name: String,
    #[serde(default)]
    duration: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ProbeFormat {
    #[serde(default)]
    format_name: String,
    #[serde(default)]
    duration: Option<String>,
    #[serde(default)]
    tags: BTreeMap<String, String>,
}

struct ToolOutput {
    status: ExitStatus,
    stdout: Vec<u8>,
    stdout_truncated: bool,
    stderr_truncated: bool,
}

fn run_tool(
    executable: &Path,
    tool: &'static str,
    arguments: &[OsString],
    timeout: Duration,
) -> Result<Vec<u8>, MetadataError> {
    let mut command = Command::new(executable);
    command
        .args(arguments)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .env_remove("FFREPORT")
        .env_remove("AV_LOG_FORCE_COLOR");
    configure_process(&mut command);

    let mut child = command.spawn().map_err(|source| MetadataError::Io {
        action: "start media metadata subprocess",
        source,
    })?;
    let stdout = child.stdout.take().ok_or_else(|| MetadataError::Io {
        action: "capture media metadata stdout",
        source: io::Error::other("stdout pipe is unavailable"),
    })?;
    let stderr = child.stderr.take().ok_or_else(|| MetadataError::Io {
        action: "capture media metadata stderr",
        source: io::Error::other("stderr pipe is unavailable"),
    })?;
    let stdout_reader = spawn_bounded_reader(stdout);
    let stderr_reader = spawn_bounded_reader(stderr);

    let status = wait_for_child(&mut child, tool, timeout)?;
    let (stdout, stdout_truncated) = join_reader(stdout_reader)?;
    let (_, stderr_truncated) = join_reader(stderr_reader)?;
    let output = ToolOutput {
        status,
        stdout,
        stdout_truncated,
        stderr_truncated,
    };
    if output.stdout_truncated || output.stderr_truncated {
        return Err(MetadataError::ProcessOutputTruncated { tool });
    }
    if !output.status.success() {
        return Err(MetadataError::ProcessFailed {
            tool,
            code: output.status.code(),
        });
    }
    Ok(output.stdout)
}

fn wait_for_child(
    child: &mut Child,
    tool: &'static str,
    timeout: Duration,
) -> Result<ExitStatus, MetadataError> {
    let deadline = Instant::now()
        .checked_add(timeout)
        .unwrap_or_else(Instant::now);
    loop {
        if let Some(status) = child.try_wait().map_err(|source| MetadataError::Io {
            action: "wait for media metadata subprocess",
            source,
        })? {
            return Ok(status);
        }
        let now = Instant::now();
        if now >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            return Err(MetadataError::ProcessTimedOut { tool, timeout });
        }
        thread::sleep(WAIT_POLL_INTERVAL.min(deadline.duration_since(now)));
    }
}

fn spawn_bounded_reader<R>(reader: R) -> JoinHandle<io::Result<(Vec<u8>, bool)>>
where
    R: Read + Send + 'static,
{
    thread::spawn(move || read_bounded(reader))
}

fn read_bounded(mut reader: impl Read) -> io::Result<(Vec<u8>, bool)> {
    let mut stored = Vec::new();
    let mut truncated = false;
    let mut buffer = [0_u8; 8192];
    loop {
        let read = reader.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        let remaining = MAX_TOOL_OUTPUT_BYTES.saturating_sub(stored.len());
        let retained = read.min(remaining);
        stored.extend_from_slice(&buffer[..retained]);
        truncated |= retained < read;
    }
    Ok((stored, truncated))
}

fn join_reader(
    reader: JoinHandle<io::Result<(Vec<u8>, bool)>>,
) -> Result<(Vec<u8>, bool), MetadataError> {
    reader
        .join()
        .map_err(|_| MetadataError::Io {
            action: "join media metadata output reader",
            source: io::Error::other("output reader panicked"),
        })?
        .map_err(|source| MetadataError::Io {
            action: "read media metadata subprocess output",
            source,
        })
}

#[cfg(windows)]
fn configure_process(command: &mut Command) {
    use std::os::windows::process::CommandExt;

    command.creation_flags(CREATE_NO_WINDOW);
}

#[cfg(not(windows))]
fn configure_process(_: &mut Command) {}
