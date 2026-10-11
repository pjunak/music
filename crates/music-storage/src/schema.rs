use std::collections::BTreeMap;
use std::fmt::{self, Display, Formatter};
use std::path::Path;
use std::time::Duration;

use serde::Serialize;
use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};
use sqlx::{Row, SqlitePool};

use crate::StorageError;
use crate::migration::{MIGRATIONS, migrator};

/// The newest embedded migration. The SQLx ledger is the schema authority.
pub const CURRENT_SCHEMA_VERSION: i64 = MIGRATIONS[MIGRATIONS.len() - 1].0;

const INSPECTION_BUSY_TIMEOUT: Duration = Duration::from_secs(5);
const SQLX_MIGRATION_TABLE: &str = "_sqlx_migrations";

#[derive(Debug, Clone, Copy, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SchemaCompatibility {
    /// No application tables yet; migrations create the schema.
    Empty,
    /// A valid ledger with embedded migrations still to apply.
    Outdated,
    Current,
    Incompatible,
}

impl Display for SchemaCompatibility {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Empty => "empty",
            Self::Outdated => "outdated",
            Self::Current => "current",
            Self::Incompatible => "incompatible",
        })
    }
}

#[derive(Debug, Clone, Eq, PartialEq, Serialize)]
/// A problem that makes the database unusable by this server.
pub struct SchemaIssue {
    pub code: &'static str,
    pub detail: String,
}

#[derive(Debug, Clone, Eq, PartialEq, Serialize)]
pub struct SchemaReport {
    pub database_exists: bool,
    pub compatibility: SchemaCompatibility,
    pub sqlite_version: Option<String>,
    pub migration_version: Option<i64>,
    pub current_schema_version: i64,
    /// Embedded migrations not yet recorded in the ledger, oldest first.
    pub pending_migrations: Vec<i64>,
    pub table_count: usize,
    pub integrity_ok: bool,
    pub foreign_key_violations: usize,
    pub migration_required: bool,
    pub issues: Vec<SchemaIssue>,
}

impl SchemaReport {
    #[must_use]
    pub fn is_compatible(&self) -> bool {
        self.compatibility != SchemaCompatibility::Incompatible
    }
}

struct AppliedMigration {
    success: bool,
    checksum: Vec<u8>,
}

pub async fn inspect_database(path: &Path) -> Result<SchemaReport, StorageError> {
    match path.try_exists() {
        Ok(false) => return Ok(empty_report(false)),
        Ok(true) => {}
        Err(source) => {
            return Err(StorageError::Io {
                operation: "inspect database path",
                path: path.to_path_buf(),
                source,
            });
        }
    }

    let options = SqliteConnectOptions::new()
        .filename(path)
        .read_only(true)
        .foreign_keys(true)
        .busy_timeout(INSPECTION_BUSY_TIMEOUT);
    let pool = SqlitePoolOptions::new()
        .max_connections(1)
        .connect_with(options)
        .await?;
    let report = inspect_pool(&pool, true).await;
    pool.close().await;
    report
}

/// Compare the database's SQLx ledger with the embedded migrations and run
/// SQLite's own integrity checks. Applied migrations are trusted to have
/// produced their schema; their checksums detect edited migration files.
pub(crate) async fn inspect_pool(
    pool: &SqlitePool,
    database_exists: bool,
) -> Result<SchemaReport, StorageError> {
    let sqlite_version = sqlx::query_scalar::<_, String>("SELECT sqlite_version()")
        .fetch_one(pool)
        .await?;
    let tables = sqlx::query_scalar::<_, String>(
        "SELECT name FROM sqlite_schema WHERE type = 'table' AND name NOT LIKE 'sqlite_%'",
    )
    .fetch_all(pool)
    .await?;
    let has_ledger = tables.iter().any(|table| table == SQLX_MIGRATION_TABLE);
    let table_count = tables
        .iter()
        .filter(|table| *table != SQLX_MIGRATION_TABLE)
        .count();
    let mut issues = Vec::new();

    let integrity_rows = sqlx::query_scalar::<_, String>("PRAGMA quick_check")
        .fetch_all(pool)
        .await?;
    let integrity_ok = integrity_rows.len() == 1 && integrity_rows[0] == "ok";
    if !integrity_ok {
        issue(
            &mut issues,
            "integrity_check_failed",
            "SQLite quick_check did not return ok",
        );
    }
    let foreign_key_violations = sqlx::query("PRAGMA foreign_key_check")
        .fetch_all(pool)
        .await?
        .len();
    if foreign_key_violations != 0 {
        issue(
            &mut issues,
            "foreign_key_violation",
            format!("found {foreign_key_violations} foreign-key violation(s)"),
        );
    }

    let applied = if has_ledger {
        applied_migrations(pool).await?
    } else {
        BTreeMap::new()
    };
    if !has_ledger && table_count != 0 {
        issue(
            &mut issues,
            "unversioned_database",
            "the database has no migration ledger; databases from before the Rust server are not supported",
        );
    }

    let known = migrator();
    for (version, record) in &applied {
        if !record.success {
            issue(
                &mut issues,
                "failed_migration",
                format!("migration {version} is recorded as failed"),
            );
        }
        match known.iter().find(|migration| migration.version == *version) {
            None if *version > CURRENT_SCHEMA_VERSION => issue(
                &mut issues,
                "future_migration_version",
                format!("migration {version} comes from a newer server version"),
            ),
            None => issue(
                &mut issues,
                "unknown_migration",
                format!("migration {version} is not part of this server"),
            ),
            Some(migration) if migration.checksum.as_ref() != record.checksum.as_slice() => {
                issue(
                    &mut issues,
                    "migration_checksum_mismatch",
                    format!("migration {version} differs from the one that was applied"),
                );
            }
            Some(_) => {}
        }
    }
    let pending_migrations = known
        .iter()
        .map(|migration| migration.version)
        .filter(|version| !applied.contains_key(version))
        .collect::<Vec<_>>();
    let migration_version = applied
        .iter()
        .filter(|(_, record)| record.success)
        .map(|(version, _)| *version)
        .max();

    let compatibility = if !issues.is_empty() {
        SchemaCompatibility::Incompatible
    } else if table_count == 0 {
        SchemaCompatibility::Empty
    } else if pending_migrations.is_empty() {
        SchemaCompatibility::Current
    } else {
        SchemaCompatibility::Outdated
    };

    Ok(SchemaReport {
        database_exists,
        compatibility,
        sqlite_version: Some(sqlite_version),
        migration_version,
        current_schema_version: CURRENT_SCHEMA_VERSION,
        pending_migrations,
        table_count,
        integrity_ok,
        foreign_key_violations,
        migration_required: compatibility != SchemaCompatibility::Current,
        issues,
    })
}

async fn applied_migrations(
    pool: &SqlitePool,
) -> Result<BTreeMap<i64, AppliedMigration>, StorageError> {
    let rows = sqlx::query("SELECT version, success, checksum FROM _sqlx_migrations")
        .fetch_all(pool)
        .await?;
    rows.iter()
        .map(|row| {
            Ok((
                row.try_get::<i64, _>("version")?,
                AppliedMigration {
                    success: row.try_get("success")?,
                    checksum: row.try_get("checksum")?,
                },
            ))
        })
        .collect()
}

fn empty_report(database_exists: bool) -> SchemaReport {
    SchemaReport {
        database_exists,
        compatibility: SchemaCompatibility::Empty,
        sqlite_version: None,
        migration_version: None,
        current_schema_version: CURRENT_SCHEMA_VERSION,
        pending_migrations: MIGRATIONS.iter().map(|(version, _, _)| *version).collect(),
        table_count: 0,
        integrity_ok: true,
        foreign_key_violations: 0,
        migration_required: true,
        issues: Vec::new(),
    }
}

fn issue(issues: &mut Vec<SchemaIssue>, code: &'static str, detail: impl Into<String>) {
    issues.push(SchemaIssue {
        code,
        detail: detail.into(),
    });
}

#[cfg(test)]
mod tests {
    use std::error::Error;
    use std::path::Path;

    use sqlx::SqlitePool;
    use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};
    use tempfile::tempdir;

    use super::{CURRENT_SCHEMA_VERSION, SchemaCompatibility, inspect_database};
    use crate::migration::migrator;

    async fn writable_pool(path: &Path) -> Result<SqlitePool, sqlx::Error> {
        SqlitePoolOptions::new()
            .max_connections(1)
            .connect_with(
                SqliteConnectOptions::new()
                    .filename(path)
                    .create_if_missing(true),
            )
            .await
    }

    #[tokio::test]
    async fn missing_database_is_empty_without_creating_it() -> Result<(), Box<dyn Error>> {
        let directory = tempdir()?;
        let path = directory.path().join("app.db");
        let report = inspect_database(&path).await?;
        assert!(!report.database_exists);
        assert_eq!(report.compatibility, SchemaCompatibility::Empty);
        assert!(!path.exists());
        Ok(())
    }

    #[tokio::test]
    async fn ledger_decides_current_and_outdated() -> Result<(), Box<dyn Error>> {
        let directory = tempdir()?;
        let path = directory.path().join("app.db");
        let pool = writable_pool(&path).await?;
        sqlx::migrate::Migrator::with_migrations(
            migrator()
                .iter()
                .filter(|migration| migration.version < CURRENT_SCHEMA_VERSION)
                .cloned()
                .collect(),
        )
        .run(&pool)
        .await?;
        let outdated = inspect_database(&path).await?;
        assert_eq!(outdated.compatibility, SchemaCompatibility::Outdated);
        assert_eq!(outdated.pending_migrations, vec![CURRENT_SCHEMA_VERSION]);

        migrator().run(&pool).await?;
        pool.close().await;
        let current = inspect_database(&path).await?;
        assert_eq!(
            current.compatibility,
            SchemaCompatibility::Current,
            "{current:?}"
        );
        assert_eq!(current.migration_version, Some(CURRENT_SCHEMA_VERSION));
        assert!(current.issues.is_empty());
        Ok(())
    }

    #[tokio::test]
    async fn rejects_unversioned_edited_and_future_databases() -> Result<(), Box<dyn Error>> {
        let cases: [(&str, &str); 3] = [
            (
                "CREATE TABLE tracks (id INTEGER PRIMARY KEY)",
                "unversioned_database",
            ),
            (
                "UPDATE _sqlx_migrations SET checksum = X'00' WHERE version = 1",
                "migration_checksum_mismatch",
            ),
            (
                "INSERT INTO _sqlx_migrations (version, description, success, checksum, execution_time) \
                 VALUES (9999, 'newer server', 1, X'00', 0)",
                "future_migration_version",
            ),
        ];
        for (index, (statement, code)) in cases.into_iter().enumerate() {
            let directory = tempdir()?;
            let path = directory.path().join("app.db");
            let pool = writable_pool(&path).await?;
            if index != 0 {
                migrator().run(&pool).await?;
            }
            sqlx::raw_sql(sqlx::AssertSqlSafe(statement))
                .execute(&pool)
                .await?;
            pool.close().await;
            let report = inspect_database(&path).await?;
            assert_eq!(report.compatibility, SchemaCompatibility::Incompatible);
            assert!(
                report.issues.iter().any(|issue| issue.code == code),
                "{report:?}"
            );
        }
        Ok(())
    }
}
