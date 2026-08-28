use std::fmt::{self, Display, Formatter};
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};

use clap::Parser;
use music_protocol::BoundedText;
use reqwest::Url;
use uuid::Uuid;

const MAX_CLIENT_ID_FILE_BYTES: u64 = 256;

#[derive(Parser)]
#[command(
    name = "music-output",
    version,
    about = "Rust headless audio output for the Music server"
)]
pub struct OutputArgs {
    /// Music server base URL.
    #[arg(long, env = "MUSIC_SERVER_URL")]
    pub server: Option<String>,
    /// Device name shown in the Console.
    #[arg(long, env = "MUSIC_OUTPUT_NAME")]
    pub name: Option<String>,
    /// Stable device identity; otherwise generated and persisted.
    #[arg(long, env = "MUSIC_CLIENT_ID")]
    pub client_id: Option<String>,
    /// Directory containing the generated client identity.
    #[arg(long, env = "MUSIC_STATE_DIR")]
    pub state_dir: Option<PathBuf>,
    /// Serve the optional local control endpoint on this port.
    #[arg(long)]
    pub control_port: Option<u16>,
    /// Address for the optional control endpoint.
    #[arg(long, env = "MUSIC_CONTROL_BIND", default_value = "127.0.0.1")]
    pub control_bind: String,
    /// Require X-Control-Token on control requests.
    #[arg(long, env = "MUSIC_CONTROL_TOKEN")]
    pub control_token: Option<String>,
    /// Require activation in the server Console in addition to local on/off.
    #[arg(long)]
    pub respect_console: bool,
    /// Disable sound-effect playback.
    #[arg(long)]
    pub no_sfx: bool,
    /// Boot locally muted.
    #[arg(long)]
    pub start_off: bool,
    /// Hardware-local gain from 0 through 1.
    #[arg(long, env = "MUSIC_VOLUME", default_value_t = 1.0)]
    pub volume: f64,
    /// mpv executable used for both isolated audio lanes.
    #[arg(long, env = "MUSIC_MPV", default_value = "mpv")]
    pub mpv: PathBuf,
}

#[derive(Clone)]
pub struct OutputConfig {
    pub server_url: Url,
    pub name: String,
    pub client_id: String,
    pub state_dir: PathBuf,
    pub control_port: Option<u16>,
    pub control_bind: String,
    pub control_token: Option<String>,
    pub respect_console: bool,
    pub play_sfx: bool,
    pub local_on: bool,
    pub local_volume: f64,
    pub mpv_executable: PathBuf,
}

impl fmt::Debug for OutputConfig {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("OutputConfig")
            .field("server_url", &self.server_url)
            .field("name", &self.name)
            .field("client_id", &self.client_id)
            .field("state_dir", &self.state_dir)
            .field("control_port", &self.control_port)
            .field("control_bind", &self.control_bind)
            .field(
                "control_token",
                &self.control_token.as_ref().map(|_| "[REDACTED]"),
            )
            .field("respect_console", &self.respect_console)
            .field("play_sfx", &self.play_sfx)
            .field("local_on", &self.local_on)
            .field("local_volume", &self.local_volume)
            .field("mpv_executable", &self.mpv_executable)
            .finish()
    }
}

#[derive(Debug)]
pub struct OutputConfigError(String);

impl Display for OutputConfigError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl std::error::Error for OutputConfigError {}

impl OutputConfig {
    pub fn resolve(args: OutputArgs) -> Result<Self, OutputConfigError> {
        let server = args.server.ok_or_else(|| {
            OutputConfigError("--server (or MUSIC_SERVER_URL) is required".to_owned())
        })?;
        let mut server_url = Url::parse(server.trim())
            .map_err(|_| OutputConfigError("server URL is invalid".to_owned()))?;
        if !matches!(server_url.scheme(), "http" | "https") || server_url.host_str().is_none() {
            return Err(OutputConfigError(
                "server URL must be an absolute http or https URL".to_owned(),
            ));
        }
        server_url.set_query(None);
        server_url.set_fragment(None);
        let server_path = server_url.path().trim_end_matches('/').to_owned();
        server_url.set_path(&server_path);

        let name = args.name.unwrap_or_else(default_output_name);
        BoundedText::<1, 128>::new(name.clone())
            .map_err(|error| OutputConfigError(format!("output name is invalid: {error}")))?;
        if !args.volume.is_finite() || !(0.0..=1.0).contains(&args.volume) {
            return Err(OutputConfigError(
                "volume must be finite and between 0 and 1".to_owned(),
            ));
        }
        if args.control_bind.trim().is_empty() {
            return Err(OutputConfigError(
                "control bind address must not be empty".to_owned(),
            ));
        }
        if args.control_token.as_deref().is_some_and(str::is_empty) {
            return Err(OutputConfigError(
                "control token must not be empty".to_owned(),
            ));
        }
        let state_dir = args.state_dir.unwrap_or_else(default_state_dir);
        let client_id = resolve_client_id(args.client_id.as_deref(), &state_dir)?;
        let control_port = args.control_port.or_else(control_port_from_environment);
        let environment_starts_off =
            std::env::var_os("MUSIC_START_ON").is_some_and(|value| value == "0");
        Ok(Self {
            server_url,
            name,
            client_id,
            state_dir,
            control_port,
            control_bind: args.control_bind,
            control_token: args.control_token,
            respect_console: args.respect_console,
            play_sfx: !args.no_sfx,
            local_on: !(args.start_off || environment_starts_off),
            local_volume: args.volume,
            mpv_executable: args.mpv,
        })
    }

    #[must_use]
    pub fn websocket_url(&self) -> Url {
        let mut url = self.server_url.clone();
        let scheme = if url.scheme() == "https" { "wss" } else { "ws" };
        let _ = url.set_scheme(scheme);
        url.set_path("/api/ws");
        url.set_query(None);
        url.set_fragment(None);
        url
    }
}

fn control_port_from_environment() -> Option<u16> {
    let value = std::env::var("MUSIC_CONTROL_PORT").ok()?;
    match value.parse() {
        Ok(port) => Some(port),
        Err(_) => {
            tracing::warn!(value = %value, "ignoring invalid MUSIC_CONTROL_PORT");
            None
        }
    }
}

fn default_output_name() -> String {
    std::env::var("HOSTNAME")
        .or_else(|_| std::env::var("COMPUTERNAME"))
        .ok()
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| "music-output".to_owned())
}

fn default_state_dir() -> PathBuf {
    if let Some(path) = std::env::var_os("XDG_CONFIG_HOME") {
        return PathBuf::from(path).join("music-output");
    }
    if let Some(path) = std::env::var_os("HOME") {
        return PathBuf::from(path).join(".config/music-output");
    }
    std::env::temp_dir().join("music-output")
}

fn resolve_client_id(
    explicit: Option<&str>,
    state_dir: &Path,
) -> Result<String, OutputConfigError> {
    if let Some(explicit) = explicit {
        validate_client_id(explicit)?;
        return Ok(explicit.to_owned());
    }
    fs::create_dir_all(state_dir)
        .map_err(|error| OutputConfigError(format!("could not create state directory: {error}")))?;
    validate_state_directory(state_dir)?;
    let path = state_dir.join("client-id");
    match read_client_id(&path) {
        Ok(Some(existing)) => return Ok(existing),
        Ok(None) => {}
        Err(error) => return Err(error),
    }
    let generated = format!("headless-{}", Uuid::new_v4());
    let mut file = match OpenOptions::new().write(true).create_new(true).open(&path) {
        Ok(file) => file,
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
            return read_client_id(&path)?
                .ok_or_else(|| OutputConfigError("persisted client id is empty".to_owned()));
        }
        Err(error) => {
            return Err(OutputConfigError(format!(
                "could not persist client id: {error}"
            )));
        }
    };
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        file.set_permissions(fs::Permissions::from_mode(0o600))
            .map_err(|error| {
                OutputConfigError(format!("could not secure client id file: {error}"))
            })?;
    }
    file.write_all(generated.as_bytes())
        .and_then(|()| file.write_all(b"\n"))
        .and_then(|()| file.sync_all())
        .map_err(|error| OutputConfigError(format!("could not persist client id: {error}")))?;
    Ok(generated)
}

fn validate_state_directory(path: &Path) -> Result<(), OutputConfigError> {
    let metadata = fs::symlink_metadata(path).map_err(|error| {
        OutputConfigError(format!("could not inspect state directory: {error}"))
    })?;
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return Err(OutputConfigError(
            "state directory must be a regular directory, not a symbolic link".to_owned(),
        ));
    }
    Ok(())
}

fn read_client_id(path: &Path) -> Result<Option<String>, OutputConfigError> {
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => {
            return Err(OutputConfigError(format!(
                "could not inspect client id: {error}"
            )));
        }
    };
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        return Err(OutputConfigError(
            "client id path must be a regular file, not a symbolic link".to_owned(),
        ));
    }
    if metadata.len() > MAX_CLIENT_ID_FILE_BYTES {
        return Err(OutputConfigError(
            "persisted client id is too large".to_owned(),
        ));
    }
    let mut bytes = Vec::new();
    File::open(path)
        .and_then(|file| {
            file.take(MAX_CLIENT_ID_FILE_BYTES + 1)
                .read_to_end(&mut bytes)
        })
        .map_err(|error| OutputConfigError(format!("could not read client id: {error}")))?;
    let value = String::from_utf8(bytes)
        .map_err(|_| OutputConfigError("persisted client id is not UTF-8".to_owned()))?;
    let value = value.trim();
    validate_client_id(value)?;
    Ok(Some(value.to_owned()))
}

fn validate_client_id(value: &str) -> Result<(), OutputConfigError> {
    BoundedText::<1, 64>::new(value.to_owned())
        .map(|_| ())
        .map_err(|error| OutputConfigError(format!("client id is invalid: {error}")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn persists_and_reuses_a_bounded_client_identity() -> Result<(), Box<dyn std::error::Error>> {
        let directory = tempfile::tempdir()?;
        let first = resolve_client_id(None, directory.path())?;
        let second = resolve_client_id(None, directory.path())?;
        assert_eq!(first, second);
        assert!(first.starts_with("headless-"));
        Ok(())
    }

    #[test]
    fn rejects_invalid_server_and_local_volume() -> Result<(), Box<dyn std::error::Error>> {
        let directory = tempfile::tempdir()?;
        let invalid_server = OutputArgs {
            server: Some("file:///tmp/music".to_owned()),
            name: Some("fixture".to_owned()),
            client_id: Some("fixture-client".to_owned()),
            state_dir: Some(directory.path().to_path_buf()),
            control_port: None,
            control_bind: "127.0.0.1".to_owned(),
            control_token: None,
            respect_console: false,
            no_sfx: false,
            start_off: false,
            volume: 1.0,
            mpv: PathBuf::from("mpv"),
        };
        assert!(OutputConfig::resolve(invalid_server).is_err());

        let invalid_volume = OutputArgs {
            server: Some("https://music.test".to_owned()),
            name: Some("fixture".to_owned()),
            client_id: Some("fixture-client".to_owned()),
            state_dir: Some(directory.path().to_path_buf()),
            control_port: None,
            control_bind: "127.0.0.1".to_owned(),
            control_token: None,
            respect_console: false,
            no_sfx: false,
            start_off: false,
            volume: f64::NAN,
            mpv: PathBuf::from("mpv"),
        };
        assert!(OutputConfig::resolve(invalid_volume).is_err());
        Ok(())
    }
}
