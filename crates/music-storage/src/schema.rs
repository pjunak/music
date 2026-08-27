use std::collections::{BTreeMap, BTreeSet};
use std::fmt::{self, Display, Formatter};
use std::path::Path;
use std::time::Duration;

use serde::Serialize;
use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};
use sqlx::{Row, SqlitePool};

use crate::StorageError;

pub const CURRENT_SCHEMA_VERSION: i64 = 2;

const BASELINE_SCHEMA_SQL: &str = include_str!("../migrations/0001_rust_baseline.sql");
const LIBRARY_STATE_SCHEMA_SQL: &str = include_str!("../migrations/0002_library_state.sql");
const INSPECTION_BUSY_TIMEOUT: Duration = Duration::from_secs(5);
const SQLX_MIGRATION_TABLE: &str = "_sqlx_migrations";

const ADDITIVE_COLUMNS: &[(&str, &str)] = &[
    ("tracks", "display_title"),
    ("tracks", "origin"),
    ("track_analyses", "metrics_json"),
    ("assistant_model_roles", "conformance_status"),
    ("assistant_model_roles", "conformance_error_code"),
    ("assistant_model_roles", "conformance_fingerprint"),
    ("assistant_model_roles", "last_conformance_at"),
    ("assistant_model_roles", "thinking_mode"),
    (
        "assistant_provider_connections",
        "verified_capabilities_json",
    ),
    ("playlists", "automatic_rule_json"),
    ("playlists", "automatic_source_signature"),
    ("playlists", "automatic_refreshed_at"),
    ("assistant_tag_vocabularies", "seed_version"),
    ("playback_state", "storage_revision"),
];

#[derive(Debug, Clone, Copy, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SchemaCompatibility {
    Empty,
    CompatibleLegacy,
    Current,
    Incompatible,
}

impl Display for SchemaCompatibility {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Empty => "empty",
            Self::CompatibleLegacy => "compatible_legacy",
            Self::Current => "current",
            Self::Incompatible => "incompatible",
        })
    }
}

#[derive(Debug, Clone, Copy, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SchemaIssueLevel {
    Warning,
    Error,
}

impl Display for SchemaIssueLevel {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Warning => "warning",
            Self::Error => "error",
        })
    }
}

#[derive(Debug, Clone, Eq, PartialEq, Serialize)]
pub struct SchemaIssue {
    pub level: SchemaIssueLevel,
    pub code: String,
    pub detail: String,
}

#[derive(Debug, Clone, Eq, PartialEq, Serialize)]
pub struct SchemaReport {
    pub database_exists: bool,
    pub compatibility: SchemaCompatibility,
    pub sqlite_version: Option<String>,
    pub migration_version: Option<i64>,
    pub current_schema_version: i64,
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

    pub fn errors(&self) -> impl Iterator<Item = &SchemaIssue> {
        self.issues
            .iter()
            .filter(|issue| issue.level == SchemaIssueLevel::Error)
    }
}

#[derive(Debug, Clone, Eq, PartialEq, Ord, PartialOrd)]
struct ColumnShape {
    data_type: String,
    not_null: bool,
    primary_key_position: i64,
}

#[derive(Debug, Clone, Eq, PartialEq, Ord, PartialOrd)]
struct ForeignKeyShape {
    source_column: String,
    target_table: String,
    target_column: String,
    on_update: String,
    on_delete: String,
    match_mode: String,
}

#[derive(Debug, Clone, Eq, PartialEq)]
struct TableShape {
    columns: BTreeMap<String, ColumnShape>,
    foreign_keys: BTreeSet<ForeignKeyShape>,
    unique_constraints: BTreeSet<Vec<String>>,
    normalized_create_sql: String,
}

#[derive(Debug, Clone, Eq, PartialEq)]
struct IndexShape {
    table: String,
    unique: bool,
    columns: Vec<String>,
}

#[derive(Debug, Clone, Eq, PartialEq)]
struct DatabaseShape {
    tables: BTreeMap<String, TableShape>,
    indexes: BTreeMap<String, IndexShape>,
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

pub(crate) async fn inspect_pool(
    pool: &SqlitePool,
    database_exists: bool,
) -> Result<SchemaReport, StorageError> {
    let expected = expected_shape().await?;
    let actual = read_shape(pool).await?;
    let sqlite_version = sqlx::query_scalar::<_, String>("SELECT sqlite_version()")
        .fetch_one(pool)
        .await?;
    let mut issues = Vec::new();
    let mut requires_migration = false;

    let integrity_rows = sqlx::query_scalar::<_, String>("PRAGMA quick_check")
        .fetch_all(pool)
        .await?;
    let integrity_ok = integrity_rows.len() == 1 && integrity_rows[0] == "ok";
    if !integrity_ok {
        issue(
            &mut issues,
            SchemaIssueLevel::Error,
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
            SchemaIssueLevel::Error,
            "foreign_key_violation",
            format!("found {foreign_key_violations} foreign-key violation(s)"),
        );
    }

    for table in actual.tables.keys() {
        if table != SQLX_MIGRATION_TABLE && !expected.tables.contains_key(table) {
            issue(
                &mut issues,
                SchemaIssueLevel::Error,
                "unknown_table",
                format!("unexpected table {table}"),
            );
        }
    }

    for (table_name, expected_table) in &expected.tables {
        let Some(actual_table) = actual.tables.get(table_name) else {
            requires_migration = true;
            issue(
                &mut issues,
                SchemaIssueLevel::Warning,
                "missing_table",
                format!("migration will create table {table_name}"),
            );
            continue;
        };

        for column in actual_table.columns.keys() {
            if !expected_table.columns.contains_key(column) {
                issue(
                    &mut issues,
                    SchemaIssueLevel::Error,
                    "unknown_column",
                    format!("unexpected column {table_name}.{column}"),
                );
            }
        }

        for (column_name, expected_column) in &expected_table.columns {
            let Some(actual_column) = actual_table.columns.get(column_name) else {
                if is_additive_column(table_name, column_name) {
                    requires_migration = true;
                    issue(
                        &mut issues,
                        SchemaIssueLevel::Warning,
                        "missing_additive_column",
                        format!("migration will add {table_name}.{column_name}"),
                    );
                } else {
                    issue(
                        &mut issues,
                        SchemaIssueLevel::Error,
                        "missing_required_column",
                        format!("required column {table_name}.{column_name} is absent"),
                    );
                }
                continue;
            };
            if actual_column != expected_column {
                issue(
                    &mut issues,
                    SchemaIssueLevel::Error,
                    "column_shape_mismatch",
                    format!("column {table_name}.{column_name} has an incompatible shape"),
                );
            }
        }

        if actual_table.foreign_keys != expected_table.foreign_keys {
            issue(
                &mut issues,
                SchemaIssueLevel::Error,
                "foreign_key_shape_mismatch",
                format!("table {table_name} has incompatible foreign keys"),
            );
        }
        if actual_table.unique_constraints != expected_table.unique_constraints {
            issue(
                &mut issues,
                SchemaIssueLevel::Error,
                "unique_constraint_mismatch",
                format!("table {table_name} has incompatible unique constraints"),
            );
        }
        for required_check in required_check_fragments(table_name) {
            if !actual_table.normalized_create_sql.contains(required_check) {
                issue(
                    &mut issues,
                    SchemaIssueLevel::Error,
                    "check_constraint_mismatch",
                    format!("table {table_name} is missing a required check constraint"),
                );
            }
        }
    }

    for (index_name, expected_index) in &expected.indexes {
        match actual.indexes.get(index_name) {
            Some(actual_index) if actual_index == expected_index => {}
            Some(_) => issue(
                &mut issues,
                SchemaIssueLevel::Error,
                "index_shape_mismatch",
                format!("index {index_name} has an incompatible shape"),
            ),
            None => {
                requires_migration = true;
                issue(
                    &mut issues,
                    SchemaIssueLevel::Warning,
                    "missing_index",
                    format!("migration will create index {index_name}"),
                );
            }
        }
    }
    for index_name in actual.indexes.keys() {
        if !expected.indexes.contains_key(index_name) {
            issue(
                &mut issues,
                SchemaIssueLevel::Warning,
                "additional_index",
                format!("additional compatible index {index_name} will be preserved"),
            );
        }
    }

    let migration_version = migration_version(pool, &actual, &mut issues).await;
    if migration_version.is_none() {
        requires_migration = true;
    }
    if migration_version.is_some_and(|version| version > CURRENT_SCHEMA_VERSION) {
        issue(
            &mut issues,
            SchemaIssueLevel::Error,
            "future_migration_version",
            "database was migrated by a newer Rust schema version",
        );
    }

    let table_count = actual
        .tables
        .keys()
        .filter(|table| table.as_str() != SQLX_MIGRATION_TABLE)
        .count();
    let has_errors = issues
        .iter()
        .any(|issue| issue.level == SchemaIssueLevel::Error);
    if migration_version == Some(CURRENT_SCHEMA_VERSION) && requires_migration && !has_errors {
        issue(
            &mut issues,
            SchemaIssueLevel::Error,
            "migration_schema_drift",
            "migration ledger is current but required schema objects are missing",
        );
    }

    let has_errors = issues
        .iter()
        .any(|issue| issue.level == SchemaIssueLevel::Error);
    let compatibility = if has_errors {
        SchemaCompatibility::Incompatible
    } else if table_count == 0 {
        SchemaCompatibility::Empty
    } else if migration_version == Some(CURRENT_SCHEMA_VERSION) && !requires_migration {
        SchemaCompatibility::Current
    } else {
        SchemaCompatibility::CompatibleLegacy
    };

    Ok(SchemaReport {
        database_exists,
        compatibility,
        sqlite_version: Some(sqlite_version),
        migration_version,
        current_schema_version: CURRENT_SCHEMA_VERSION,
        table_count,
        integrity_ok,
        foreign_key_violations,
        migration_required: compatibility != SchemaCompatibility::Current,
        issues,
    })
}

fn empty_report(database_exists: bool) -> SchemaReport {
    SchemaReport {
        database_exists,
        compatibility: SchemaCompatibility::Empty,
        sqlite_version: None,
        migration_version: None,
        current_schema_version: CURRENT_SCHEMA_VERSION,
        table_count: 0,
        integrity_ok: true,
        foreign_key_violations: 0,
        migration_required: true,
        issues: Vec::new(),
    }
}

async fn expected_shape() -> Result<DatabaseShape, StorageError> {
    let options = SqliteConnectOptions::new()
        .in_memory(true)
        .foreign_keys(true);
    let pool = SqlitePoolOptions::new()
        .max_connections(1)
        .connect_with(options)
        .await?;
    sqlx::raw_sql(BASELINE_SCHEMA_SQL).execute(&pool).await?;
    sqlx::raw_sql(LIBRARY_STATE_SCHEMA_SQL)
        .execute(&pool)
        .await?;
    let shape = read_shape(&pool).await;
    pool.close().await;
    shape
}

async fn read_shape(pool: &SqlitePool) -> Result<DatabaseShape, StorageError> {
    let table_rows = sqlx::query(
        "SELECT name, sql FROM sqlite_schema \
         WHERE type = 'table' AND name NOT LIKE 'sqlite_%' ORDER BY name",
    )
    .fetch_all(pool)
    .await?;
    let mut tables = BTreeMap::new();
    let mut indexes = BTreeMap::new();

    for table_row in table_rows {
        let table_name: String = table_row.try_get("name")?;
        let create_sql = table_row
            .try_get::<Option<String>, _>("sql")?
            .unwrap_or_default();
        let column_rows = sqlx::query(
            "SELECT name, type, \"notnull\" AS not_null, pk \
             FROM pragma_table_info(?) ORDER BY cid",
        )
        .bind(&table_name)
        .fetch_all(pool)
        .await?;
        let mut columns = BTreeMap::new();
        for row in column_rows {
            let column_name: String = row.try_get("name")?;
            let data_type: String = row.try_get("type")?;
            columns.insert(
                column_name,
                ColumnShape {
                    data_type: normalize_type(&data_type),
                    not_null: row.try_get::<i64, _>("not_null")? != 0,
                    primary_key_position: row.try_get("pk")?,
                },
            );
        }

        let foreign_key_rows = sqlx::query(
            "SELECT \"from\" AS source_column, \"table\" AS target_table, \
                    \"to\" AS target_column, on_update, on_delete, \
                    \"match\" AS match_mode \
             FROM pragma_foreign_key_list(?)",
        )
        .bind(&table_name)
        .fetch_all(pool)
        .await?;
        let mut foreign_keys = BTreeSet::new();
        for row in foreign_key_rows {
            foreign_keys.insert(ForeignKeyShape {
                source_column: row.try_get("source_column")?,
                target_table: row.try_get("target_table")?,
                target_column: row.try_get("target_column")?,
                on_update: row.try_get("on_update")?,
                on_delete: row.try_get("on_delete")?,
                match_mode: row.try_get("match_mode")?,
            });
        }

        let index_rows = sqlx::query(
            "SELECT name, \"unique\" AS is_unique, origin \
             FROM pragma_index_list(?) ORDER BY name",
        )
        .bind(&table_name)
        .fetch_all(pool)
        .await?;
        let mut unique_constraints = BTreeSet::new();
        for row in index_rows {
            let index_name: String = row.try_get("name")?;
            let index_columns = sqlx::query_scalar::<_, Option<String>>(
                "SELECT name FROM pragma_index_info(?) ORDER BY seqno",
            )
            .bind(&index_name)
            .fetch_all(pool)
            .await?
            .into_iter()
            .map(|column| column.unwrap_or_else(|| "<expression>".to_owned()))
            .collect::<Vec<_>>();
            let origin: String = row.try_get("origin")?;
            if origin == "u" {
                unique_constraints.insert(index_columns);
            } else if origin == "c" {
                indexes.insert(
                    index_name,
                    IndexShape {
                        table: table_name.clone(),
                        unique: row.try_get::<i64, _>("is_unique")? != 0,
                        columns: index_columns,
                    },
                );
            }
        }

        tables.insert(
            table_name,
            TableShape {
                columns,
                foreign_keys,
                unique_constraints,
                normalized_create_sql: normalize_sql(&create_sql),
            },
        );
    }
    Ok(DatabaseShape { tables, indexes })
}

async fn migration_version(
    pool: &SqlitePool,
    actual: &DatabaseShape,
    issues: &mut Vec<SchemaIssue>,
) -> Option<i64> {
    if !actual.tables.contains_key(SQLX_MIGRATION_TABLE) {
        return None;
    }
    let failed =
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM _sqlx_migrations WHERE success = 0")
            .fetch_one(pool)
            .await;
    match failed {
        Ok(0) => {}
        Ok(count) => issue(
            issues,
            SchemaIssueLevel::Error,
            "failed_migration",
            format!("migration ledger contains {count} failed migration(s)"),
        ),
        Err(_) => {
            issue(
                issues,
                SchemaIssueLevel::Error,
                "invalid_migration_ledger",
                "SQLx migration ledger cannot be read",
            );
            return None;
        }
    }
    match sqlx::query_scalar::<_, Option<i64>>(
        "SELECT MAX(version) FROM _sqlx_migrations WHERE success = 1",
    )
    .fetch_one(pool)
    .await
    {
        Ok(version) => version,
        Err(_) => {
            issue(
                issues,
                SchemaIssueLevel::Error,
                "invalid_migration_ledger",
                "SQLx migration version cannot be read",
            );
            None
        }
    }
}

fn normalize_type(data_type: &str) -> String {
    data_type
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_ascii_uppercase()
}

fn normalize_sql(sql: &str) -> String {
    sql.chars()
        .filter(|character| !character.is_ascii_whitespace() && *character != '"')
        .flat_map(char::to_lowercase)
        .collect()
}

fn required_check_fragments(table: &str) -> &'static [&'static str] {
    match table {
        "track_analysis_tag_reviews" => &["check(decisionin('accepted','rejected'))"],
        "remembered_devices" => &["check(is_outputin(0,1))"],
        "legacy_device_imports" => {
            &["check(source_statusin('imported','missing','corrupt','unsupported'))"]
        }
        "recovery_journal" => &[concat!(
            "check(statein('planned','applying','committed','rolling_back',",
            "'rolled_back','failed'))"
        )],
        "library_state" => &[
            "check(id=1)",
            "check(generation>=0)",
            "check(statusin('pending','reconciling','current','failed'))",
            "check(discovered_tracks>=0)",
        ],
        _ => &[],
    }
}

fn is_additive_column(table: &str, column: &str) -> bool {
    ADDITIVE_COLUMNS.contains(&(table, column))
}

fn issue(
    issues: &mut Vec<SchemaIssue>,
    level: SchemaIssueLevel,
    code: impl Into<String>,
    detail: impl Into<String>,
) {
    issues.push(SchemaIssue {
        level,
        code: code.into(),
        detail: detail.into(),
    });
}

#[cfg(test)]
mod tests {
    use std::error::Error;

    use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};
    use tempfile::tempdir;

    use super::{SchemaCompatibility, inspect_database, inspect_pool};

    const PYTHON_SQLITE_FIXTURE: &str =
        include_str!("../../../contracts/reference/v1/sqlite-fixture.sql");

    #[tokio::test]
    async fn missing_database_is_empty_without_creating_it() -> Result<(), Box<dyn Error>> {
        let directory = tempdir()?;
        let path = directory.path().join("missing.db");

        let report = inspect_database(&path).await?;

        assert_eq!(report.compatibility, SchemaCompatibility::Empty);
        assert!(report.migration_required);
        assert!(!report.database_exists);
        assert!(!path.exists());
        Ok(())
    }

    #[tokio::test]
    async fn frozen_python_schema_is_a_compatible_legacy_shape() -> Result<(), Box<dyn Error>> {
        let options = SqliteConnectOptions::new()
            .in_memory(true)
            .foreign_keys(true);
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect_with(options)
            .await?;
        sqlx::raw_sql(PYTHON_SQLITE_FIXTURE).execute(&pool).await?;

        let report = inspect_pool(&pool, true).await?;

        assert_eq!(report.compatibility, SchemaCompatibility::CompatibleLegacy);
        assert!(report.migration_required);
        assert!(report.errors().next().is_none());
        assert!(report.issues.iter().any(|issue| {
            issue.code == "missing_additive_column"
                && issue.detail.contains("playback_state.storage_revision")
        }));
        Ok(())
    }

    #[tokio::test]
    async fn rejects_unknown_tables_and_damaged_known_tables() -> Result<(), Box<dyn Error>> {
        let options = SqliteConnectOptions::new().in_memory(true);
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect_with(options)
            .await?;
        sqlx::raw_sql(
            "CREATE TABLE users (id INTEGER PRIMARY KEY); \
             CREATE TABLE surprise (secret TEXT);",
        )
        .execute(&pool)
        .await?;

        let report = inspect_pool(&pool, true).await?;

        assert_eq!(report.compatibility, SchemaCompatibility::Incompatible);
        assert!(report.errors().any(|issue| issue.code == "unknown_table"));
        assert!(
            report
                .errors()
                .any(|issue| issue.code == "missing_required_column")
        );
        Ok(())
    }
}
