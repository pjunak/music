#![deny(clippy::expect_used, clippy::panic, clippy::unwrap_used)]
#![forbid(unsafe_code)]

use std::error::Error;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use clap::{Parser, Subcommand};
use music_server::AppConfig;
use music_storage::{SchemaReport, SqliteStorage, SqliteStorageOptions, StorageError};

const INCOMPATIBLE_EXIT_CODE: u8 = 2;

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

async fn run(cli: Cli) -> Result<ExitCode, Box<dyn Error>> {
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
    }
}

fn database_path(override_path: Option<PathBuf>) -> Result<PathBuf, Box<dyn Error>> {
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
