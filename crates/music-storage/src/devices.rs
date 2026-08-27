use std::path::Path;

use music_application::devices::{
    DeviceDependencyError, DeviceFuture, RememberedDevice, RememberedDeviceRepository,
};
use serde_json::{Map, Value};
use sha2::{Digest, Sha256};
use sqlx::Row;

use crate::{SqliteStorage, StorageError};

const MAX_LEGACY_DEVICE_FILE_BYTES: u64 = 4 * 1024 * 1024;

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
    Ok(RememberedDevice {
        client_id: row.try_get("client_id").map_err(box_sqlx)?,
        name: row.try_get("name").map_err(box_sqlx)?,
        is_output: row.try_get("is_output").map_err(box_sqlx)?,
        added_at: row.try_get("added_at").map_err(box_sqlx)?,
    })
}

fn box_storage(source: sqlx::Error) -> DeviceDependencyError {
    Box::new(StorageError::Database(source))
}

fn box_sqlx(source: sqlx::Error) -> DeviceDependencyError {
    box_storage(source)
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use music_application::devices::RememberedDeviceService;
    use tempfile::tempdir;

    use super::{LegacyDeviceImportOutcome, LegacyDeviceImportStatus};
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
}
