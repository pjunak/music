#![deny(clippy::expect_used, clippy::panic, clippy::unwrap_used)]
#![forbid(unsafe_code)]

use std::error::Error;
use std::io::{self, Read, Write};
use std::net::{TcpStream, ToSocketAddrs};
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::Duration;

use clap::{Parser, Subcommand};
use music_server::{AppConfig, check_contracts, export_contracts};
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
    /// Generate or verify browser and HTTP contracts.
    Contracts {
        #[command(subcommand)]
        command: ContractCommand,
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
        Command::Healthcheck {
            address,
            timeout_ms,
        } => {
            probe_liveness(&address, Duration::from_millis(timeout_ms))?;
            Ok(ExitCode::SUCCESS)
        }
    }
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

#[cfg(test)]
mod tests {
    use std::error::Error;
    use std::io::{self, Read, Write};
    use std::net::TcpListener;
    use std::thread;
    use std::time::Duration;

    use super::probe_liveness;

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
