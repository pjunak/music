use std::fmt::{self, Display, Formatter};
use std::path::Path;
#[cfg(any(unix, test))]
use std::time::Duration;

#[cfg(any(unix, test))]
use serde_json::{Value, json};
#[cfg(any(unix, test))]
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt, BufReader};

use crate::reconcile::PlaybackCommand;

#[cfg(any(unix, test))]
const MAX_IPC_RESPONSE_BYTES: usize = 1024 * 1024;
#[cfg(any(unix, test))]
const IPC_COMMAND_TIMEOUT: Duration = Duration::from_secs(5);

#[derive(Debug)]
pub struct MpvError(String);

impl Display for MpvError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl std::error::Error for MpvError {}

impl MpvError {
    pub(crate) fn configuration(detail: impl Into<String>) -> Self {
        Self(format!(
            "could not configure output runtime: {}",
            detail.into()
        ))
    }
}

#[cfg(any(unix, test))]
#[derive(Debug)]
struct MpvConnection<R, W> {
    reader: BufReader<R>,
    writer: W,
    next_request_id: u64,
}

#[cfg(any(unix, test))]
impl<R, W> MpvConnection<R, W>
where
    R: AsyncRead + Unpin,
    W: AsyncWrite + Unpin,
{
    fn new(reader: R, writer: W) -> Self {
        Self {
            reader: BufReader::new(reader),
            writer,
            next_request_id: 1,
        }
    }

    async fn command(&mut self, command: Value) -> Result<Value, MpvError> {
        tokio::time::timeout(IPC_COMMAND_TIMEOUT, self.command_inner(command))
            .await
            .map_err(|_| MpvError("mpv IPC command timed out".to_owned()))?
    }

    async fn command_inner(&mut self, command: Value) -> Result<Value, MpvError> {
        let request_id = self.next_request_id;
        self.next_request_id = self.next_request_id.saturating_add(1);
        let request = json!({"command": command, "request_id": request_id});
        let mut encoded = serde_json::to_vec(&request)
            .map_err(|error| MpvError(format!("could not encode mpv command: {error}")))?;
        encoded.push(b'\n');
        self.writer
            .write_all(&encoded)
            .await
            .map_err(|error| MpvError(format!("could not write mpv IPC command: {error}")))?;
        self.writer
            .flush()
            .await
            .map_err(|error| MpvError(format!("could not flush mpv IPC command: {error}")))?;
        loop {
            let line = read_bounded_line(&mut self.reader).await?;
            let response: Value = serde_json::from_slice(&line)
                .map_err(|error| MpvError(format!("mpv returned invalid JSON: {error}")))?;
            if response.get("request_id").and_then(Value::as_u64) != Some(request_id) {
                continue;
            }
            let error = response
                .get("error")
                .and_then(Value::as_str)
                .unwrap_or("unknown error");
            if error != "success" {
                return Err(MpvError(format!("mpv command failed: {error}")));
            }
            return Ok(response.get("data").cloned().unwrap_or(Value::Null));
        }
    }
}

#[cfg(any(unix, test))]
async fn read_bounded_line<R>(reader: &mut BufReader<R>) -> Result<Vec<u8>, MpvError>
where
    R: AsyncRead + Unpin,
{
    let mut line = Vec::new();
    loop {
        let byte = reader
            .read_u8()
            .await
            .map_err(|error| MpvError(format!("could not read mpv IPC response: {error}")))?;
        if byte == b'\n' {
            if line.is_empty() {
                continue;
            }
            return Ok(line);
        }
        if line.len() >= MAX_IPC_RESPONSE_BYTES {
            return Err(MpvError(
                "mpv IPC response exceeded the size limit".to_owned(),
            ));
        }
        line.push(byte);
    }
}

#[cfg(unix)]
mod platform {
    use std::process::Stdio;

    use tokio::net::UnixStream;
    use tokio::net::unix::{OwnedReadHalf, OwnedWriteHalf};
    use tokio::process::{Child, Command};
    use tokio::sync::Mutex;
    use tokio::time::Instant;
    use uuid::Uuid;

    use super::*;

    type UnixMpvConnection = MpvConnection<OwnedReadHalf, OwnedWriteHalf>;

    #[derive(Debug)]
    struct MpvProcess {
        connection: UnixMpvConnection,
        child: Child,
        socket_path: std::path::PathBuf,
    }

    impl MpvProcess {
        async fn start(executable: &Path, lane: &str) -> Result<Self, MpvError> {
            let socket_path = std::env::temp_dir().join(format!(
                "music-output-{lane}-{}-{}.sock",
                std::process::id(),
                Uuid::new_v4()
            ));
            let mut child = Command::new(executable)
                .arg("--idle=yes")
                .arg("--no-video")
                .arg("--audio-display=no")
                .arg("--ytdl=no")
                .arg("--no-terminal")
                .arg("--really-quiet")
                .arg(format!("--input-ipc-server={}", socket_path.display()))
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::inherit())
                .kill_on_drop(true)
                .spawn()
                .map_err(|error| MpvError(format!("could not start mpv {lane} lane: {error}")))?;
            let deadline = Instant::now() + Duration::from_secs(5);
            loop {
                match UnixStream::connect(&socket_path).await {
                    Ok(stream) => {
                        let (reader, writer) = stream.into_split();
                        return Ok(Self {
                            connection: MpvConnection::new(reader, writer),
                            child,
                            socket_path,
                        });
                    }
                    Err(_) if Instant::now() < deadline => {
                        if let Some(status) = child.try_wait().map_err(|source| {
                            MpvError(format!("could not inspect mpv {lane} lane: {source}"))
                        })? {
                            return Err(MpvError(format!(
                                "mpv {lane} lane exited before IPC became ready: {status}"
                            )));
                        }
                        tokio::time::sleep(Duration::from_millis(50)).await;
                    }
                    Err(error) => {
                        return Err(MpvError(format!(
                            "mpv {lane} IPC did not become ready: {error}"
                        )));
                    }
                }
            }
        }

        async fn shutdown(&mut self) {
            let _ = self.child.kill().await;
            let _ = tokio::fs::remove_file(&self.socket_path).await;
        }

        fn ensure_running(&mut self, lane: &str) -> Result<(), MpvError> {
            match self.child.try_wait() {
                Ok(None) => Ok(()),
                Ok(Some(status)) => Err(MpvError(format!(
                    "mpv {lane} lane exited unexpectedly: {status}"
                ))),
                Err(error) => Err(MpvError(format!(
                    "could not inspect mpv {lane} lane: {error}"
                ))),
            }
        }
    }

    impl Drop for MpvProcess {
        fn drop(&mut self) {
            let _ = std::fs::remove_file(&self.socket_path);
        }
    }

    #[derive(Debug)]
    pub struct MpvPlayer {
        ambient: Mutex<MpvProcess>,
        sfx: Option<Mutex<MpvProcess>>,
    }

    impl MpvPlayer {
        pub async fn start(executable: &Path, enable_sfx: bool) -> Result<Self, MpvError> {
            let ambient = MpvProcess::start(executable, "ambient").await?;
            let sfx = if enable_sfx {
                match MpvProcess::start(executable, "sfx").await {
                    Ok(process) => Some(Mutex::new(process)),
                    Err(error) => {
                        let mut ambient = ambient;
                        ambient.shutdown().await;
                        return Err(error);
                    }
                }
            } else {
                None
            };
            Ok(Self {
                ambient: Mutex::new(ambient),
                sfx,
            })
        }

        pub async fn execute(&self, commands: &[PlaybackCommand]) -> Result<(), MpvError> {
            let mut ambient = self.ambient.lock().await;
            for command in commands {
                match command {
                    PlaybackCommand::SetVolume(volume) => {
                        ambient
                            .connection
                            .command(json!(["set_property", "volume", volume * 100.0]))
                            .await?;
                    }
                    PlaybackCommand::Load { url, start_seconds } => {
                        ambient
                            .connection
                            .command(json!([
                                "loadfile",
                                url,
                                "replace",
                                format!("start={}", start_seconds.max(0.0))
                            ]))
                            .await?;
                        ambient
                            .connection
                            .command(json!(["set_property", "pause", false]))
                            .await?;
                    }
                    PlaybackCommand::SetPaused(paused) => {
                        ambient
                            .connection
                            .command(json!(["set_property", "pause", paused]))
                            .await?;
                    }
                    PlaybackCommand::Stop => {
                        ambient.connection.command(json!(["stop"])).await?;
                    }
                    PlaybackCommand::SeekAbsolute(seconds) => {
                        ambient
                            .connection
                            .command(json!(["seek", seconds.max(0.0), "absolute"]))
                            .await?;
                    }
                }
            }
            Ok(())
        }

        pub async fn fire_sfx(&self, url: &str, volume: f64) -> Result<(), MpvError> {
            let Some(sfx) = self.sfx.as_ref() else {
                return Ok(());
            };
            let mut sfx = sfx.lock().await;
            sfx.connection
                .command(json!([
                    "set_property",
                    "volume",
                    volume.clamp(0.0, 1.0) * 100.0
                ]))
                .await?;
            sfx.connection
                .command(json!(["loadfile", url, "replace"]))
                .await?;
            sfx.connection
                .command(json!(["set_property", "pause", false]))
                .await?;
            Ok(())
        }

        pub async fn time_position_seconds(&self) -> Result<Option<f64>, MpvError> {
            let data = self
                .ambient
                .lock()
                .await
                .connection
                .command(json!(["get_property", "time-pos"]))
                .await?;
            Ok(data
                .as_f64()
                .filter(|value| value.is_finite() && *value >= 0.0))
        }

        pub async fn healthcheck(&self) -> Result<(), MpvError> {
            self.ambient.lock().await.ensure_running("ambient")?;
            if let Some(sfx) = self.sfx.as_ref() {
                sfx.lock().await.ensure_running("sfx")?;
            }
            Ok(())
        }

        pub async fn shutdown(&self) {
            self.ambient.lock().await.shutdown().await;
            if let Some(sfx) = self.sfx.as_ref() {
                sfx.lock().await.shutdown().await;
            }
        }
    }
}

#[cfg(not(unix))]
mod platform {
    use super::*;

    #[derive(Debug)]
    pub struct MpvPlayer;

    impl MpvPlayer {
        pub async fn start(_executable: &Path, _enable_sfx: bool) -> Result<Self, MpvError> {
            Err(MpvError(
                "the headless mpv appliance is supported on Unix systems".to_owned(),
            ))
        }

        pub async fn execute(&self, _commands: &[PlaybackCommand]) -> Result<(), MpvError> {
            Ok(())
        }

        pub async fn fire_sfx(&self, _url: &str, _volume: f64) -> Result<(), MpvError> {
            Ok(())
        }

        pub async fn time_position_seconds(&self) -> Result<Option<f64>, MpvError> {
            Ok(None)
        }

        pub async fn healthcheck(&self) -> Result<(), MpvError> {
            Ok(())
        }

        pub async fn shutdown(&self) {}
    }
}

pub use platform::MpvPlayer;

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn correlates_commands_while_ignoring_unsolicited_events()
    -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let (client, mut server) = tokio::io::duplex(4_096);
        let (reader, writer) = tokio::io::split(client);
        let fake = tokio::spawn(async move {
            let mut request = Vec::new();
            loop {
                let byte = server.read_u8().await?;
                if byte == b'\n' {
                    break;
                }
                request.push(byte);
            }
            let request: Value = serde_json::from_slice(&request)?;
            let request_id = request["request_id"].as_u64().ok_or_else(|| {
                std::io::Error::new(std::io::ErrorKind::InvalidData, "missing request id")
            })?;
            server
                .write_all(b"{\"event\":\"property-change\"}\n")
                .await?;
            server
                .write_all(
                    format!(
                        "{{\"request_id\":{request_id},\"error\":\"success\",\"data\":12.5}}\n"
                    )
                    .as_bytes(),
                )
                .await?;
            Ok::<(), Box<dyn std::error::Error + Send + Sync>>(())
        });
        let data = MpvConnection::new(reader, writer)
            .command(json!(["get_property", "time-pos"]))
            .await?;
        assert_eq!(data, json!(12.5));
        fake.await??;
        Ok(())
    }
}
