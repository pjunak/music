#![deny(clippy::expect_used, clippy::panic, clippy::unwrap_used)]
#![forbid(unsafe_code)]

use std::error::Error;
use std::io::{self, BufRead, Read, Write};
use std::net::{TcpStream, ToSocketAddrs};
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::str::FromStr;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use clap::{Parser, Subcommand};
use music_application::auth::{AuthRepository, UnixSeconds};
use music_server::{AppConfig, check_contracts, export_contracts};
use music_storage::{
    DeviceImportOutcome, SchemaReport, SecretString, SqliteStorage, SqliteStorageOptions,
    StorageError, hash_password,
};

const INCOMPATIBLE_EXIT_CODE: u8 = 2;
type CliError = Box<dyn Error + Send + Sync>;

struct PasswordArgument(SecretString);

// Clap clones parsed values while building `ArgMatches`. Keep that required
// clone explicit so the inner secret never gains a generally available Clone
// implementation or an accidentally revealing Debug implementation.
impl Clone for PasswordArgument {
    fn clone(&self) -> Self {
        Self(SecretString::new(self.0.expose_secret()))
    }
}

impl std::fmt::Debug for PasswordArgument {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("PasswordArgument([REDACTED])")
    }
}

impl FromStr for PasswordArgument {
    type Err = std::convert::Infallible;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Ok(Self(SecretString::new(value)))
    }
}

impl PasswordArgument {
    fn into_secret(self) -> SecretString {
        self.0
    }
}

#[derive(Debug, Parser)]
#[command(
    name = "music-cli",
    version,
    about = "Offline administration for the Rust music server"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Inspect or migrate the SQLite application database.
    Db {
        #[command(subcommand)]
        command: DatabaseCommand,
    },
    /// Generate or verify browser and HTTP contracts.
    Contracts {
        #[command(subcommand)]
        command: ContractCommand,
    },
    /// Create an administrator account while the server is stopped.
    #[command(name = "create-user")]
    CreateUser {
        username: String,
        /// Password value; prefer the hidden prompt or --password-stdin.
        #[arg(long, conflicts_with = "password_stdin")]
        password: Option<PasswordArgument>,
        /// Read exactly one password line from standard input.
        #[arg(long)]
        password_stdin: bool,
        /// Database file; defaults to DATABASE_URL from the application configuration.
        #[arg(long, value_name = "PATH")]
        database: Option<PathBuf>,
    },
    /// Change an administrator password while the server is stopped.
    #[command(name = "set-password")]
    SetPassword {
        username: String,
        /// Password value; prefer the hidden prompt or --password-stdin.
        #[arg(long, conflicts_with = "password_stdin")]
        password: Option<PasswordArgument>,
        /// Read exactly one password line from standard input.
        #[arg(long)]
        password_stdin: bool,
        /// Preserve active sessions instead of revoking them after the change.
        #[arg(long)]
        keep_sessions: bool,
        /// Database file; defaults to DATABASE_URL from the application configuration.
        #[arg(long, value_name = "PATH")]
        database: Option<PathBuf>,
    },
    /// Export or import the SQLite-owned remembered-device registry.
    Devices {
        #[command(subcommand)]
        command: DeviceCommand,
    },
    /// Probe the local HTTP liveness endpoint (used by the Rust container).
    Healthcheck {
        /// Plain HTTP socket address for the local server.
        #[arg(long, default_value = "127.0.0.1:8000")]
        address: String,
        /// Connection and response timeout.
        #[arg(long, default_value_t = 2_000)]
        timeout_ms: u64,
    },
}

#[derive(Debug, Subcommand)]
enum ContractCommand {
    /// Regenerate checked-in TypeScript, OpenAPI, and compatibility reports.
    Export {
        /// Repository root containing contracts/reference and frontend.
        #[arg(long, default_value = ".", value_name = "PATH")]
        root: PathBuf,
    },
    /// Fail with exit code 2 when checked-in generated contracts have drifted.
    Check {
        /// Repository root containing contracts/reference and frontend.
        #[arg(long, default_value = ".", value_name = "PATH")]
        root: PathBuf,
    },
}

#[derive(Debug, Subcommand)]
enum DatabaseCommand {
    /// Inspect integrity and compatibility without changing the database.
    Doctor {
        /// Database file; defaults to DATABASE_URL from the application configuration.
        #[arg(long, value_name = "PATH")]
        database: Option<PathBuf>,
        /// Emit a stable machine-readable report.
        #[arg(long)]
        json: bool,
    },
    /// Back up and apply all compatible pending migrations.
    Migrate {
        /// Database file; defaults to DATABASE_URL from the application configuration.
        #[arg(long, value_name = "PATH")]
        database: Option<PathBuf>,
        /// Emit a stable machine-readable outcome.
        #[arg(long)]
        json: bool,
    },
}

#[derive(Debug, Subcommand)]
enum DeviceCommand {
    /// Export a new versioned recovery document; never overwrites a target.
    Export {
        #[arg(value_name = "PATH")]
        path: PathBuf,
        /// Database file; defaults to DATABASE_URL from the application configuration.
        #[arg(long, value_name = "PATH")]
        database: Option<PathBuf>,
        /// Emit a stable machine-readable outcome.
        #[arg(long)]
        json: bool,
    },
    /// Import a versioned recovery document transactionally.
    Import {
        #[arg(value_name = "PATH")]
        path: PathBuf,
        /// Replace the complete current registry; otherwise a non-empty target is refused.
        #[arg(long)]
        replace: bool,
        /// Database file; defaults to DATABASE_URL from the application configuration.
        #[arg(long, value_name = "PATH")]
        database: Option<PathBuf>,
        /// Emit a stable machine-readable outcome.
        #[arg(long)]
        json: bool,
    },
}

#[tokio::main]
async fn main() -> ExitCode {
    match run(Cli::parse()).await {
        Ok(code) => code,
        Err(error) => {
            eprintln!("music-cli: {error}");
            ExitCode::FAILURE
        }
    }
}

async fn run(cli: Cli) -> Result<ExitCode, CliError> {
    match cli.command {
        Command::Db {
            command: DatabaseCommand::Doctor { database, json },
        } => {
            let path = database_path(database)?;
            let report = SqliteStorage::doctor(&path).await?;
            print_schema_report(&path, &report, json)?;
            Ok(if report.is_compatible() {
                ExitCode::SUCCESS
            } else {
                ExitCode::from(INCOMPATIBLE_EXIT_CODE)
            })
        }
        Command::Db {
            command: DatabaseCommand::Migrate { database, json },
        } => {
            let path = database_path(database)?;
            match SqliteStorage::open(SqliteStorageOptions::new(&path)).await {
                Ok(storage) => {
                    if json {
                        println!(
                            "{}",
                            serde_json::to_string_pretty(storage.migration_outcome())?
                        );
                    } else {
                        let outcome = storage.migration_outcome();
                        println!("database: {}", path.display());
                        println!(
                            "schema: {} -> {} (version {})",
                            outcome.schema_before.compatibility,
                            outcome.schema_after.compatibility,
                            outcome.schema_after.current_schema_version
                        );
                        println!("migration applied: {}", outcome.migration_applied);
                        if let Some(backup) = &outcome.backup {
                            println!("backup: {}", backup.database_path.display());
                            println!("manifest: {}", backup.manifest_path.display());
                            println!("backup sha256: {}", backup.sha256);
                        }
                    }
                    storage.close().await;
                    Ok(ExitCode::SUCCESS)
                }
                Err(StorageError::IncompatibleSchema(report)) => {
                    print_schema_report(&path, &report, json)?;
                    Ok(ExitCode::from(INCOMPATIBLE_EXIT_CODE))
                }
                Err(error) => Err(error.into()),
            }
        }
        Command::Contracts {
            command: ContractCommand::Export { root },
        } => {
            let changed = export_contracts(&root)?;
            if changed.is_empty() {
                println!("generated contracts are current");
            } else {
                for path in changed {
                    println!("generated {}", path.display());
                }
            }
            Ok(ExitCode::SUCCESS)
        }
        Command::Contracts {
            command: ContractCommand::Check { root },
        } => {
            let drifted = check_contracts(&root)?;
            if drifted.is_empty() {
                println!("generated contracts are current");
                Ok(ExitCode::SUCCESS)
            } else {
                for path in drifted {
                    eprintln!("generated contract drift: {}", path.display());
                }
                Ok(ExitCode::from(INCOMPATIBLE_EXIT_CODE))
            }
        }
        Command::CreateUser {
            username,
            password,
            password_stdin,
            database,
        } => {
            let path = database_path(database)?;
            create_user(&path, &username, password, password_stdin).await
        }
        Command::SetPassword {
            username,
            password,
            password_stdin,
            keep_sessions,
            database,
        } => {
            let path = database_path(database)?;
            set_password(&path, &username, password, password_stdin, keep_sessions).await
        }
        Command::Devices {
            command:
                DeviceCommand::Export {
                    path,
                    database,
                    json,
                },
        } => {
            let database = database_path(database)?;
            export_devices(&database, &path, json).await
        }
        Command::Devices {
            command:
                DeviceCommand::Import {
                    path,
                    replace,
                    database,
                    json,
                },
        } => {
            let database = database_path(database)?;
            import_devices(&database, &path, replace, json).await
        }
        Command::Healthcheck {
            address,
            timeout_ms,
        } => {
            probe_liveness(&address, Duration::from_millis(timeout_ms))?;
            Ok(ExitCode::SUCCESS)
        }
    }
}

async fn create_user(
    database: &Path,
    username: &str,
    password: Option<PasswordArgument>,
    password_stdin: bool,
) -> Result<ExitCode, CliError> {
    if let Err(detail) = validate_username(username) {
        eprintln!("error: {detail}");
        return Ok(ExitCode::from(INCOMPATIBLE_EXIT_CODE));
    }
    let storage = SqliteStorage::open(SqliteStorageOptions::new(database)).await?;
    let outcome = create_user_with_storage(&storage, username, password, password_stdin).await;
    storage.close().await;
    outcome
}

async fn create_user_with_storage(
    storage: &SqliteStorage,
    username: &str,
    password: Option<PasswordArgument>,
    password_stdin: bool,
) -> Result<ExitCode, CliError> {
    if AuthRepository::find_user_by_username(storage, username)
        .await?
        .is_some()
    {
        eprintln!("error: user '{username}' already exists");
        return Ok(ExitCode::FAILURE);
    }
    let password = password_from_source(password, password_stdin, "Password: ")?;
    if let Err(detail) = validate_password(&password) {
        eprintln!("error: {detail}");
        return Ok(ExitCode::from(INCOMPATIBLE_EXIT_CODE));
    }
    let password_hash = hash_cli_password(password).await?;
    let user_id = storage
        .create_user(username, &password_hash, current_unix_seconds()?)
        .await?;
    println!("created user '{username}' (id={user_id})");
    Ok(ExitCode::SUCCESS)
}

async fn set_password(
    database: &Path,
    username: &str,
    password: Option<PasswordArgument>,
    password_stdin: bool,
    keep_sessions: bool,
) -> Result<ExitCode, CliError> {
    if let Err(detail) = validate_username(username) {
        eprintln!("error: {detail}");
        return Ok(ExitCode::from(INCOMPATIBLE_EXIT_CODE));
    }
    let storage = SqliteStorage::open(SqliteStorageOptions::new(database)).await?;
    let outcome =
        set_password_with_storage(&storage, username, password, password_stdin, keep_sessions)
            .await;
    storage.close().await;
    outcome
}

async fn set_password_with_storage(
    storage: &SqliteStorage,
    username: &str,
    password: Option<PasswordArgument>,
    password_stdin: bool,
    keep_sessions: bool,
) -> Result<ExitCode, CliError> {
    if AuthRepository::find_user_by_username(storage, username)
        .await?
        .is_none()
    {
        eprintln!("error: user '{username}' not found");
        return Ok(ExitCode::FAILURE);
    }
    let password = password_from_source(password, password_stdin, "New password: ")?;
    if let Err(detail) = validate_password(&password) {
        eprintln!("error: {detail}");
        return Ok(ExitCode::from(INCOMPATIBLE_EXIT_CODE));
    }
    let password_hash = hash_cli_password(password).await?;
    let Some(revoked) = storage
        .replace_user_password(username, &password_hash, !keep_sessions)
        .await?
    else {
        eprintln!("error: user '{username}' disappeared during the update");
        return Ok(ExitCode::FAILURE);
    };
    if revoked == 0 {
        println!("updated password for '{username}'");
    } else {
        println!("updated password for '{username}' ({revoked} active session(s) invalidated)");
    }
    Ok(ExitCode::SUCCESS)
}

async fn export_devices(database: &Path, path: &Path, json: bool) -> Result<ExitCode, CliError> {
    let storage = SqliteStorage::open(SqliteStorageOptions::new(database)).await?;
    let outcome = storage.export_remembered_devices(path).await;
    storage.close().await;
    let outcome = outcome?;
    if json {
        println!("{}", serde_json::to_string_pretty(&outcome)?);
    } else {
        println!(
            "exported {} remembered device(s) to {} ({})",
            outcome.exported_count,
            outcome.path.display(),
            outcome.schema_version
        );
    }
    Ok(ExitCode::SUCCESS)
}

async fn import_devices(
    database: &Path,
    path: &Path,
    replace: bool,
    json: bool,
) -> Result<ExitCode, CliError> {
    let storage = SqliteStorage::open(SqliteStorageOptions::new(database)).await?;
    let outcome = storage.import_remembered_devices(path, replace).await;
    storage.close().await;
    let outcome = outcome?;
    if json {
        println!("{}", serde_json::to_string_pretty(&outcome)?);
    } else {
        match &outcome {
            DeviceImportOutcome::Imported {
                schema_version,
                imported_count,
                replaced_count,
            } => println!(
                "imported {imported_count} remembered device(s) from {} ({schema_version}; replaced {replaced_count})",
                path.display()
            ),
            DeviceImportOutcome::TargetNotEmpty { existing_count } => eprintln!(
                "error: remembered-device registry already contains {existing_count} record(s); pass --replace to replace it"
            ),
        }
    }
    Ok(match outcome {
        DeviceImportOutcome::Imported { .. } => ExitCode::SUCCESS,
        DeviceImportOutcome::TargetNotEmpty { .. } => ExitCode::from(INCOMPATIBLE_EXIT_CODE),
    })
}

fn password_from_source(
    password: Option<PasswordArgument>,
    password_stdin: bool,
    prompt: &'static str,
) -> Result<SecretString, io::Error> {
    if let Some(password) = password {
        return Ok(password.into_secret());
    }
    if password_stdin {
        let mut line = String::new();
        io::stdin().lock().read_line(&mut line)?;
        if line.ends_with('\n') {
            line.pop();
            if line.ends_with('\r') {
                line.pop();
            }
        }
        return Ok(SecretString::new(line));
    }
    rpassword::prompt_password(prompt).map(SecretString::new)
}

fn validate_username(username: &str) -> Result<(), &'static str> {
    if !(1..=64).contains(&username.chars().count()) || username.chars().any(char::is_control) {
        Err("username must contain 1 to 64 printable characters")
    } else {
        Ok(())
    }
}

fn validate_password(password: &SecretString) -> Result<(), &'static str> {
    if !(8..=256).contains(&password.expose_secret().chars().count()) {
        Err("password must contain 8 to 256 characters")
    } else {
        Ok(())
    }
}

async fn hash_cli_password(password: SecretString) -> Result<String, CliError> {
    Ok(tokio::task::spawn_blocking(move || hash_password(password.expose_secret())).await??)
}

fn current_unix_seconds() -> Result<UnixSeconds, io::Error> {
    let seconds = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|_| io::Error::other("system clock is before the Unix epoch"))?
        .as_secs();
    let seconds = i64::try_from(seconds)
        .map_err(|_| io::Error::other("system clock timestamp is too large"))?;
    Ok(UnixSeconds::new(seconds))
}

fn probe_liveness(address: &str, timeout: Duration) -> io::Result<()> {
    if timeout.is_zero() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "healthcheck timeout must be greater than zero",
        ));
    }
    let socket_address = address.to_socket_addrs()?.next().ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            "healthcheck address did not resolve",
        )
    })?;
    let mut stream = TcpStream::connect_timeout(&socket_address, timeout)?;
    stream.set_read_timeout(Some(timeout))?;
    stream.set_write_timeout(Some(timeout))?;
    stream
        .write_all(b"GET /api/health HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n")?;
    let mut response = [0_u8; 512];
    let read = stream.read(&mut response)?;
    let status_line = String::from_utf8_lossy(&response[..read]);
    if status_line.starts_with("HTTP/1.1 200 ") || status_line.starts_with("HTTP/1.0 200 ") {
        Ok(())
    } else {
        Err(io::Error::other(
            "liveness endpoint did not return HTTP 200",
        ))
    }
}

fn database_path(override_path: Option<PathBuf>) -> Result<PathBuf, CliError> {
    match override_path {
        Some(path) => Ok(path),
        None => Ok(AppConfig::load()?.database_path),
    }
}

fn print_schema_report(
    path: &Path,
    report: &SchemaReport,
    json: bool,
) -> Result<(), serde_json::Error> {
    if json {
        println!("{}", serde_json::to_string_pretty(report)?);
        return Ok(());
    }

    println!("database: {}", path.display());
    println!("exists: {}", report.database_exists);
    println!("compatibility: {}", report.compatibility);
    println!(
        "sqlite: {}",
        report.sqlite_version.as_deref().unwrap_or("not opened")
    );
    println!(
        "schema version: {} (target {})",
        report
            .migration_version
            .map_or_else(|| "none".to_owned(), |version| version.to_string()),
        report.current_schema_version
    );
    println!("tables: {}", report.table_count);
    println!(
        "integrity: {}",
        if report.integrity_ok { "ok" } else { "failed" }
    );
    println!("foreign-key violations: {}", report.foreign_key_violations);
    println!("migration required: {}", report.migration_required);
    for issue in &report.issues {
        println!("[{}] {}: {}", issue.level, issue.code, issue.detail);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::error::Error;
    use std::io::{self, Read, Write};
    use std::net::TcpListener;
    use std::thread;
    use std::time::Duration;

    use clap::Parser;

    use super::{Cli, Command, DeviceCommand, probe_liveness};

    #[test]
    fn parser_redacts_inline_passwords_and_accepts_explicit_device_replacement()
    -> Result<(), Box<dyn Error>> {
        let create = Cli::try_parse_from([
            "music-cli",
            "create-user",
            "operator",
            "--password",
            "very-secret-password",
            "--database",
            "test.db",
        ])?;
        let debug = format!("{create:?}");
        assert!(debug.contains("[REDACTED]"));
        assert!(!debug.contains("very-secret-password"));

        let import = Cli::try_parse_from([
            "music-cli",
            "devices",
            "import",
            "remembered-devices.json",
            "--replace",
        ])?;
        assert!(matches!(
            import.command,
            Command::Devices {
                command: DeviceCommand::Import { replace: true, .. }
            }
        ));

        assert!(
            Cli::try_parse_from([
                "music-cli",
                "set-password",
                "operator",
                "--password",
                "very-secret-password",
                "--password-stdin",
            ])
            .is_err()
        );
        Ok(())
    }

    #[test]
    fn healthcheck_accepts_only_an_http_200_status() -> Result<(), Box<dyn Error>> {
        let healthy = serve_once("200 OK")?;
        probe_liveness(&healthy.address, Duration::from_secs(1))?;
        healthy.join()?;

        let unavailable = serve_once("503 Service Unavailable")?;
        assert!(probe_liveness(&unavailable.address, Duration::from_secs(1)).is_err());
        unavailable.join()?;
        Ok(())
    }

    struct TestServer {
        address: String,
        thread: thread::JoinHandle<io::Result<()>>,
    }

    impl TestServer {
        fn join(self) -> io::Result<()> {
            self.thread
                .join()
                .map_err(|_| io::Error::other("test HTTP server panicked"))?
        }
    }

    fn serve_once(status: &'static str) -> io::Result<TestServer> {
        let listener = TcpListener::bind("127.0.0.1:0")?;
        let address = listener.local_addr()?.to_string();
        let thread = thread::spawn(move || {
            let (mut connection, _) = listener.accept()?;
            connection.set_read_timeout(Some(Duration::from_secs(1)))?;
            let mut request = [0_u8; 512];
            let read = connection.read(&mut request)?;
            if !String::from_utf8_lossy(&request[..read]).starts_with("GET /api/health ") {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "healthcheck requested the wrong path",
                ));
            }
            write!(
                connection,
                "HTTP/1.1 {status}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
            )?;
            Ok(())
        });
        Ok(TestServer { address, thread })
    }
}
