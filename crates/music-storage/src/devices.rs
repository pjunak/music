use std::collections::BTreeSet;
use std::io::Write;
use std::path::{Path, PathBuf};

use music_application::devices::{
    DeviceDependencyError, DeviceFuture, RememberedDevice, RememberedDeviceRepository,
};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use sha2::{Digest, Sha256};
use sqlx::Row;

use crate::{SqliteStorage, StorageError};

const MAX_LEGACY_DEVICE_FILE_BYTES: u64 = 4 * 1024 * 1024;
const MAX_TRANSFER_DEVICE_COUNT: usize = 4_096;
const DEVICE_TRANSFER_SCHEMA: &str = "remembered-devices/v1";

#[derive(Debug, Clone, Eq, PartialEq, Serialize)]
pub struct DeviceExportOutcome {
    pub schema_version: &'static str,
    pub exported_count: u64,
    pub path: PathBuf,
}

#[derive(Debug, Clone, Eq, PartialEq, Serialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum DeviceImportOutcome {
    Imported {
        schema_version: &'static str,
        imported_count: u64,
        replaced_count: u64,
    },
    TargetNotEmpty {
        existing_count: u64,
    },
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct DeviceTransferDocument {
    schema_version: String,
    devices: Vec<DeviceTransferRecord>,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct DeviceTransferRecord {
    client_id: String,
    name: String,
    is_output: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    added_at: Option<String>,
}

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum LegacyDeviceImportStatus {
    Imported,
    Missing,
    Corrupt,
    Unsupported,
}

impl LegacyDeviceImportStatus {
    const fn as_str(self) -> &'static str {
        match self {
            Self::Imported => "imported",
            Self::Missing => "missing",
            Self::Corrupt => "corrupt",
            Self::Unsupported => "unsupported",
        }
    }

    fn parse(value: &str) -> Option<Self> {
        match value {
            "imported" => Some(Self::Imported),
            "missing" => Some(Self::Missing),
            "corrupt" => Some(Self::Corrupt),
            "unsupported" => Some(Self::Unsupported),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Eq, PartialEq)]
pub struct LegacyDeviceImportRecord {
    pub source_fingerprint: String,
    pub source_file_name: String,
    pub status: LegacyDeviceImportStatus,
    pub imported_count: u64,
}

#[derive(Debug, Clone, Eq, PartialEq)]
pub enum LegacyDeviceImportOutcome {
    Applied(LegacyDeviceImportRecord),
    AlreadyRecorded(LegacyDeviceImportRecord),
    TargetNotEmpty,
}

#[derive(Debug)]
struct LegacyImportCandidate {
    record: LegacyDeviceImportRecord,
    devices: Vec<RememberedDevice>,
}

impl SqliteStorage {
    pub async fn import_legacy_devices_once(
        &self,
        path: &Path,
    ) -> Result<LegacyDeviceImportOutcome, StorageError> {
        if let Some(record) = self.legacy_device_import_record().await? {
            return Ok(LegacyDeviceImportOutcome::AlreadyRecorded(record));
        }
        let target_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM remembered_devices")
            .fetch_one(&self.pool)
            .await?;
        if target_count != 0 {
            return Ok(LegacyDeviceImportOutcome::TargetNotEmpty);
        }

        let path = path.to_path_buf();
        let candidate = tokio::task::spawn_blocking(move || legacy_candidate(&path)).await??;
        let _admission = self.write_gate.lock().await;
        let mut transaction = self.pool.begin().await?;
        let recorded_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM legacy_device_imports")
            .fetch_one(&mut *transaction)
            .await?;
        if recorded_count != 0 {
            transaction.rollback().await?;
            let record = self
                .legacy_device_import_record()
                .await?
                .ok_or(StorageError::InvalidLegacyDeviceImport)?;
            return Ok(LegacyDeviceImportOutcome::AlreadyRecorded(record));
        }
        let target_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM remembered_devices")
            .fetch_one(&mut *transaction)
            .await?;
        if target_count != 0 {
            transaction.rollback().await?;
            return Ok(LegacyDeviceImportOutcome::TargetNotEmpty);
        }

        for device in &candidate.devices {
            sqlx::query(
                "INSERT INTO remembered_devices \
                 (client_id, name, is_output, added_at, updated_at) \
                 VALUES (?, ?, ?, ?, CURRENT_TIMESTAMP)",
            )
            .bind(&device.client_id)
            .bind(&device.name)
            .bind(device.is_output)
            .bind(device.added_at.as_deref())
            .execute(&mut *transaction)
            .await?;
        }
        sqlx::query(
            "INSERT INTO legacy_device_imports \
             (source_fingerprint, source_file_name, source_status, imported_count, imported_at) \
             VALUES (?, ?, ?, ?, CURRENT_TIMESTAMP)",
        )
        .bind(&candidate.record.source_fingerprint)
        .bind(&candidate.record.source_file_name)
        .bind(candidate.record.status.as_str())
        .bind(i64::try_from(candidate.record.imported_count).unwrap_or(i64::MAX))
        .execute(&mut *transaction)
        .await?;
        transaction.commit().await?;
        Ok(LegacyDeviceImportOutcome::Applied(candidate.record))
    }

    /// Write a stable, versioned remembered-device recovery document without
    /// overwriting an existing target. The temporary file lives beside the
    /// destination so the final no-clobber publish stays on one filesystem.
    pub async fn export_remembered_devices(
        &self,
        path: &Path,
    ) -> Result<DeviceExportOutcome, StorageError> {
        let rows = sqlx::query(
            "SELECT client_id, name, is_output, added_at FROM remembered_devices \
             ORDER BY client_id",
        )
        .fetch_all(&self.pool)
        .await?;
        let devices = rows
            .iter()
            .map(stored_device_from_row)
            .collect::<Result<Vec<_>, _>>()?;
        let exported_count = u64::try_from(devices.len()).unwrap_or(u64::MAX);
        let document = DeviceTransferDocument {
            schema_version: DEVICE_TRANSFER_SCHEMA.to_owned(),
            devices: devices
                .into_iter()
                .map(|device| DeviceTransferRecord {
                    client_id: device.client_id,
                    name: device.name,
                    is_output: device.is_output,
                    added_at: device.added_at,
                })
                .collect(),
        };
        let mut bytes = serde_json::to_vec_pretty(&document)
            .map_err(StorageError::DeviceTransferSerialization)?;
        bytes.push(b'\n');
        let path = path.to_path_buf();
        let write_path = path.clone();
        tokio::task::spawn_blocking(move || write_transfer_file(&write_path, &bytes)).await??;
        Ok(DeviceExportOutcome {
            schema_version: DEVICE_TRANSFER_SCHEMA,
            exported_count,
            path,
        })
    }

    /// Import a transfer document transactionally. A populated target is
    /// refused unless the operator explicitly selected replacement.
    pub async fn import_remembered_devices(
        &self,
        path: &Path,
        replace: bool,
    ) -> Result<DeviceImportOutcome, StorageError> {
        let path = path.to_path_buf();
        let devices = tokio::task::spawn_blocking(move || read_transfer_file(&path)).await??;
        let _admission = self.write_gate.lock().await;
        let mut transaction = self.pool.begin().await?;
        let existing_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM remembered_devices")
            .fetch_one(&mut *transaction)
            .await?;
        let existing_count = u64::try_from(existing_count)
            .map_err(|_| StorageError::InvalidDeviceTransfer("stored device count is invalid"))?;
        if existing_count != 0 && !replace {
            transaction.rollback().await?;
            return Ok(DeviceImportOutcome::TargetNotEmpty { existing_count });
        }
        if replace {
            sqlx::query("DELETE FROM remembered_devices")
                .execute(&mut *transaction)
                .await?;
        }
        for device in &devices {
            sqlx::query(
                "INSERT INTO remembered_devices \
                 (client_id, name, is_output, added_at, updated_at) \
                 VALUES (?, ?, ?, COALESCE(?, strftime('%Y-%m-%dT%H:%M:%SZ', 'now')), \
                         CURRENT_TIMESTAMP)",
            )
            .bind(&device.client_id)
            .bind(&device.name)
            .bind(device.is_output)
            .bind(device.added_at.as_deref())
            .execute(&mut *transaction)
            .await?;
        }
        transaction.commit().await?;
        Ok(DeviceImportOutcome::Imported {
            schema_version: DEVICE_TRANSFER_SCHEMA,
            imported_count: u64::try_from(devices.len()).unwrap_or(u64::MAX),
            replaced_count: if replace { existing_count } else { 0 },
        })
    }

    async fn legacy_device_import_record(
        &self,
    ) -> Result<Option<LegacyDeviceImportRecord>, StorageError> {
        let row = sqlx::query(
            "SELECT source_fingerprint, source_file_name, source_status, imported_count \
             FROM legacy_device_imports ORDER BY imported_at, source_fingerprint LIMIT 1",
        )
        .fetch_optional(&self.pool)
        .await?;
        row.map(|row| {
            let status: String = row.try_get("source_status")?;
            let status = LegacyDeviceImportStatus::parse(&status)
                .ok_or(StorageError::InvalidLegacyDeviceImport)?;
            let imported_count: i64 = row.try_get("imported_count")?;
            Ok(LegacyDeviceImportRecord {
                source_fingerprint: row.try_get("source_fingerprint")?,
                source_file_name: row.try_get("source_file_name")?,
                status,
                imported_count: u64::try_from(imported_count)
                    .map_err(|_| StorageError::InvalidLegacyDeviceImport)?,
            })
        })
        .transpose()
    }
}

impl RememberedDeviceRepository for SqliteStorage {
    fn list_devices(&self) -> DeviceFuture<'_, Vec<RememberedDevice>> {
        Box::pin(async move {
            let rows = sqlx::query(
                "SELECT client_id, name, is_output, added_at FROM remembered_devices \
                 ORDER BY is_output DESC, name COLLATE NOCASE, client_id",
            )
            .fetch_all(&self.pool)
            .await
            .map_err(box_storage)?;
            rows.iter().map(device_from_row).collect()
        })
    }

    fn find_device<'a>(&'a self, client_id: &'a str) -> DeviceFuture<'a, Option<RememberedDevice>> {
        Box::pin(async move {
            sqlx::query(
                "SELECT client_id, name, is_output, added_at FROM remembered_devices \
                 WHERE client_id = ?",
            )
            .bind(client_id)
            .fetch_optional(&self.pool)
            .await
            .map_err(box_storage)?
            .as_ref()
            .map(device_from_row)
            .transpose()
        })
    }

    fn upsert_device<'a>(
        &'a self,
        client_id: &'a str,
        name: &'a str,
        is_output: bool,
    ) -> DeviceFuture<'a, RememberedDevice> {
        Box::pin(async move {
            let _admission = self.write_gate.lock().await;
            let mut transaction = self.pool.begin().await.map_err(box_storage)?;
            sqlx::query(
                "INSERT INTO remembered_devices \
                 (client_id, name, is_output, added_at, updated_at) \
                 VALUES (?, ?, ?, strftime('%Y-%m-%dT%H:%M:%SZ', 'now'), CURRENT_TIMESTAMP) \
                 ON CONFLICT(client_id) DO UPDATE SET \
                    name = excluded.name, is_output = excluded.is_output, \
                    updated_at = CURRENT_TIMESTAMP",
            )
            .bind(client_id)
            .bind(name)
            .bind(is_output)
            .execute(&mut *transaction)
            .await
            .map_err(box_storage)?;
            let row = sqlx::query(
                "SELECT client_id, name, is_output, added_at FROM remembered_devices \
                 WHERE client_id = ?",
            )
            .bind(client_id)
            .fetch_one(&mut *transaction)
            .await
            .map_err(box_storage)?;
            let device = device_from_row(&row)?;
            transaction.commit().await.map_err(box_storage)?;
            Ok(device)
        })
    }

    fn delete_device<'a>(&'a self, client_id: &'a str) -> DeviceFuture<'a, bool> {
        Box::pin(async move {
            let _admission = self.write_gate.lock().await;
            sqlx::query("DELETE FROM remembered_devices WHERE client_id = ?")
                .bind(client_id)
                .execute(&self.pool)
                .await
                .map(|result| result.rows_affected() == 1)
                .map_err(box_storage)
        })
    }
}

fn legacy_candidate(path: &Path) -> Result<LegacyImportCandidate, StorageError> {
    let source_file_name = path
        .file_name()
        .map_or_else(String::new, |name| name.to_string_lossy().into_owned());
    let metadata = match std::fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(source) if source.kind() == std::io::ErrorKind::NotFound => {
            return Ok(status_candidate(
                &source_file_name,
                LegacyDeviceImportStatus::Missing,
                b"missing",
            ));
        }
        Err(source) => {
            return Err(StorageError::Io {
                operation: "inspect legacy device registry",
                path: path.to_path_buf(),
                source,
            });
        }
    };
    if !metadata.file_type().is_file() || metadata.file_type().is_symlink() {
        return Ok(status_candidate(
            &source_file_name,
            LegacyDeviceImportStatus::Unsupported,
            b"unsupported-file-type",
        ));
    }
    if metadata.len() > MAX_LEGACY_DEVICE_FILE_BYTES {
        return Ok(status_candidate(
            &source_file_name,
            LegacyDeviceImportStatus::Unsupported,
            b"file-too-large",
        ));
    }
    let bytes = std::fs::read(path).map_err(|source| StorageError::Io {
        operation: "read legacy device registry",
        path: path.to_path_buf(),
        source,
    })?;
    let source_fingerprint = sha256(&bytes);
    let Ok(value) = serde_json::from_slice::<Value>(&bytes) else {
        return Ok(LegacyImportCandidate {
            record: LegacyDeviceImportRecord {
                source_fingerprint,
                source_file_name,
                status: LegacyDeviceImportStatus::Corrupt,
                imported_count: 0,
            },
            devices: Vec::new(),
        });
    };
    let Value::Object(records) = value else {
        return Ok(LegacyImportCandidate {
            record: LegacyDeviceImportRecord {
                source_fingerprint,
                source_file_name,
                status: LegacyDeviceImportStatus::Unsupported,
                imported_count: 0,
            },
            devices: Vec::new(),
        });
    };
    let devices = records
        .into_iter()
        .filter_map(|(client_id, value)| legacy_device(client_id, value))
        .collect::<Vec<_>>();
    let imported_count = u64::try_from(devices.len()).unwrap_or(u64::MAX);
    Ok(LegacyImportCandidate {
        record: LegacyDeviceImportRecord {
            source_fingerprint,
            source_file_name,
            status: LegacyDeviceImportStatus::Imported,
            imported_count,
        },
        devices,
    })
}

fn write_transfer_file(path: &Path, bytes: &[u8]) -> Result<(), StorageError> {
    if path.file_name().is_none() {
        return Err(StorageError::InvalidDeviceTransfer(
            "export path must name a file",
        ));
    }
    let parent = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    let mut staged =
        tempfile::NamedTempFile::new_in(parent).map_err(|source| StorageError::Io {
            operation: "create staged remembered-device export in",
            path: parent.to_path_buf(),
            source,
        })?;
    staged
        .write_all(bytes)
        .and_then(|()| staged.as_file().sync_all())
        .map_err(|source| StorageError::Io {
            operation: "write staged remembered-device export for",
            path: path.to_path_buf(),
            source,
        })?;
    staged
        .persist_noclobber(path)
        .map_err(|error| StorageError::Io {
            operation: "publish remembered-device export to",
            path: path.to_path_buf(),
            source: error.error,
        })?;
    Ok(())
}

fn read_transfer_file(path: &Path) -> Result<Vec<RememberedDevice>, StorageError> {
    let metadata = std::fs::symlink_metadata(path).map_err(|source| StorageError::Io {
        operation: "inspect remembered-device transfer",
        path: path.to_path_buf(),
        source,
    })?;
    if !metadata.file_type().is_file() || metadata.file_type().is_symlink() {
        return Err(StorageError::InvalidDeviceTransfer(
            "input must be a regular non-symlink file",
        ));
    }
    if metadata.len() > MAX_LEGACY_DEVICE_FILE_BYTES {
        return Err(StorageError::InvalidDeviceTransfer(
            "input exceeds the four MiB limit",
        ));
    }
    let bytes = std::fs::read(path).map_err(|source| StorageError::Io {
        operation: "read remembered-device transfer",
        path: path.to_path_buf(),
        source,
    })?;
    let document = serde_json::from_slice::<DeviceTransferDocument>(&bytes)
        .map_err(|_| StorageError::InvalidDeviceTransfer("document is not valid versioned JSON"))?;
    if document.schema_version != DEVICE_TRANSFER_SCHEMA {
        return Err(StorageError::InvalidDeviceTransfer(
            "schema_version is unsupported",
        ));
    }
    if document.devices.len() > MAX_TRANSFER_DEVICE_COUNT {
        return Err(StorageError::InvalidDeviceTransfer(
            "document contains too many devices",
        ));
    }
    let mut client_ids = BTreeSet::new();
    let mut devices = Vec::with_capacity(document.devices.len());
    for record in document.devices {
        if !(1..=64).contains(&record.client_id.chars().count())
            || record.client_id.chars().any(char::is_control)
        {
            return Err(StorageError::InvalidDeviceTransfer(
                "client_id must contain 1 to 64 printable characters",
            ));
        }
        if !client_ids.insert(record.client_id.clone()) {
            return Err(StorageError::InvalidDeviceTransfer(
                "client_id values must be unique",
            ));
        }
        if !(1..=128).contains(&record.name.chars().count())
            || record.name.chars().any(char::is_control)
        {
            return Err(StorageError::InvalidDeviceTransfer(
                "name must contain 1 to 128 printable characters",
            ));
        }
        if record.added_at.as_ref().is_some_and(|added_at| {
            !(1..=64).contains(&added_at.chars().count()) || added_at.chars().any(char::is_control)
        }) {
            return Err(StorageError::InvalidDeviceTransfer(
                "added_at must be a bounded printable timestamp",
            ));
        }
        devices.push(RememberedDevice {
            client_id: record.client_id,
            name: record.name,
            is_output: record.is_output,
            added_at: record.added_at,
        });
    }
    Ok(devices)
}

fn status_candidate(
    source_file_name: &str,
    status: LegacyDeviceImportStatus,
    marker: &[u8],
) -> LegacyImportCandidate {
    let mut fingerprint_input = source_file_name.as_bytes().to_vec();
    fingerprint_input.push(0);
    fingerprint_input.extend_from_slice(marker);
    LegacyImportCandidate {
        record: LegacyDeviceImportRecord {
            source_fingerprint: sha256(&fingerprint_input),
            source_file_name: source_file_name.to_owned(),
            status,
            imported_count: 0,
        },
        devices: Vec::new(),
    }
}

fn legacy_device(client_id: String, value: Value) -> Option<RememberedDevice> {
    let Value::Object(record) = value else {
        return None;
    };
    Some(RememberedDevice {
        client_id,
        name: string_field(&record, "name").unwrap_or_default(),
        is_output: record.get("is_output").is_some_and(python_truthy),
        added_at: string_field(&record, "added_at"),
    })
}

fn string_field(record: &Map<String, Value>, field: &str) -> Option<String> {
    record.get(field).and_then(Value::as_str).map(str::to_owned)
}

fn python_truthy(value: &Value) -> bool {
    match value {
        Value::Null => false,
        Value::Bool(value) => *value,
        Value::Number(value) => value.as_f64().is_some_and(|value| value != 0.0),
        Value::String(value) => !value.is_empty(),
        Value::Array(value) => !value.is_empty(),
        Value::Object(value) => !value.is_empty(),
    }
}

fn sha256(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

fn device_from_row(
    row: &sqlx::sqlite::SqliteRow,
) -> Result<RememberedDevice, DeviceDependencyError> {
    stored_device_from_row(row).map_err(|source| -> DeviceDependencyError { Box::new(source) })
}

fn stored_device_from_row(row: &sqlx::sqlite::SqliteRow) -> Result<RememberedDevice, StorageError> {
    Ok(RememberedDevice {
        client_id: row.try_get("client_id")?,
        name: row.try_get("name")?,
        is_output: row.try_get("is_output")?,
        added_at: row.try_get("added_at")?,
    })
}

fn box_storage(source: sqlx::Error) -> DeviceDependencyError {
    Box::new(StorageError::Database(source))
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use music_application::devices::RememberedDeviceService;
    use tempfile::tempdir;

    use super::{DeviceImportOutcome, LegacyDeviceImportOutcome, LegacyDeviceImportStatus};
    use crate::{SqliteStorage, SqliteStorageOptions};

    #[tokio::test]
    async fn imports_legacy_json_once_without_modifying_the_source()
    -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let directory = tempdir()?;
        let source = directory.path().join("devices.json");
        let source_bytes = br#"{
          "tv": {"name": "Living Room", "is_output": true,
                 "added_at": "2026-06-05T12:00:00+00:00"},
          "phone": {"name": "Phone", "is_output": false},
          "ignored": "not-a-record"
        }"#;
        std::fs::write(&source, source_bytes)?;
        let storage = Arc::new(
            SqliteStorage::open(SqliteStorageOptions::new(directory.path().join("app.db"))).await?,
        );

        let first = storage.import_legacy_devices_once(&source).await?;
        assert!(matches!(
            first,
            LegacyDeviceImportOutcome::Applied(ref record)
                if record.status == LegacyDeviceImportStatus::Imported
                    && record.imported_count == 2
        ));
        assert_eq!(std::fs::read(&source)?, source_bytes);
        let service = RememberedDeviceService::new(Arc::clone(&storage));
        let devices = service.list().await?;
        assert_eq!(devices.len(), 2);
        assert_eq!(devices[0].client_id, "tv");
        assert!(devices[0].is_output);
        assert_eq!(
            devices[0].added_at.as_deref(),
            Some("2026-06-05T12:00:00+00:00")
        );

        std::fs::write(&source, b"{}")?;
        assert!(matches!(
            storage.import_legacy_devices_once(&source).await?,
            LegacyDeviceImportOutcome::AlreadyRecorded(ref record)
                if record.imported_count == 2
        ));
        assert_eq!(service.list().await?.len(), 2);
        Ok(())
    }

    #[tokio::test]
    async fn missing_or_corrupt_legacy_input_is_recorded_as_a_safe_empty_state()
    -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        for (name, bytes, expected) in [
            ("missing", None, LegacyDeviceImportStatus::Missing),
            (
                "corrupt",
                Some(b"{broken".as_slice()),
                LegacyDeviceImportStatus::Corrupt,
            ),
        ] {
            let directory = tempdir()?;
            let source = directory.path().join(format!("{name}.json"));
            if let Some(bytes) = bytes {
                std::fs::write(&source, bytes)?;
            }
            let storage =
                SqliteStorage::open(SqliteStorageOptions::new(directory.path().join("app.db")))
                    .await?;
            assert!(matches!(
                storage.import_legacy_devices_once(&source).await?,
                LegacyDeviceImportOutcome::Applied(ref record)
                    if record.status == expected && record.imported_count == 0
            ));
        }
        Ok(())
    }

    #[tokio::test]
    async fn sqlite_device_crud_preserves_added_time_and_sorts_outputs_first()
    -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let directory = tempdir()?;
        let storage = Arc::new(
            SqliteStorage::open(SqliteStorageOptions::new(directory.path().join("app.db"))).await?,
        );
        let service = RememberedDeviceService::new(storage);
        let phone = service.save("phone", "Phone", false).await?;
        let first_added = phone.added_at.clone();
        service.save("tv", "Living TV", true).await?;
        let updated = service.save("phone", "Mobile", true).await?;
        assert_eq!(updated.added_at, first_added);
        let list = service.list().await?;
        assert_eq!(list.len(), 2);
        assert!(list.iter().all(|device| device.is_output));
        assert!(service.forget("phone").await?);
        assert!(!service.forget("phone").await?);
        Ok(())
    }

    #[tokio::test]
    async fn versioned_transfer_refuses_clobber_and_requires_explicit_replacement()
    -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let directory = tempdir()?;
        let source = Arc::new(
            SqliteStorage::open(SqliteStorageOptions::new(
                directory.path().join("source.db"),
            ))
            .await?,
        );
        let source_service = RememberedDeviceService::new(Arc::clone(&source));
        source_service.save("phone", "Phone", false).await?;
        source_service.save("tv", "Living TV", true).await?;
        let transfer = directory.path().join("remembered-devices.json");
        let exported = source.export_remembered_devices(&transfer).await?;
        assert_eq!(exported.schema_version, "remembered-devices/v1");
        assert_eq!(exported.exported_count, 2);
        let original = std::fs::read(&transfer)?;
        assert!(source.export_remembered_devices(&transfer).await.is_err());
        assert_eq!(std::fs::read(&transfer)?, original);

        let target = Arc::new(
            SqliteStorage::open(SqliteStorageOptions::new(
                directory.path().join("target.db"),
            ))
            .await?,
        );
        let target_service = RememberedDeviceService::new(Arc::clone(&target));
        target_service.save("old", "Old output", true).await?;
        assert_eq!(
            target.import_remembered_devices(&transfer, false).await?,
            DeviceImportOutcome::TargetNotEmpty { existing_count: 1 }
        );
        assert_eq!(target_service.list().await?.len(), 1);
        assert!(matches!(
            target.import_remembered_devices(&transfer, true).await?,
            DeviceImportOutcome::Imported {
                imported_count: 2,
                replaced_count: 1,
                ..
            }
        ));
        let imported = target_service.list().await?;
        assert_eq!(imported.len(), 2);
        assert!(imported.iter().any(|device| device.client_id == "tv"));

        let invalid = directory.path().join("invalid.json");
        std::fs::write(
            &invalid,
            br#"{"schema_version":"remembered-devices/v1","devices":[{"client_id":"tv","name":"One","is_output":true},{"client_id":"tv","name":"Two","is_output":false}]}"#,
        )?;
        assert!(
            target
                .import_remembered_devices(&invalid, true)
                .await
                .is_err()
        );
        assert_eq!(target_service.list().await?.len(), 2);
        Ok(())
    }
}
