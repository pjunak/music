#![deny(clippy::expect_used, clippy::panic, clippy::unwrap_used)]
#![forbid(unsafe_code)]

use std::error::Error;
use std::io::{self, BufRead, Read, Write};
use std::net::{TcpStream, ToSocketAddrs};
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::str::FromStr;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use clap::{Parser, Subcommand, ValueEnum};
use music_application::assistant::{
    PlaylistQualityEvaluationResult, evaluate_local_playlist_suite, load_playlist_quality_suite,
};
use music_application::auth::{AuthRepository, UnixSeconds};
use music_application::modes::ModeCatalogSource;
use music_media::FilesystemModeCatalogSource;
use music_server::{
    AppConfig, RestoreOptions, check_contracts, evaluate_configured_playlist_suite,
    export_contracts, initialize_storage, load_configured_credential_vault,
    recover_interrupted_restore, restore_backup,
};
use music_storage::{
    CredentialVault, DeviceImportOutcome, ProviderCredentialAudit,
    ProviderCredentialRotationOutcome, SchemaReport, SecretString, SqliteStorage,
    SqliteStorageOptions, StorageError, hash_password,
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
    /// Restore or recover a versioned application backup while the server is stopped.
    Backup {
        #[command(subcommand)]
        command: BackupCommand,
    },
    /// Create the configured media directories without starting the server.
    #[command(name = "init-storage")]
    InitStorage {
        /// Seed the modes directory when it is empty.
        #[arg(long)]
        seed: bool,
        /// Emit a stable machine-readable outcome.
        #[arg(long)]
        json: bool,
    },
    /// Parse every authored mode and report isolated mode errors.
    #[command(name = "reload-modes")]
    ReloadModes {
        /// Modes directory; defaults to MODES_DIR from the application configuration.
        #[arg(long, value_name = "PATH")]
        modes: Option<PathBuf>,
        /// Emit a stable machine-readable outcome.
        #[arg(long)]
        json: bool,
    },
    /// Run a versioned playlist recommendation evaluation suite.
    #[command(name = "evaluate-playlists")]
    EvaluatePlaylists {
        #[arg(value_name = "SUITE")]
        suite: PathBuf,
        /// Planner implementation to evaluate.
        #[arg(long, value_enum, default_value_t = PlaylistEngine::Local)]
        engine: PlaylistEngine,
        /// Explicitly permit synthetic suite content to leave this machine.
        #[arg(long)]
        send_suite_to_provider: bool,
        /// Emit the full machine-readable result.
        #[arg(long)]
        json: bool,
    },
    /// Audit or rotate provider credential encryption while the server is stopped.
    #[command(name = "assistant-credentials")]
    AssistantCredentials {
        #[command(subcommand)]
        command: CredentialCommand,
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

#[derive(Debug, Clone, Copy, Eq, PartialEq, ValueEnum)]
enum PlaylistEngine {
    Local,
    ConfiguredModel,
}

#[derive(Debug, Subcommand)]
enum CredentialCommand {
    /// Verify that the configured key decrypts every saved provider credential.
    Check {
        /// Database file; defaults to DATABASE_URL from the application configuration.
        #[arg(long, value_name = "PATH")]
        database: Option<PathBuf>,
        /// Emit a stable machine-readable report.
        #[arg(long)]
        json: bool,
    },
    /// Validate or atomically rotate to ASSISTANT_CREDENTIAL_KEY_NEW.
    Rotate {
        /// Commit the rotation; omission performs a read-only dry run.
        #[arg(long)]
        apply: bool,
        /// Confirm that every server using this database has been stopped.
        #[arg(long)]
        server_stopped: bool,
        /// Database file; defaults to DATABASE_URL from the application configuration.
        #[arg(long, value_name = "PATH")]
        database: Option<PathBuf>,
        /// Emit a stable machine-readable outcome.
        #[arg(long)]
        json: bool,
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

#[derive(Debug, Subcommand)]
enum BackupCommand {
    /// Verify, stage, and journal the database and modes-tree replacement.
    Restore {
        #[arg(value_name = "ARCHIVE")]
        archive: PathBuf,
        /// Database file; defaults to DATABASE_URL from the application configuration.
        #[arg(long, value_name = "PATH")]
        database: Option<PathBuf>,
        /// Modes directory; defaults to MODES_DIR from the application configuration.
        #[arg(long, value_name = "PATH")]
        modes: Option<PathBuf>,
        /// Confirm replacement of current database and modes targets.
        #[arg(long)]
        replace: bool,
        /// Confirm that every server using this database has been stopped.
        #[arg(long)]
        server_stopped: bool,
        /// Emit a stable machine-readable outcome.
        #[arg(long)]
        json: bool,
    },
    /// Roll back an interrupted restore from its durable journal.
    Recover {
        /// Database file; defaults to DATABASE_URL from the application configuration.
        #[arg(long, value_name = "PATH")]
        database: Option<PathBuf>,
        /// Modes directory; defaults to MODES_DIR from the application configuration.
        #[arg(long, value_name = "PATH")]
        modes: Option<PathBuf>,
        /// Confirm that every server using this database has been stopped.
        #[arg(long)]
        server_stopped: bool,
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
        Command::Backup {
            command:
                BackupCommand::Restore {
                    archive,
                    database,
                    modes,
                    replace,
                    server_stopped,
                    json,
                },
        } => {
            let config = AppConfig::load()?;
            let database_path = database.unwrap_or_else(|| config.database_path.clone());
            let modes_path = modes.unwrap_or_else(|| config.modes_dir.clone());
            let outcome = restore_backup(
                &config,
                RestoreOptions {
                    archive_path: archive,
                    database_path,
                    modes_path,
                    replace,
                    server_stopped,
                },
            )
            .await?;
            print_restore_outcome(&outcome, json)?;
            Ok(ExitCode::SUCCESS)
        }
        Command::Backup {
            command:
                BackupCommand::Recover {
                    database,
                    modes,
                    server_stopped,
                    json,
                },
        } => {
            let config = AppConfig::load()?;
            let database = database.unwrap_or_else(|| config.database_path.clone());
            let modes = modes.unwrap_or_else(|| config.modes_dir.clone());
            let outcome = recover_interrupted_restore(&database, &modes, server_stopped)?;
            if json {
                println!("{}", serde_json::to_string_pretty(&outcome)?);
            } else {
                println!(
                    "recovered interrupted restore recorded by {}",
                    outcome.journal_path.display()
                );
                for path in &outcome.recovered_targets {
                    println!("recovered: {}", path.display());
                }
                for path in &outcome.preserved_interrupted_targets {
                    println!("preserved interrupted target: {}", path.display());
                }
            }
            Ok(ExitCode::SUCCESS)
        }
        Command::InitStorage { seed, json } => {
            let config = AppConfig::load()?;
            let outcome = initialize_storage(&config, seed)?;
            if json {
                println!("{}", serde_json::to_string_pretty(&outcome)?);
            } else {
                println!("music    {}", outcome.music_dir.display());
                println!("sfx      {}", outcome.sfx_library_dir.display());
                println!("modes    {}", outcome.modes_dir.display());
                println!("mode seed: {:?}", outcome.mode_seed);
            }
            Ok(ExitCode::SUCCESS)
        }
        Command::ReloadModes { modes, json } => reload_modes(modes, json).await,
        Command::EvaluatePlaylists {
            suite,
            engine,
            send_suite_to_provider,
            json,
        } => evaluate_playlists(&suite, engine, send_suite_to_provider, json).await,
        Command::AssistantCredentials { command } => match command {
            CredentialCommand::Check { database, json } => {
                check_provider_credentials(database, json).await
            }
            CredentialCommand::Rotate {
                apply,
                server_stopped,
                database,
                json,
            } => rotate_provider_credentials(database, apply, server_stopped, json).await,
        },
        Command::Healthcheck {
            address,
            timeout_ms,
        } => {
            probe_liveness(&address, Duration::from_millis(timeout_ms))?;
            Ok(ExitCode::SUCCESS)
        }
    }
}

async fn reload_modes(modes: Option<PathBuf>, json: bool) -> Result<ExitCode, CliError> {
    let config = AppConfig::load()?;
    let path = modes.unwrap_or(config.modes_dir);
    let attempt = FilesystemModeCatalogSource::open(&path)?.load().await?;
    if json {
        let modes = attempt
            .modes
            .iter()
            .map(|(id, mode)| {
                serde_json::json!({
                    "id": id,
                    "name": mode.manifest.name,
                    "panels": mode.manifest.panels.len(),
                    "soundboards": mode.soundboards.len(),
                    "cues": mode.cues.len(),
                    "presets": mode.presets.len(),
                })
            })
            .collect::<Vec<_>>();
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({
                "modes": modes,
                "errors": attempt.errors,
            }))?
        );
    } else if attempt.modes.is_empty() && attempt.errors.is_empty() {
        println!("no modes loaded");
    } else {
        for (id, mode) in &attempt.modes {
            println!(
                "- {id}: {} ({} panels, {} soundboards, {} cues, {} presets)",
                mode.manifest.name,
                mode.manifest.panels.len(),
                mode.soundboards.len(),
                mode.cues.len(),
                mode.presets.len(),
            );
        }
        for (id, error) in &attempt.errors {
            println!("! {id}: {error}");
        }
    }
    Ok(if !attempt.errors.is_empty() {
        ExitCode::from(INCOMPATIBLE_EXIT_CODE)
    } else if attempt.modes.is_empty() {
        ExitCode::FAILURE
    } else {
        ExitCode::SUCCESS
    })
}

async fn evaluate_playlists(
    suite_path: &Path,
    engine: PlaylistEngine,
    send_suite_to_provider: bool,
    json: bool,
) -> Result<ExitCode, CliError> {
    let suite = match load_playlist_quality_suite(suite_path) {
        Ok(suite) => suite,
        Err(error) => {
            eprintln!("Could not load evaluation suite: {}", error.code);
            return Ok(ExitCode::from(INCOMPATIBLE_EXIT_CODE));
        }
    };
    let result = match engine {
        PlaylistEngine::Local => evaluate_local_playlist_suite(&suite)?,
        PlaylistEngine::ConfiguredModel => {
            if !send_suite_to_provider {
                eprintln!(
                    "Configured-model evaluation requires --send-suite-to-provider. Only run it with a synthetic suite you are willing to disclose."
                );
                return Ok(ExitCode::from(INCOMPATIBLE_EXIT_CODE));
            }
            let config = AppConfig::load()?;
            match evaluate_configured_playlist_suite(&config, &config.database_path, &suite).await {
                Ok(result) => result,
                Err(error) => {
                    eprintln!(
                        "Could not prepare or execute configured playlist model ({}).",
                        error.code()
                    );
                    return Ok(ExitCode::from(INCOMPATIBLE_EXIT_CODE));
                }
            }
        }
    };
    print_playlist_evaluation(&result, json)?;
    Ok(if result.passed {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    })
}

fn print_playlist_evaluation(
    result: &PlaylistQualityEvaluationResult,
    json: bool,
) -> Result<(), serde_json::Error> {
    if json {
        println!("{}", serde_json::to_string_pretty(result)?);
        return Ok(());
    }
    println!(
        "{} {} with {}: {}/{} cases passed",
        if result.passed { "PASS" } else { "FAIL" },
        result.suite_id,
        result.engine_id,
        result.summary.passed_cases,
        result.summary.cases,
    );
    for case in &result.cases {
        println!(
            "  {} {}: precision={:.2}, recall={:.2}, rr={:.2}, reasons={:.2}",
            if case.passed { "PASS" } else { "FAIL" },
            case.id,
            case.metrics.precision_at_k,
            case.metrics.recall_at_k,
            case.metrics.reciprocal_rank,
            case.metrics.reason_coverage,
        );
        for failure in &case.failures {
            println!("    - {failure}");
        }
    }
    Ok(())
}

async fn check_provider_credentials(
    database: Option<PathBuf>,
    json: bool,
) -> Result<ExitCode, CliError> {
    let config = AppConfig::load()?;
    let vault = match load_configured_credential_vault(&config) {
        Ok(vault) => vault,
        Err(error) => {
            eprintln!("Credential key is not usable ({}).", error.code());
            return Ok(ExitCode::from(INCOMPATIBLE_EXIT_CODE));
        }
    };
    let path = database.unwrap_or(config.database_path);
    let storage = SqliteStorage::open(SqliteStorageOptions::new(path)).await?;
    let audit = storage.audit_provider_credentials(&vault).await?;
    print_credential_audit(&audit, json)?;
    let healthy = audit.healthy();
    storage.close().await;
    Ok(if healthy {
        ExitCode::SUCCESS
    } else {
        ExitCode::from(INCOMPATIBLE_EXIT_CODE)
    })
}

async fn rotate_provider_credentials(
    database: Option<PathBuf>,
    apply: bool,
    server_stopped: bool,
    json: bool,
) -> Result<ExitCode, CliError> {
    let config = AppConfig::load()?;
    let current = match load_configured_credential_vault(&config) {
        Ok(vault) => vault,
        Err(error) => {
            eprintln!("Credential key is not usable ({}).", error.code());
            return Ok(ExitCode::from(INCOMPATIBLE_EXIT_CODE));
        }
    };
    let Some(encoded_new_key) = std::env::var_os("ASSISTANT_CREDENTIAL_KEY_NEW") else {
        eprintln!("Set ASSISTANT_CREDENTIAL_KEY_NEW to a new URL-safe base64 32-byte key first.");
        return Ok(ExitCode::from(INCOMPATIBLE_EXIT_CODE));
    };
    let Some(encoded_new_key) = encoded_new_key.to_str() else {
        eprintln!("New credential key is not usable (invalid_master_key).");
        return Ok(ExitCode::from(INCOMPATIBLE_EXIT_CODE));
    };
    let replacement = match CredentialVault::from_encoded_key(encoded_new_key) {
        Ok(vault) => vault,
        Err(_) => {
            eprintln!("New credential key is not usable (invalid_master_key).");
            return Ok(ExitCode::from(INCOMPATIBLE_EXIT_CODE));
        }
    };
    if current.key_id() == replacement.key_id() {
        eprintln!("The new key is the same as the current key.");
        return Ok(ExitCode::from(INCOMPATIBLE_EXIT_CODE));
    }
    let path = database.unwrap_or(config.database_path);
    let storage = SqliteStorage::open(SqliteStorageOptions::new(path)).await?;
    let audit = storage.audit_provider_credentials(&current).await?;
    if !json {
        print_credential_audit(&audit, false)?;
    }
    if !audit.healthy() {
        storage.close().await;
        if json {
            println!(
                "{}",
                serde_json::to_string_pretty(&serde_json::json!({
                    "status": "unreadable_credentials",
                    "audit": audit,
                    "database_changed": false,
                }))?
            );
        }
        eprintln!("Rotation stopped: the current key cannot decrypt every saved credential.");
        return Ok(ExitCode::from(INCOMPATIBLE_EXIT_CODE));
    }
    if !apply {
        if json {
            println!(
                "{}",
                serde_json::to_string_pretty(&serde_json::json!({
                    "status": "dry_run_passed",
                    "audit": audit,
                    "new_key_id": replacement.key_id(),
                    "database_changed": false,
                }))?
            );
        } else {
            println!("new key id: {}", replacement.key_id());
            println!("Dry run passed. No database rows were changed.");
            println!("Stop the Music server, then rerun with --apply --server-stopped.");
        }
        storage.close().await;
        return Ok(ExitCode::SUCCESS);
    }
    if !server_stopped {
        storage.close().await;
        eprintln!("Refusing to rotate without --server-stopped.");
        return Ok(ExitCode::from(INCOMPATIBLE_EXIT_CODE));
    }
    let outcome = storage
        .rotate_provider_credentials(&current, &replacement)
        .await?;
    storage.close().await;
    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({
                "audit": audit,
                "new_key_id": replacement.key_id(),
                "outcome": outcome,
            }))?
        );
    }
    match outcome {
        ProviderCredentialRotationOutcome::Applied {
            rotated_credentials,
        } => {
            if !json {
                println!("Rotated {rotated_credentials} saved provider credential(s) atomically.");
                println!(
                    "Before restarting, configure the credential key whose id is {}.",
                    replacement.key_id()
                );
                println!("Provider connections must be verified and model quality gates rerun.");
            }
            Ok(ExitCode::SUCCESS)
        }
        ProviderCredentialRotationOutcome::UnreadableCredentials { .. }
        | ProviderCredentialRotationOutcome::ModelJobActive
        | ProviderCredentialRotationOutcome::ChangedDuringPreflight => {
            if !json {
                eprintln!("Rotation failed before commit ({outcome:?}).");
            }
            Ok(ExitCode::from(INCOMPATIBLE_EXIT_CODE))
        }
    }
}

fn print_credential_audit(
    audit: &ProviderCredentialAudit,
    json: bool,
) -> Result<(), serde_json::Error> {
    if json {
        println!("{}", serde_json::to_string_pretty(audit)?);
    } else {
        println!("key id: {}", audit.key_id);
        println!("connections: {}", audit.total_connections);
        println!("saved credentials: {}", audit.saved_credentials);
        println!(
            "connections without a credential: {}",
            audit.connections_without_credentials
        );
        println!("unreadable credentials: {}", audit.unreadable_credentials);
    }
    Ok(())
}

fn print_restore_outcome(
    outcome: &music_server::RestoreOutcome,
    json: bool,
) -> Result<(), serde_json::Error> {
    if json {
        println!("{}", serde_json::to_string_pretty(outcome)?);
        return Ok(());
    }
    println!("restored database: {}", outcome.database_path.display());
    println!("restored modes: {}", outcome.modes_path.display());
    println!("database sha256: {}", outcome.database_sha256);
    println!("restored mode files: {}", outcome.restored_mode_files);
    if let Some(key_id) = &outcome.credential_key_id {
        println!("credential key id: {key_id}");
    }
    if let Some(path) = &outcome.previous_database_path {
        println!("retained previous database: {}", path.display());
    }
    if let Some(path) = &outcome.previous_modes_path {
        println!("retained previous modes: {}", path.display());
    }
    for path in &outcome.previous_sidecar_paths {
        println!("retained previous SQLite sidecar: {}", path.display());
    }
    Ok(())
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
    if liveness_status_is_ok(&mut stream)? {
        Ok(())
    } else {
        Err(io::Error::other(
            "liveness endpoint did not return HTTP 200",
        ))
    }
}

fn liveness_status_is_ok(reader: &mut impl Read) -> io::Result<bool> {
    let mut response = [0_u8; 512];
    let mut response_len = 0;
    while response_len < response.len() {
        match reader.read(&mut response[response_len..]) {
            Ok(0) => break,
            Ok(read) => {
                response_len += read;
                if response[..response_len].contains(&b'\n') {
                    break;
                }
            }
            Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
            Err(error) => return Err(error),
        }
    }

    let status_line_end = response[..response_len]
        .iter()
        .position(|byte| *byte == b'\n')
        .unwrap_or(response_len);
    let status_line = &response[..status_line_end];
    Ok(status_line.starts_with(b"HTTP/1.1 200 ") || status_line.starts_with(b"HTTP/1.0 200 "))
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
    use std::io::{self, Cursor, Read, Write};
    use std::net::TcpListener;
    use std::thread;
    use std::time::Duration;

    use clap::Parser;

    use super::{
        BackupCommand, Cli, Command, CredentialCommand, DeviceCommand, PlaylistEngine,
        liveness_status_is_ok, probe_liveness,
    };

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

        let restore = Cli::try_parse_from([
            "music-cli",
            "backup",
            "restore",
            "music-backup.tar.gz",
            "--replace",
            "--server-stopped",
        ])?;
        assert!(matches!(
            restore.command,
            Command::Backup {
                command: BackupCommand::Restore {
                    replace: true,
                    server_stopped: true,
                    ..
                }
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

        let evaluate = Cli::try_parse_from([
            "music-cli",
            "evaluate-playlists",
            "suite.json",
            "--engine",
            "configured-model",
            "--send-suite-to-provider",
        ])?;
        assert!(matches!(
            evaluate.command,
            Command::EvaluatePlaylists {
                engine: PlaylistEngine::ConfiguredModel,
                send_suite_to_provider: true,
                ..
            }
        ));

        let rotate = Cli::try_parse_from([
            "music-cli",
            "assistant-credentials",
            "rotate",
            "--apply",
            "--server-stopped",
        ])?;
        assert!(matches!(
            rotate.command,
            Command::AssistantCredentials {
                command: CredentialCommand::Rotate {
                    apply: true,
                    server_stopped: true,
                    ..
                }
            }
        ));
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

    #[test]
    fn healthcheck_reads_a_fragmented_http_status_line() -> Result<(), Box<dyn Error>> {
        let response = b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\n\r\n";
        let mut reader = FragmentedReader {
            inner: Cursor::new(response),
            maximum_read: 1,
        };

        assert!(liveness_status_is_ok(&mut reader)?);
        Ok(())
    }

    struct FragmentedReader<T> {
        inner: Cursor<T>,
        maximum_read: usize,
    }

    impl<T: AsRef<[u8]>> Read for FragmentedReader<T> {
        fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
            let read_len = buffer.len().min(self.maximum_read);
            self.inner.read(&mut buffer[..read_len])
        }
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
