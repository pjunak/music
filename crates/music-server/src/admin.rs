use std::collections::{BTreeMap, BTreeSet};
use std::error::Error;
use std::fmt::{self, Display, Formatter};
use std::fs::{self, File, OpenOptions};
use std::io::{self, Cursor, Read, Write};
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use axum::body::Body;
use axum::extract::State;
use axum::http::header::{
    CACHE_CONTROL, CONTENT_DISPOSITION, CONTENT_LENGTH, CONTENT_TYPE, HeaderValue,
};
use axum::http::{HeaderMap, Response};
use flate2::Compression;
use flate2::read::GzDecoder;
use flate2::write::GzEncoder;
use futures_util::stream;
use music_application::auth::SessionTouch;
use music_storage::{
    CredentialVault, CryptoError, InstanceLock, SchemaCompatibility, SecretString, SqliteStorage,
    StorageError, inspect_database,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tar::{Archive, Builder, EntryType, Header};
use tempfile::TempDir;
use tokio::io::AsyncReadExt;
use tokio::sync::RwLock;
use utoipa::openapi::RefOr;
use utoipa::openapi::schema::{ObjectBuilder, Schema, SchemaType};
use utoipa::{PartialSchema, ToSchema};
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;
use uuid::Uuid;

use crate::auth::current_session;
use crate::config::AppConfig;
use crate::error::ApiError;
use crate::http::HttpState;

const BACKUP_KIND: &str = "music-state-backup";
const BACKUP_FORMAT_VERSION: u32 = 1;
const MANIFEST_PATH: &str = "manifest.json";
const DATABASE_PATH: &str = "app.db";
const MODES_PATH: &str = "modes";
const ARCHIVE_CHUNK_BYTES: usize = 64 * 1_024;
const MAX_MODE_DEPTH: usize = 32;
const MAX_MODE_ENTRIES: usize = 10_000;
const MAX_MODE_FILE_BYTES: u64 = 64 * 1_024 * 1_024;
const MAX_MODE_TOTAL_BYTES: u64 = 128 * 1_024 * 1_024;
const MAX_MANIFEST_BYTES: u64 = 1_024 * 1_024;
const MAX_DATABASE_BYTES: u64 = 8 * 1_024 * 1_024 * 1_024;
const MAX_CREDENTIAL_KEY_FILE_BYTES: u64 = 4 * 1_024;
const MAX_ARCHIVE_BYTES: u64 = MAX_DATABASE_BYTES + MAX_MODE_TOTAL_BYTES + 16 * 1_024 * 1_024;
const RESTORE_JOURNAL_VERSION: u32 = 1;

#[derive(Debug)]
pub enum BackupError {
    Storage(StorageError),
    Io {
        operation: &'static str,
        path: PathBuf,
        source: io::Error,
    },
    Serialization(serde_json::Error),
    Credential(CryptoError),
    BackgroundTask(tokio::task::JoinError),
    InvalidArchive(&'static str),
    UnsafePath(PathBuf),
    LimitExceeded(&'static str),
    ClockBeforeEpoch,
    RestoreConfirmationRequired,
    ServerStoppedConfirmationRequired,
    CredentialKeyMismatch,
    PendingRestore(PathBuf),
    RestoreRollbackFailed(PathBuf),
}

impl Display for BackupError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        match self {
            Self::Storage(error) => Display::fmt(error, formatter),
            Self::Io {
                operation, path, ..
            } => write!(formatter, "failed to {operation} {}", path.display()),
            Self::Serialization(_) => formatter.write_str("failed to encode backup metadata"),
            Self::Credential(_) => {
                formatter.write_str("configured credential master key is invalid")
            }
            Self::BackgroundTask(_) => formatter.write_str("backup worker did not complete"),
            Self::InvalidArchive(detail) => write!(formatter, "backup is invalid: {detail}"),
            Self::UnsafePath(path) => {
                write!(
                    formatter,
                    "backup contains an unsafe path: {}",
                    path.display()
                )
            }
            Self::LimitExceeded(limit) => write!(formatter, "backup exceeded {limit}"),
            Self::ClockBeforeEpoch => formatter.write_str("system clock is before the Unix epoch"),
            Self::RestoreConfirmationRequired => formatter
                .write_str("restore requires both --replace and --server-stopped confirmations"),
            Self::ServerStoppedConfirmationRequired => {
                formatter.write_str("recovery requires the --server-stopped confirmation")
            }
            Self::CredentialKeyMismatch => formatter
                .write_str("backup credential key identifier does not match the configured key"),
            Self::PendingRestore(path) => write!(
                formatter,
                "an interrupted restore journal requires recovery: {}",
                path.display()
            ),
            Self::RestoreRollbackFailed(path) => write!(
                formatter,
                "restore rollback was incomplete; recovery journal retained at {}",
                path.display()
            ),
        }
    }
}

impl Error for BackupError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Storage(source) => Some(source),
            Self::Io { source, .. } => Some(source),
            Self::Serialization(source) => Some(source),
            Self::Credential(source) => Some(source),
            Self::BackgroundTask(source) => Some(source),
            Self::InvalidArchive(_)
            | Self::UnsafePath(_)
            | Self::LimitExceeded(_)
            | Self::ClockBeforeEpoch
            | Self::RestoreConfirmationRequired
            | Self::ServerStoppedConfirmationRequired
            | Self::CredentialKeyMismatch
            | Self::PendingRestore(_)
            | Self::RestoreRollbackFailed(_) => None,
        }
    }
}

impl From<StorageError> for BackupError {
    fn from(error: StorageError) -> Self {
        Self::Storage(error)
    }
}

impl From<serde_json::Error> for BackupError {
    fn from(error: serde_json::Error) -> Self {
        Self::Serialization(error)
    }
}

impl From<CryptoError> for BackupError {
    fn from(error: CryptoError) -> Self {
        Self::Credential(error)
    }
}

impl From<tokio::task::JoinError> for BackupError {
    fn from(error: tokio::task::JoinError) -> Self {
        Self::BackgroundTask(error)
    }
}

#[derive(Debug, Clone, Default)]
pub(crate) struct MaintenanceGate {
    admission: Arc<RwLock<()>>,
}

impl MaintenanceGate {
    async fn exclusive(&self) -> tokio::sync::RwLockWriteGuard<'_, ()> {
        self.admission.write().await
    }
}

#[derive(Debug)]
pub(crate) struct BackupService {
    storage: Arc<SqliteStorage>,
    config: Arc<AppConfig>,
    maintenance: MaintenanceGate,
}

impl BackupService {
    pub(crate) fn new(
        storage: Arc<SqliteStorage>,
        config: Arc<AppConfig>,
        maintenance: MaintenanceGate,
    ) -> Self {
        Self {
            storage,
            config,
            maintenance,
        }
    }

    async fn prepare(&self) -> Result<PreparedBackup, BackupError> {
        let _maintenance = self.maintenance.exclusive().await;
        let created_unix_seconds = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|_| BackupError::ClockBeforeEpoch)?
            .as_secs();
        let workspace = backup_workspace(&self.config.database_path, &self.config.modes_dir)?;
        let database_path = workspace.path().join(DATABASE_PATH);
        let database = self
            .storage
            .create_verified_snapshot(&database_path)
            .await?;
        let workspace_path = workspace.path().to_path_buf();
        let config = Arc::clone(&self.config);

        let archive = tokio::task::spawn_blocking(move || {
            let modes_path = normalized_path(&config.modes_dir)?;
            ensure_credential_key_is_outside_modes(&config, &modes_path)?;
            let initial_credential_key_id = credential_key_id(&config)?;
            let modes = copy_modes_snapshot(&config.modes_dir, &workspace_path.join(MODES_PATH))?;
            let manifest = BackupManifest {
                kind: BACKUP_KIND.to_owned(),
                format_version: BACKUP_FORMAT_VERSION,
                created_unix_seconds,
                database: DatabaseManifestEntry {
                    path: DATABASE_PATH.to_owned(),
                    bytes: database.bytes,
                    sha256: database.sha256,
                    schema_version: database.schema_version,
                },
                mode_directories: modes.directories,
                modes: modes.files.clone(),
                credential_key_id: initial_credential_key_id,
            };
            let archive_path = workspace_path.join("backup.tar.gz");
            write_archive(
                &archive_path,
                &workspace_path,
                &manifest,
                created_unix_seconds,
            )?;
            verify_archive(&archive_path)?;
            if credential_key_id(&config)? != manifest.credential_key_id {
                return Err(BackupError::InvalidArchive(
                    "credential key changed while the backup was being created",
                ));
            }
            let bytes = metadata(&archive_path, "inspect completed backup archive")?.len();
            Ok::<_, BackupError>((archive_path, bytes))
        })
        .await??;

        Ok(PreparedBackup {
            archive_path: archive.0,
            archive_bytes: archive.1,
            download_name: format!("music-backup-{created_unix_seconds}.tar.gz"),
            _workspace: workspace,
        })
    }
}

#[derive(Debug)]
struct PreparedBackup {
    archive_path: PathBuf,
    archive_bytes: u64,
    download_name: String,
    _workspace: TempDir,
}

#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct BackupManifest {
    kind: String,
    format_version: u32,
    created_unix_seconds: u64,
    database: DatabaseManifestEntry,
    mode_directories: Vec<String>,
    modes: Vec<FileManifestEntry>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    credential_key_id: Option<String>,
}

#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct DatabaseManifestEntry {
    path: String,
    bytes: u64,
    sha256: String,
    schema_version: Option<i64>,
}

#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct FileManifestEntry {
    path: String,
    bytes: u64,
    sha256: String,
}

#[derive(Debug)]
struct ModesSnapshot {
    directories: Vec<String>,
    files: Vec<FileManifestEntry>,
}

fn backup_workspace(database_path: &Path, modes_path: &Path) -> Result<TempDir, BackupError> {
    let parent = database_path
        .parent()
        .filter(|path| !path.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    let parent_is_in_modes = fs::canonicalize(parent)
        .ok()
        .zip(fs::canonicalize(modes_path).ok())
        .is_some_and(|(parent, modes)| parent.starts_with(modes));
    let mut builder = tempfile::Builder::new();
    builder.prefix(".music-backup-");
    if parent_is_in_modes {
        builder
            .tempdir()
            .map_err(|source| io_error("create backup workspace", parent, source))
    } else {
        builder
            .tempdir_in(parent)
            .map_err(|source| io_error("create backup workspace", parent, source))
    }
}

fn copy_modes_snapshot(source: &Path, destination: &Path) -> Result<ModesSnapshot, BackupError> {
    create_directory(destination, "create modes backup root")?;
    let source_metadata = symlink_metadata(source, "inspect modes directory")?;
    if source_metadata.file_type().is_symlink() || !source_metadata.is_dir() {
        return Err(BackupError::UnsafePath(source.to_path_buf()));
    }
    let canonical_root = fs::canonicalize(source)
        .map_err(|source_error| io_error("resolve modes directory", source, source_error))?;
    let mut snapshot = ModesSnapshot {
        directories: vec![MODES_PATH.to_owned()],
        files: Vec::new(),
    };
    let mut entry_count = 0_usize;
    let mut total_bytes = 0_u64;
    copy_modes_directory(
        source,
        destination,
        &canonical_root,
        source,
        0,
        &mut entry_count,
        &mut total_bytes,
        &mut snapshot,
    )?;
    snapshot.directories.sort();
    snapshot
        .files
        .sort_by(|left, right| left.path.cmp(&right.path));
    Ok(snapshot)
}

#[allow(clippy::too_many_arguments)]
fn copy_modes_directory(
    source_root: &Path,
    destination_root: &Path,
    canonical_root: &Path,
    directory: &Path,
    depth: usize,
    entry_count: &mut usize,
    total_bytes: &mut u64,
    snapshot: &mut ModesSnapshot,
) -> Result<(), BackupError> {
    if depth > MAX_MODE_DEPTH {
        return Err(BackupError::LimitExceeded(
            "the maximum modes directory depth",
        ));
    }
    let mut entries = fs::read_dir(directory)
        .map_err(|source| io_error("read modes directory", directory, source))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|source| io_error("read modes directory entry", directory, source))?;
    entries.sort_by_key(std::fs::DirEntry::file_name);

    for entry in entries {
        *entry_count = entry_count
            .checked_add(1)
            .ok_or(BackupError::LimitExceeded("the modes entry count"))?;
        if *entry_count > MAX_MODE_ENTRIES {
            return Err(BackupError::LimitExceeded("the modes entry count"));
        }
        let source_path = entry.path();
        let entry_metadata = symlink_metadata(&source_path, "inspect modes entry")?;
        if entry_metadata.file_type().is_symlink() {
            return Err(BackupError::UnsafePath(source_path));
        }
        ensure_beneath_root(&source_path, canonical_root)?;
        let relative = source_path
            .strip_prefix(source_root)
            .map_err(|_| BackupError::UnsafePath(source_path.clone()))?;
        let portable_relative = portable_relative_path(relative)?;
        let archive_path = format!("{MODES_PATH}/{portable_relative}");
        let destination_path = destination_root.join(relative);

        if entry_metadata.is_dir() {
            create_directory(&destination_path, "create modes backup directory")?;
            snapshot.directories.push(archive_path);
            copy_modes_directory(
                source_root,
                destination_root,
                canonical_root,
                &source_path,
                depth + 1,
                entry_count,
                total_bytes,
                snapshot,
            )?;
        } else if entry_metadata.is_file() {
            let (sha256, bytes) =
                copy_and_hash_mode_file(&source_path, &destination_path, total_bytes)?;
            snapshot.files.push(FileManifestEntry {
                path: archive_path,
                bytes,
                sha256,
            });
        } else {
            return Err(BackupError::UnsafePath(source_path));
        }
    }
    Ok(())
}

fn ensure_beneath_root(path: &Path, canonical_root: &Path) -> Result<(), BackupError> {
    let canonical =
        fs::canonicalize(path).map_err(|source| io_error("resolve modes entry", path, source))?;
    if canonical.starts_with(canonical_root) {
        Ok(())
    } else {
        Err(BackupError::UnsafePath(path.to_path_buf()))
    }
}

fn copy_and_hash_mode_file(
    source: &Path,
    destination: &Path,
    total_bytes: &mut u64,
) -> Result<(String, u64), BackupError> {
    let mut input = File::open(source)
        .map_err(|source_error| io_error("open modes source file", source, source_error))?;
    if !input
        .metadata()
        .map_err(|source_error| io_error("inspect modes source file", source, source_error))?
        .is_file()
    {
        return Err(BackupError::UnsafePath(source.to_path_buf()));
    }
    let mut output = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(destination)
        .map_err(|source_error| io_error("create modes backup file", destination, source_error))?;
    let mut digest = Sha256::new();
    let mut bytes = 0_u64;
    let mut buffer = [0_u8; ARCHIVE_CHUNK_BYTES];
    loop {
        let read = input
            .read(&mut buffer)
            .map_err(|source_error| io_error("read modes source file", source, source_error))?;
        if read == 0 {
            break;
        }
        bytes = bytes
            .checked_add(read as u64)
            .ok_or(BackupError::LimitExceeded("the maximum modes file size"))?;
        if bytes > MAX_MODE_FILE_BYTES {
            return Err(BackupError::LimitExceeded("the maximum modes file size"));
        }
        *total_bytes = total_bytes
            .checked_add(read as u64)
            .ok_or(BackupError::LimitExceeded("the total modes size"))?;
        if *total_bytes > MAX_MODE_TOTAL_BYTES {
            return Err(BackupError::LimitExceeded("the total modes size"));
        }
        digest.update(&buffer[..read]);
        output.write_all(&buffer[..read]).map_err(|source_error| {
            io_error("write modes backup file", destination, source_error)
        })?;
    }
    output.sync_all().map_err(|source_error| {
        io_error("synchronize modes backup file", destination, source_error)
    })?;
    Ok((hex_digest(&digest.finalize()), bytes))
}

fn credential_key_id(config: &AppConfig) -> Result<Option<String>, BackupError> {
    if let Some(secret) = config.assistant_credential_key.as_ref() {
        return Ok(Some(
            CredentialVault::from_encoded_key(secret.expose_secret())?
                .key_id()
                .to_owned(),
        ));
    }
    let Some(path) = config.assistant_credential_key_file.as_ref() else {
        return Ok(None);
    };
    let key_metadata = match fs::metadata(path) {
        Ok(metadata) => metadata,
        Err(source) if source.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(source) => return Err(io_error("inspect credential key file", path, source)),
    };
    if !key_metadata.is_file() || key_metadata.len() > MAX_CREDENTIAL_KEY_FILE_BYTES {
        return Err(BackupError::LimitExceeded("the credential key file size"));
    }
    let encoded = SecretString::new(
        fs::read_to_string(path)
            .map_err(|source| io_error("read credential key file", path, source))?,
    );
    if encoded.expose_secret().len() as u64 > MAX_CREDENTIAL_KEY_FILE_BYTES {
        return Err(BackupError::LimitExceeded("the credential key file size"));
    }
    Ok(Some(
        CredentialVault::from_encoded_key(encoded.expose_secret())?
            .key_id()
            .to_owned(),
    ))
}

fn write_archive(
    archive_path: &Path,
    workspace: &Path,
    manifest: &BackupManifest,
    created_unix_seconds: u64,
) -> Result<(), BackupError> {
    let archive_file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(archive_path)
        .map_err(|source| io_error("create backup archive", archive_path, source))?;
    let encoder = GzEncoder::new(archive_file, Compression::default());
    let mut builder = Builder::new(encoder);
    let mut manifest_bytes = serde_json::to_vec_pretty(manifest)?;
    manifest_bytes.push(b'\n');
    if manifest_bytes.len() as u64 > MAX_MANIFEST_BYTES {
        return Err(BackupError::LimitExceeded("the backup manifest size"));
    }

    append_bytes(
        &mut builder,
        MANIFEST_PATH,
        &manifest_bytes,
        created_unix_seconds,
    )?;
    append_file(
        &mut builder,
        DATABASE_PATH,
        &workspace.join(DATABASE_PATH),
        manifest.database.bytes,
        created_unix_seconds,
    )?;
    for directory in &manifest.mode_directories {
        append_directory(&mut builder, directory, created_unix_seconds)?;
    }
    for mode in &manifest.modes {
        append_file(
            &mut builder,
            &mode.path,
            &workspace.join(Path::new(&mode.path)),
            mode.bytes,
            created_unix_seconds,
        )?;
    }
    builder
        .finish()
        .map_err(|source| io_error("finish backup tar stream", archive_path, source))?;
    let encoder = builder
        .into_inner()
        .map_err(|source| io_error("finalize backup tar stream", archive_path, source))?;
    let archive_file = encoder
        .finish()
        .map_err(|source| io_error("finish backup gzip stream", archive_path, source))?;
    archive_file
        .sync_all()
        .map_err(|source| io_error("synchronize backup archive", archive_path, source))?;
    Ok(())
}

fn append_bytes(
    builder: &mut Builder<GzEncoder<File>>,
    archive_path: &str,
    bytes: &[u8],
    modified: u64,
) -> Result<(), BackupError> {
    let mut header = regular_header(bytes.len() as u64, modified);
    builder
        .append_data(&mut header, archive_path, Cursor::new(bytes))
        .map_err(|source| io_error("append backup metadata", Path::new(archive_path), source))
}

fn append_file(
    builder: &mut Builder<GzEncoder<File>>,
    archive_path: &str,
    source_path: &Path,
    bytes: u64,
    modified: u64,
) -> Result<(), BackupError> {
    let mut source = File::open(source_path)
        .map_err(|error| io_error("open backup payload", source_path, error))?;
    let mut header = regular_header(bytes, modified);
    builder
        .append_data(&mut header, archive_path, &mut source)
        .map_err(|error| io_error("append backup payload", source_path, error))
}

fn append_directory(
    builder: &mut Builder<GzEncoder<File>>,
    archive_path: &str,
    modified: u64,
) -> Result<(), BackupError> {
    let mut header = Header::new_gnu();
    header.set_entry_type(EntryType::Directory);
    header.set_mode(0o700);
    header.set_uid(0);
    header.set_gid(0);
    header.set_mtime(modified);
    header.set_size(0);
    header.set_cksum();
    builder
        .append_data(&mut header, archive_path, io::empty())
        .map_err(|source| io_error("append backup directory", Path::new(archive_path), source))
}

fn regular_header(bytes: u64, modified: u64) -> Header {
    let mut header = Header::new_gnu();
    header.set_entry_type(EntryType::Regular);
    header.set_mode(0o600);
    header.set_uid(0);
    header.set_gid(0);
    header.set_mtime(modified);
    header.set_size(bytes);
    header.set_cksum();
    header
}

fn verify_archive(archive_path: &Path) -> Result<BackupManifest, BackupError> {
    let archive_metadata = symlink_metadata(archive_path, "inspect backup archive")?;
    if archive_metadata.file_type().is_symlink() || !archive_metadata.is_file() {
        return Err(BackupError::UnsafePath(archive_path.to_path_buf()));
    }
    if archive_metadata.len() > MAX_ARCHIVE_BYTES {
        return Err(BackupError::LimitExceeded("the compressed backup size"));
    }
    let file = File::open(archive_path)
        .map_err(|source| io_error("open backup for verification", archive_path, source))?;
    let mut archive = Archive::new(GzDecoder::new(file));
    let entries = archive
        .entries()
        .map_err(|source| io_error("read backup archive", archive_path, source))?;
    let mut manifest = None;
    let mut observed = BTreeMap::new();
    let mut directories = BTreeSet::new();
    let mut entry_count = 0_usize;
    let mut mode_bytes = 0_u64;

    for entry in entries {
        entry_count = entry_count
            .checked_add(1)
            .ok_or(BackupError::LimitExceeded("the backup entry count"))?;
        if entry_count > MAX_MODE_ENTRIES + 3 {
            return Err(BackupError::LimitExceeded("the backup entry count"));
        }
        let mut entry =
            entry.map_err(|source| io_error("read backup entry", archive_path, source))?;
        let raw_path = entry
            .path()
            .map_err(|source| io_error("read backup entry path", archive_path, source))?
            .into_owned();
        let path = portable_archive_path(&raw_path)?;
        let entry_type = entry.header().entry_type();
        if entry_type.is_dir() {
            if !directories.insert(path) {
                return Err(BackupError::InvalidArchive("duplicate directory entry"));
            }
            continue;
        }
        if !entry_type.is_file() {
            return Err(BackupError::InvalidArchive(
                "links and special entries are not allowed",
            ));
        }
        if path == MANIFEST_PATH {
            if manifest.is_some() {
                return Err(BackupError::InvalidArchive("duplicate manifest"));
            }
            if entry.size() > MAX_MANIFEST_BYTES {
                return Err(BackupError::LimitExceeded("the backup manifest size"));
            }
            let bytes = read_bounded(&mut entry, MAX_MANIFEST_BYTES, "the backup manifest size")?;
            manifest = Some(serde_json::from_slice::<BackupManifest>(&bytes)?);
            continue;
        }
        let limit = if path == DATABASE_PATH {
            MAX_DATABASE_BYTES
        } else if path.starts_with("modes/") {
            MAX_MODE_FILE_BYTES
        } else {
            return Err(BackupError::InvalidArchive("unexpected backup payload"));
        };
        if entry.size() > limit {
            return Err(BackupError::LimitExceeded("the backup payload size"));
        }
        if observed.contains_key(&path) {
            return Err(BackupError::InvalidArchive("duplicate payload entry"));
        }
        let observation = hash_bounded(&mut entry, limit, "the backup payload size")?;
        if path.starts_with("modes/") {
            mode_bytes = mode_bytes
                .checked_add(observation.bytes)
                .ok_or(BackupError::LimitExceeded("the total modes size"))?;
            if mode_bytes > MAX_MODE_TOTAL_BYTES {
                return Err(BackupError::LimitExceeded("the total modes size"));
            }
        }
        observed.insert(path, observation);
    }

    let manifest = manifest.ok_or(BackupError::InvalidArchive("manifest is missing"))?;
    validate_manifest(&manifest, &observed, &directories)?;
    Ok(manifest)
}

fn validate_manifest(
    manifest: &BackupManifest,
    observed: &BTreeMap<String, ObservedFile>,
    directories: &BTreeSet<String>,
) -> Result<(), BackupError> {
    if manifest.kind != BACKUP_KIND || manifest.format_version != BACKUP_FORMAT_VERSION {
        return Err(BackupError::InvalidArchive("unsupported backup format"));
    }
    if manifest.database.path != DATABASE_PATH
        || manifest.database.bytes > MAX_DATABASE_BYTES
        || !valid_sha256(&manifest.database.sha256)
    {
        return Err(BackupError::InvalidArchive(
            "invalid database manifest entry",
        ));
    }
    if manifest
        .credential_key_id
        .as_ref()
        .is_some_and(|key_id| !valid_key_id(key_id))
    {
        return Err(BackupError::InvalidArchive(
            "invalid credential key identifier",
        ));
    }

    let mut expected = BTreeMap::from([(
        DATABASE_PATH.to_owned(),
        ObservedFile {
            bytes: manifest.database.bytes,
            sha256: manifest.database.sha256.clone(),
        },
    )]);
    if manifest.mode_directories.is_empty()
        || manifest.mode_directories.first().map(String::as_str) != Some(MODES_PATH)
    {
        return Err(BackupError::InvalidArchive(
            "modes directory manifest is missing its root",
        ));
    }
    let mut expected_directories = BTreeSet::new();
    let mut previous_directory = None;
    for directory in &manifest.mode_directories {
        if (directory != MODES_PATH && !directory.starts_with("modes/"))
            || portable_archive_path(Path::new(directory))? != *directory
            || previous_directory
                .as_ref()
                .is_some_and(|previous| previous >= directory)
        {
            return Err(BackupError::InvalidArchive(
                "invalid modes directory manifest entry",
            ));
        }
        previous_directory = Some(directory.clone());
        expected_directories.insert(directory.clone());
    }
    let mut previous_path = None;
    let mut mode_bytes = 0_u64;
    for mode in &manifest.modes {
        if !mode.path.starts_with("modes/")
            || portable_archive_path(Path::new(&mode.path))? != mode.path
            || !valid_sha256(&mode.sha256)
            || mode.bytes > MAX_MODE_FILE_BYTES
        {
            return Err(BackupError::InvalidArchive("invalid modes manifest entry"));
        }
        if previous_path
            .as_ref()
            .is_some_and(|path| path >= &mode.path)
        {
            return Err(BackupError::InvalidArchive(
                "modes manifest entries are not uniquely sorted",
            ));
        }
        previous_path = Some(mode.path.clone());
        mode_bytes = mode_bytes
            .checked_add(mode.bytes)
            .ok_or(BackupError::LimitExceeded("the total modes size"))?;
        if mode_bytes > MAX_MODE_TOTAL_BYTES {
            return Err(BackupError::LimitExceeded("the total modes size"));
        }
        if expected
            .insert(
                mode.path.clone(),
                ObservedFile {
                    bytes: mode.bytes,
                    sha256: mode.sha256.clone(),
                },
            )
            .is_some()
        {
            return Err(BackupError::InvalidArchive("duplicate manifest payload"));
        }
        let parent = Path::new(&mode.path)
            .parent()
            .ok_or(BackupError::InvalidArchive("mode payload has no parent"))?;
        let parent = portable_archive_path(parent)?;
        if !expected_directories.contains(&parent) {
            return Err(BackupError::InvalidArchive(
                "mode payload parent directory is missing from the manifest",
            ));
        }
    }
    if expected != *observed {
        return Err(BackupError::InvalidArchive(
            "payload hashes do not match the manifest",
        ));
    }
    if expected_directories != *directories {
        return Err(BackupError::InvalidArchive(
            "directory entries do not match the manifest",
        ));
    }
    Ok(())
}

#[derive(Debug, Clone, Eq, PartialEq)]
struct ObservedFile {
    bytes: u64,
    sha256: String,
}

fn hash_bounded(
    reader: &mut impl Read,
    limit: u64,
    limit_name: &'static str,
) -> Result<ObservedFile, BackupError> {
    let mut digest = Sha256::new();
    let mut bytes = 0_u64;
    let mut buffer = [0_u8; ARCHIVE_CHUNK_BYTES];
    loop {
        let read = reader
            .read(&mut buffer)
            .map_err(|source| io_error("read backup payload", Path::new("<archive>"), source))?;
        if read == 0 {
            break;
        }
        bytes = bytes
            .checked_add(read as u64)
            .ok_or(BackupError::LimitExceeded(limit_name))?;
        if bytes > limit {
            return Err(BackupError::LimitExceeded(limit_name));
        }
        digest.update(&buffer[..read]);
    }
    Ok(ObservedFile {
        bytes,
        sha256: hex_digest(&digest.finalize()),
    })
}

fn read_bounded(
    reader: &mut impl Read,
    limit: u64,
    limit_name: &'static str,
) -> Result<Vec<u8>, BackupError> {
    let capacity = usize::try_from(limit).map_err(|_| BackupError::LimitExceeded(limit_name))?;
    let mut bytes = Vec::with_capacity(capacity.min(64 * 1_024));
    reader
        .take(limit + 1)
        .read_to_end(&mut bytes)
        .map_err(|source| io_error("read backup metadata", Path::new("<archive>"), source))?;
    if bytes.len() as u64 > limit {
        Err(BackupError::LimitExceeded(limit_name))
    } else {
        Ok(bytes)
    }
}

fn portable_relative_path(path: &Path) -> Result<String, BackupError> {
    portable_path(path, false)
}

fn portable_archive_path(path: &Path) -> Result<String, BackupError> {
    portable_path(path, true)
}

fn portable_path(path: &Path, allow_empty: bool) -> Result<String, BackupError> {
    let mut components = Vec::new();
    for component in path.components() {
        let Component::Normal(component) = component else {
            return Err(BackupError::UnsafePath(path.to_path_buf()));
        };
        let component = component
            .to_str()
            .filter(|value| !value.is_empty() && !value.contains('/') && !value.contains('\\'))
            .ok_or_else(|| BackupError::UnsafePath(path.to_path_buf()))?;
        components.push(component);
    }
    if components.is_empty() && !allow_empty {
        return Err(BackupError::UnsafePath(path.to_path_buf()));
    }
    Ok(components.join("/"))
}

fn valid_sha256(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn valid_key_id(value: &str) -> bool {
    value.len() == 16
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn hex_digest(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut encoded = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        encoded.push(char::from(HEX[usize::from(byte >> 4)]));
        encoded.push(char::from(HEX[usize::from(byte & 0x0f)]));
    }
    encoded
}

fn create_directory(path: &Path, operation: &'static str) -> Result<(), BackupError> {
    fs::create_dir(path).map_err(|source| io_error(operation, path, source))
}

fn metadata(path: &Path, operation: &'static str) -> Result<fs::Metadata, BackupError> {
    fs::metadata(path).map_err(|source| io_error(operation, path, source))
}

fn symlink_metadata(path: &Path, operation: &'static str) -> Result<fs::Metadata, BackupError> {
    fs::symlink_metadata(path).map_err(|source| io_error(operation, path, source))
}

fn io_error(operation: &'static str, path: &Path, source: io::Error) -> BackupError {
    BackupError::Io {
        operation,
        path: path.to_path_buf(),
        source,
    }
}

#[derive(Debug, Clone)]
pub struct RestoreOptions {
    pub archive_path: PathBuf,
    pub database_path: PathBuf,
    pub modes_path: PathBuf,
    pub replace: bool,
    pub server_stopped: bool,
}

#[derive(Debug, Clone, Eq, PartialEq, Serialize)]
pub struct RestoreOutcome {
    pub format_version: u32,
    pub database_path: PathBuf,
    pub modes_path: PathBuf,
    pub restored_mode_files: usize,
    pub database_sha256: String,
    pub credential_key_id: Option<String>,
    pub previous_database_path: Option<PathBuf>,
    pub previous_modes_path: Option<PathBuf>,
    pub previous_sidecar_paths: Vec<PathBuf>,
}

#[derive(Debug, Clone, Eq, PartialEq, Serialize)]
pub struct RestoreRecoveryOutcome {
    pub journal_path: PathBuf,
    pub recovered_targets: Vec<PathBuf>,
    pub preserved_interrupted_targets: Vec<PathBuf>,
}

#[derive(Debug, Clone, Copy, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum RestoreTargetKind {
    File,
    Directory,
}

#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct RestoreTarget {
    target: PathBuf,
    previous: PathBuf,
    previous_existed: bool,
    kind: RestoreTargetKind,
}

#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct RestoreJournal {
    version: u32,
    operation_id: String,
    database_path: PathBuf,
    modes_path: PathBuf,
    targets: Vec<RestoreTarget>,
}

#[derive(Debug)]
struct PreviousRestoreState {
    database: Option<PathBuf>,
    modes: Option<PathBuf>,
    sidecars: Vec<PathBuf>,
}

/// Restore a verified Rust backup while the server is offline. The two
/// confirmations deliberately remain separate so a copied command cannot
/// silently replace live state.
pub async fn restore_backup(
    config: &AppConfig,
    options: RestoreOptions,
) -> Result<RestoreOutcome, BackupError> {
    if !options.replace || !options.server_stopped {
        return Err(BackupError::RestoreConfirmationRequired);
    }
    validate_optional_target(&options.database_path, RestoreTargetKind::File)?;
    validate_optional_target(&options.modes_path, RestoreTargetKind::Directory)?;
    let archive_path = normalized_path(&options.archive_path)?;
    let database_path = normalized_path(&options.database_path)?;
    let modes_path = normalized_path(&options.modes_path)?;
    validate_restore_paths(&archive_path, &database_path, &modes_path)?;
    ensure_credential_key_is_outside_modes(config, &modes_path)?;
    let journal_path = restore_journal_path(&database_path)?;
    if path_try_exists(&journal_path, "inspect restore journal")? {
        return Err(BackupError::PendingRestore(journal_path));
    }

    let _instance_lock = InstanceLock::acquire(&database_path)?;
    let archive_for_verification = archive_path.clone();
    let manifest =
        tokio::task::spawn_blocking(move || verify_archive(&archive_for_verification)).await??;
    let configured_key_id = credential_key_id(config)?;
    if manifest.credential_key_id != configured_key_id {
        return Err(BackupError::CredentialKeyMismatch);
    }

    let database_parent = required_parent(&database_path)?;
    let modes_parent = required_parent(&modes_path)?;
    let database_workspace = tempfile::Builder::new()
        .prefix(".music-restore-db-")
        .tempdir_in(database_parent)
        .map_err(|source| io_error("create database restore workspace", database_parent, source))?;
    let modes_workspace = tempfile::Builder::new()
        .prefix(".music-restore-modes-")
        .tempdir_in(modes_parent)
        .map_err(|source| io_error("create modes restore workspace", modes_parent, source))?;
    let staged_database = database_workspace.path().join(DATABASE_PATH);
    let staged_modes = modes_workspace.path().join(MODES_PATH);
    create_directory(&staged_modes, "create staged modes directory")?;

    let archive_for_extraction = archive_path.clone();
    let staged_database_for_extraction = staged_database.clone();
    let staged_modes_for_extraction = staged_modes.clone();
    let extracted_manifest = tokio::task::spawn_blocking(move || {
        extract_archive(
            &archive_for_extraction,
            &staged_database_for_extraction,
            &staged_modes_for_extraction,
        )
    })
    .await??;
    if extracted_manifest != manifest {
        return Err(BackupError::InvalidArchive(
            "archive changed while it was being restored",
        ));
    }
    validate_staged_database(&staged_database, &manifest).await?;

    let operation_id = Uuid::new_v4().to_string();
    let journal = build_restore_journal(&operation_id, &database_path, &modes_path)?;
    write_restore_journal(&journal_path, &journal)?;
    let journal_for_commit = journal.clone();
    let journal_path_for_commit = journal_path.clone();
    let staged_database_for_commit = staged_database.clone();
    let staged_modes_for_commit = staged_modes.clone();
    let previous = tokio::task::spawn_blocking(move || {
        commit_restore(
            &journal_path_for_commit,
            &journal_for_commit,
            &staged_database_for_commit,
            &staged_modes_for_commit,
        )
    })
    .await??;

    Ok(RestoreOutcome {
        format_version: manifest.format_version,
        database_path,
        modes_path,
        restored_mode_files: manifest.modes.len(),
        database_sha256: manifest.database.sha256,
        credential_key_id: manifest.credential_key_id,
        previous_database_path: previous.database,
        previous_modes_path: previous.modes,
        previous_sidecar_paths: previous.sidecars,
    })
}

/// Roll back an interrupted restore recorded beside the database. Newly
/// installed targets are moved aside, never deleted, before the prior targets
/// are put back.
pub fn recover_interrupted_restore(
    database_path: &Path,
    modes_path: &Path,
    server_stopped: bool,
) -> Result<RestoreRecoveryOutcome, BackupError> {
    if !server_stopped {
        return Err(BackupError::ServerStoppedConfirmationRequired);
    }
    validate_optional_target(database_path, RestoreTargetKind::File)?;
    validate_optional_target(modes_path, RestoreTargetKind::Directory)?;
    let database_path = normalized_path(database_path)?;
    let modes_path = normalized_path(modes_path)?;
    let _instance_lock = InstanceLock::acquire(&database_path)?;
    let journal_path = restore_journal_path(&database_path)?;
    let journal = read_restore_journal(&journal_path)?;
    validate_restore_journal(&journal, &database_path, &modes_path)?;
    let recovery = rollback_restore(&journal)?;
    fs::remove_file(&journal_path)
        .map_err(|source| io_error("remove completed restore journal", &journal_path, source))?;
    Ok(RestoreRecoveryOutcome {
        journal_path,
        recovered_targets: recovery.recovered,
        preserved_interrupted_targets: recovery.preserved,
    })
}

#[must_use]
pub fn pending_restore_journal(database_path: &Path) -> Option<PathBuf> {
    let path = restore_journal_path(database_path).ok()?;
    path.try_exists()
        .ok()
        .filter(|exists| *exists)
        .map(|_| path)
}

fn extract_archive(
    archive_path: &Path,
    staged_database: &Path,
    staged_modes: &Path,
) -> Result<BackupManifest, BackupError> {
    let file = File::open(archive_path)
        .map_err(|source| io_error("open backup for extraction", archive_path, source))?;
    let mut archive = Archive::new(GzDecoder::new(file));
    let entries = archive
        .entries()
        .map_err(|source| io_error("read backup archive", archive_path, source))?;
    let mut manifest = None;
    let mut observed = BTreeMap::new();
    let mut directories = BTreeSet::new();
    let mut entry_count = 0_usize;
    let mut mode_bytes = 0_u64;

    for entry in entries {
        entry_count = entry_count
            .checked_add(1)
            .ok_or(BackupError::LimitExceeded("the backup entry count"))?;
        if entry_count > MAX_MODE_ENTRIES + 3 {
            return Err(BackupError::LimitExceeded("the backup entry count"));
        }
        let mut entry =
            entry.map_err(|source| io_error("read backup entry", archive_path, source))?;
        let raw_path = entry
            .path()
            .map_err(|source| io_error("read backup entry path", archive_path, source))?
            .into_owned();
        let path = portable_archive_path(&raw_path)?;
        let entry_type = entry.header().entry_type();
        if entry_type.is_dir() {
            if !directories.insert(path.clone()) {
                return Err(BackupError::InvalidArchive("duplicate directory entry"));
            }
            let destination = staged_mode_path(staged_modes, &path)?;
            fs::create_dir_all(&destination).map_err(|source| {
                io_error("create staged modes directory", &destination, source)
            })?;
            continue;
        }
        if !entry_type.is_file() {
            return Err(BackupError::InvalidArchive(
                "links and special entries are not allowed",
            ));
        }
        if path == MANIFEST_PATH {
            if manifest.is_some() {
                return Err(BackupError::InvalidArchive("duplicate manifest"));
            }
            if entry.size() > MAX_MANIFEST_BYTES {
                return Err(BackupError::LimitExceeded("the backup manifest size"));
            }
            let bytes = read_bounded(&mut entry, MAX_MANIFEST_BYTES, "the backup manifest size")?;
            manifest = Some(serde_json::from_slice::<BackupManifest>(&bytes)?);
            continue;
        }
        if observed.contains_key(&path) {
            return Err(BackupError::InvalidArchive("duplicate payload entry"));
        }
        let (destination, limit) = if path == DATABASE_PATH {
            (staged_database.to_path_buf(), MAX_DATABASE_BYTES)
        } else if path.starts_with("modes/") {
            (staged_mode_path(staged_modes, &path)?, MAX_MODE_FILE_BYTES)
        } else {
            return Err(BackupError::InvalidArchive("unexpected backup payload"));
        };
        if entry.size() > limit {
            return Err(BackupError::LimitExceeded("the backup payload size"));
        }
        let observation =
            write_entry_bounded(&mut entry, &destination, limit, "the backup payload size")?;
        if path.starts_with("modes/") {
            mode_bytes = mode_bytes
                .checked_add(observation.bytes)
                .ok_or(BackupError::LimitExceeded("the total modes size"))?;
            if mode_bytes > MAX_MODE_TOTAL_BYTES {
                return Err(BackupError::LimitExceeded("the total modes size"));
            }
        }
        observed.insert(path, observation);
    }

    let manifest = manifest.ok_or(BackupError::InvalidArchive("manifest is missing"))?;
    validate_manifest(&manifest, &observed, &directories)?;
    Ok(manifest)
}

fn staged_mode_path(staged_modes: &Path, archive_path: &str) -> Result<PathBuf, BackupError> {
    let relative = Path::new(archive_path)
        .strip_prefix(MODES_PATH)
        .map_err(|_| BackupError::InvalidArchive("mode path is outside the modes tree"))?;
    Ok(staged_modes.join(relative))
}

fn write_entry_bounded(
    reader: &mut impl Read,
    destination: &Path,
    limit: u64,
    limit_name: &'static str,
) -> Result<ObservedFile, BackupError> {
    if let Some(parent) = destination.parent() {
        fs::create_dir_all(parent)
            .map_err(|source| io_error("create restore payload directory", parent, source))?;
    }
    let mut output = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(destination)
        .map_err(|source| io_error("create staged restore payload", destination, source))?;
    let mut digest = Sha256::new();
    let mut bytes = 0_u64;
    let mut buffer = [0_u8; ARCHIVE_CHUNK_BYTES];
    loop {
        let read = reader
            .read(&mut buffer)
            .map_err(|source| io_error("read restore payload", destination, source))?;
        if read == 0 {
            break;
        }
        bytes = bytes
            .checked_add(read as u64)
            .ok_or(BackupError::LimitExceeded(limit_name))?;
        if bytes > limit {
            return Err(BackupError::LimitExceeded(limit_name));
        }
        digest.update(&buffer[..read]);
        output
            .write_all(&buffer[..read])
            .map_err(|source| io_error("write staged restore payload", destination, source))?;
    }
    output
        .sync_all()
        .map_err(|source| io_error("synchronize staged restore payload", destination, source))?;
    Ok(ObservedFile {
        bytes,
        sha256: hex_digest(&digest.finalize()),
    })
}

async fn validate_staged_database(
    database_path: &Path,
    manifest: &BackupManifest,
) -> Result<(), BackupError> {
    let report = inspect_database(database_path).await?;
    if report.compatibility != SchemaCompatibility::Current
        || !report.integrity_ok
        || report.foreign_key_violations != 0
        || report.migration_version != manifest.database.schema_version
    {
        return Err(BackupError::InvalidArchive(
            "staged database failed schema verification",
        ));
    }
    Ok(())
}

fn validate_restore_paths(
    archive_path: &Path,
    database_path: &Path,
    modes_path: &Path,
) -> Result<(), BackupError> {
    let archive = symlink_metadata(archive_path, "inspect restore archive")?;
    if archive.file_type().is_symlink() || !archive.is_file() {
        return Err(BackupError::UnsafePath(archive_path.to_path_buf()));
    }
    let database_parent = required_parent(database_path)?;
    let modes_parent = required_parent(modes_path)?;
    if !metadata(database_parent, "inspect database parent")?.is_dir()
        || !metadata(modes_parent, "inspect modes parent")?.is_dir()
        || archive_path == database_path
        || archive_path == modes_path
        || database_path.starts_with(modes_path)
    {
        return Err(BackupError::UnsafePath(database_path.to_path_buf()));
    }
    validate_optional_target(database_path, RestoreTargetKind::File)?;
    validate_optional_target(modes_path, RestoreTargetKind::Directory)?;
    Ok(())
}

fn validate_optional_target(path: &Path, kind: RestoreTargetKind) -> Result<(), BackupError> {
    let target = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(source) if source.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(source) => return Err(io_error("inspect restore target", path, source)),
    };
    let valid_kind = match kind {
        RestoreTargetKind::File => target.is_file(),
        RestoreTargetKind::Directory => target.is_dir(),
    };
    if target.file_type().is_symlink() || !valid_kind {
        Err(BackupError::UnsafePath(path.to_path_buf()))
    } else {
        Ok(())
    }
}

fn ensure_credential_key_is_outside_modes(
    config: &AppConfig,
    modes_path: &Path,
) -> Result<(), BackupError> {
    let Some(key_path) = config.assistant_credential_key_file.as_ref() else {
        return Ok(());
    };
    let key_path = normalized_path(key_path)?;
    if key_path.starts_with(modes_path) {
        Err(BackupError::InvalidArchive(
            "credential key file must not be inside the modes directory",
        ))
    } else {
        Ok(())
    }
}

fn normalized_path(path: &Path) -> Result<PathBuf, BackupError> {
    let absolute = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()
            .map_err(|source| io_error("resolve current directory", path, source))?
            .join(path)
    };
    let mut existing = absolute.as_path();
    let mut suffix = Vec::new();
    loop {
        match existing.try_exists() {
            Ok(true) => break,
            Ok(false) => {}
            Err(source) => return Err(io_error("inspect restore path", existing, source)),
        }
        let component = existing
            .file_name()
            .ok_or_else(|| BackupError::UnsafePath(path.to_path_buf()))?;
        suffix.push(component.to_os_string());
        existing = existing
            .parent()
            .ok_or_else(|| BackupError::UnsafePath(path.to_path_buf()))?;
    }
    let mut normalized = fs::canonicalize(existing)
        .map_err(|source| io_error("resolve restore path", path, source))?;
    for component in suffix.iter().rev() {
        normalized.push(component);
    }
    Ok(normalized)
}

fn required_parent(path: &Path) -> Result<&Path, BackupError> {
    if path.file_name().is_none() {
        return Err(BackupError::UnsafePath(path.to_path_buf()));
    }
    path.parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .ok_or_else(|| BackupError::UnsafePath(path.to_path_buf()))
}

fn build_restore_journal(
    operation_id: &str,
    database_path: &Path,
    modes_path: &Path,
) -> Result<RestoreJournal, BackupError> {
    let targets = vec![
        restore_target(database_path, operation_id, RestoreTargetKind::File)?,
        restore_target(
            &sqlite_sidecar_path(database_path, "-wal")?,
            operation_id,
            RestoreTargetKind::File,
        )?,
        restore_target(
            &sqlite_sidecar_path(database_path, "-shm")?,
            operation_id,
            RestoreTargetKind::File,
        )?,
        restore_target(modes_path, operation_id, RestoreTargetKind::Directory)?,
    ];
    Ok(RestoreJournal {
        version: RESTORE_JOURNAL_VERSION,
        operation_id: operation_id.to_owned(),
        database_path: database_path.to_path_buf(),
        modes_path: modes_path.to_path_buf(),
        targets,
    })
}

fn restore_target(
    path: &Path,
    operation_id: &str,
    kind: RestoreTargetKind,
) -> Result<RestoreTarget, BackupError> {
    validate_optional_target(path, kind)?;
    let previous = sibling_with_suffix(path, &format!(".pre-restore-{operation_id}"))?;
    if path_try_exists(&previous, "inspect retained pre-restore target")? {
        return Err(BackupError::InvalidArchive(
            "pre-restore retention target already exists",
        ));
    }
    Ok(RestoreTarget {
        target: path.to_path_buf(),
        previous,
        previous_existed: path_try_exists(path, "inspect restore target")?,
        kind,
    })
}

fn sqlite_sidecar_path(database_path: &Path, suffix: &str) -> Result<PathBuf, BackupError> {
    let file_name = database_path
        .file_name()
        .ok_or_else(|| BackupError::UnsafePath(database_path.to_path_buf()))?;
    let mut sidecar_name = file_name.to_os_string();
    sidecar_name.push(suffix);
    Ok(database_path.with_file_name(sidecar_name))
}

fn sibling_with_suffix(path: &Path, suffix: &str) -> Result<PathBuf, BackupError> {
    let file_name = path
        .file_name()
        .ok_or_else(|| BackupError::UnsafePath(path.to_path_buf()))?;
    let mut retained_name = file_name.to_os_string();
    retained_name.push(suffix);
    Ok(path.with_file_name(retained_name))
}

fn restore_journal_path(database_path: &Path) -> Result<PathBuf, BackupError> {
    sibling_with_suffix(database_path, ".restore-journal.json")
}

fn write_restore_journal(path: &Path, journal: &RestoreJournal) -> Result<(), BackupError> {
    let mut bytes = serde_json::to_vec_pretty(journal)?;
    bytes.push(b'\n');
    if bytes.len() as u64 > MAX_MANIFEST_BYTES {
        return Err(BackupError::LimitExceeded("the restore journal size"));
    }
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .map_err(|source| io_error("create restore journal", path, source))?;
    file.write_all(&bytes)
        .map_err(|source| io_error("write restore journal", path, source))?;
    file.sync_all()
        .map_err(|source| io_error("synchronize restore journal", path, source))
}

fn read_restore_journal(path: &Path) -> Result<RestoreJournal, BackupError> {
    let metadata = symlink_metadata(path, "inspect restore journal")?;
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        return Err(BackupError::UnsafePath(path.to_path_buf()));
    }
    let mut file =
        File::open(path).map_err(|source| io_error("open restore journal", path, source))?;
    let bytes = read_bounded(&mut file, MAX_MANIFEST_BYTES, "the restore journal size")?;
    Ok(serde_json::from_slice(&bytes)?)
}

fn validate_restore_journal(
    journal: &RestoreJournal,
    database_path: &Path,
    modes_path: &Path,
) -> Result<(), BackupError> {
    if journal.version != RESTORE_JOURNAL_VERSION
        || Uuid::parse_str(&journal.operation_id).is_err()
        || journal.database_path != database_path
        || journal.modes_path != modes_path
        || journal.database_path.starts_with(&journal.modes_path)
        || journal.targets.len() != 4
    {
        return Err(BackupError::InvalidArchive("invalid restore journal"));
    }
    let expected = build_restore_journal_without_state(
        &journal.operation_id,
        &journal.database_path,
        &journal.modes_path,
    )?;
    for (actual, expected) in journal.targets.iter().zip(expected) {
        if actual.target != expected.target
            || actual.previous != expected.previous
            || actual.kind != expected.kind
        {
            return Err(BackupError::InvalidArchive(
                "restore journal target does not match its operation",
            ));
        }
    }
    Ok(())
}

fn build_restore_journal_without_state(
    operation_id: &str,
    database_path: &Path,
    modes_path: &Path,
) -> Result<Vec<RestoreTarget>, BackupError> {
    let paths = [
        (database_path.to_path_buf(), RestoreTargetKind::File),
        (
            sqlite_sidecar_path(database_path, "-wal")?,
            RestoreTargetKind::File,
        ),
        (
            sqlite_sidecar_path(database_path, "-shm")?,
            RestoreTargetKind::File,
        ),
        (modes_path.to_path_buf(), RestoreTargetKind::Directory),
    ];
    paths
        .into_iter()
        .map(|(target, kind)| {
            Ok(RestoreTarget {
                previous: sibling_with_suffix(&target, &format!(".pre-restore-{operation_id}"))?,
                target,
                previous_existed: false,
                kind,
            })
        })
        .collect()
}

fn commit_restore(
    journal_path: &Path,
    journal: &RestoreJournal,
    staged_database: &Path,
    staged_modes: &Path,
) -> Result<PreviousRestoreState, BackupError> {
    let commit = (|| {
        for target in &journal.targets {
            if target.previous_existed {
                fs::rename(&target.target, &target.previous).map_err(|source| {
                    io_error("retain pre-restore target", &target.target, source)
                })?;
            }
        }
        fs::rename(staged_database, &journal.database_path).map_err(|source| {
            io_error("install restored database", &journal.database_path, source)
        })?;
        fs::rename(staged_modes, &journal.modes_path)
            .map_err(|source| io_error("install restored modes", &journal.modes_path, source))?;
        Ok::<(), BackupError>(())
    })();
    if let Err(error) = commit {
        return match rollback_restore(journal) {
            Ok(recovery) => {
                if !recovery.preserved.is_empty() {
                    tracing::warn!(
                        paths = ?recovery.preserved,
                        "preserved interrupted restore targets during rollback"
                    );
                }
                fs::remove_file(journal_path).map_err(|source| {
                    io_error("remove rolled-back restore journal", journal_path, source)
                })?;
                Err(error)
            }
            Err(_) => Err(BackupError::RestoreRollbackFailed(
                journal_path.to_path_buf(),
            )),
        };
    }
    if let Err(source) = fs::remove_file(journal_path) {
        return match rollback_restore(journal) {
            Ok(_) => match fs::remove_file(journal_path) {
                Ok(()) => Err(io_error(
                    "remove completed restore journal",
                    journal_path,
                    source,
                )),
                Err(_) => Err(BackupError::RestoreRollbackFailed(
                    journal_path.to_path_buf(),
                )),
            },
            Err(_) => Err(BackupError::RestoreRollbackFailed(
                journal_path.to_path_buf(),
            )),
        };
    }

    Ok(previous_restore_state(journal))
}

#[derive(Debug)]
struct RollbackOutcome {
    recovered: Vec<PathBuf>,
    preserved: Vec<PathBuf>,
}

fn rollback_restore(journal: &RestoreJournal) -> Result<RollbackOutcome, BackupError> {
    let mut recovered = Vec::new();
    let mut preserved = Vec::new();
    for target in journal.targets.iter().rev() {
        let previous_exists = path_try_exists(&target.previous, "inspect pre-restore target")?;
        let target_exists = path_try_exists(&target.target, "inspect current restore target")?;
        if target.previous_existed {
            if previous_exists {
                if target_exists {
                    let interrupted = sibling_with_suffix(
                        &target.target,
                        &format!(".interrupted-restore-{}", journal.operation_id),
                    )?;
                    if path_try_exists(&interrupted, "inspect interrupted restore target")? {
                        return Err(BackupError::InvalidArchive(
                            "interrupted restore retention target already exists",
                        ));
                    }
                    fs::rename(&target.target, &interrupted).map_err(|source| {
                        io_error(
                            "preserve interrupted restore target",
                            &target.target,
                            source,
                        )
                    })?;
                    preserved.push(interrupted);
                }
                fs::rename(&target.previous, &target.target).map_err(|source| {
                    io_error("recover pre-restore target", &target.previous, source)
                })?;
                recovered.push(target.target.clone());
            } else if !target_exists {
                return Err(BackupError::InvalidArchive(
                    "both current and retained restore targets are missing",
                ));
            }
        } else if target_exists {
            let interrupted = sibling_with_suffix(
                &target.target,
                &format!(".interrupted-restore-{}", journal.operation_id),
            )?;
            if path_try_exists(&interrupted, "inspect interrupted restore target")? {
                return Err(BackupError::InvalidArchive(
                    "interrupted restore retention target already exists",
                ));
            }
            fs::rename(&target.target, &interrupted).map_err(|source| {
                io_error(
                    "preserve interrupted restore target",
                    &target.target,
                    source,
                )
            })?;
            preserved.push(interrupted);
        }
    }
    Ok(RollbackOutcome {
        recovered,
        preserved,
    })
}

fn previous_restore_state(journal: &RestoreJournal) -> PreviousRestoreState {
    let previous = |index: usize| {
        journal
            .targets
            .get(index)
            .filter(|target| target.previous_existed)
            .map(|target| target.previous.clone())
    };
    PreviousRestoreState {
        database: previous(0),
        sidecars: [previous(1), previous(2)].into_iter().flatten().collect(),
        modes: previous(3),
    }
}

fn path_try_exists(path: &Path, operation: &'static str) -> Result<bool, BackupError> {
    path.try_exists()
        .map_err(|source| io_error(operation, path, source))
}

struct BackupResponseContract;

impl PartialSchema for BackupResponseContract {
    fn schema() -> RefOr<Schema> {
        Schema::Object(
            ObjectBuilder::new()
                .schema_type(SchemaType::AnyValue)
                .build(),
        )
        .into()
    }
}

impl ToSchema for BackupResponseContract {}

pub(crate) fn admin_router() -> OpenApiRouter<HttpState> {
    OpenApiRouter::default().routes(routes!(download_backup))
}

#[utoipa::path(
    get,
    path = "/admin/backup",
    responses((status = 200, description = "Successful Response", body = inline(BackupResponseContract))),
    tag = "admin"
)]
async fn download_backup(
    State(state): State<HttpState>,
    headers: HeaderMap,
) -> Result<Response<Body>, ApiError> {
    let _current = current_session(&state, &headers, SessionTouch::UpdateLastSeen).await?;
    let service = state
        .backup
        .as_ref()
        .ok_or_else(ApiError::service_unavailable)?;
    let prepared = service.prepare().await.map_err(|error| {
        tracing::error!(error = %error, "state backup failed");
        ApiError::internal()
    })?;
    stream_backup(prepared).await
}

async fn stream_backup(prepared: PreparedBackup) -> Result<Response<Body>, ApiError> {
    let file = tokio::fs::File::open(&prepared.archive_path)
        .await
        .map_err(|error| {
            tracing::error!(error = %error, "completed backup could not be opened");
            ApiError::internal()
        })?;
    let content_disposition = HeaderValue::from_str(&format!(
        "attachment; filename=\"{}\"",
        prepared.download_name
    ))
    .map_err(|_| ApiError::internal())?;
    let content_length = HeaderValue::from_str(&prepared.archive_bytes.to_string())
        .map_err(|_| ApiError::internal())?;
    let stream = stream::try_unfold(
        BackupStreamState {
            file,
            _workspace: prepared._workspace,
        },
        |mut state| async move {
            let mut buffer = vec![0_u8; ARCHIVE_CHUNK_BYTES];
            let read = state.file.read(&mut buffer).await?;
            if read == 0 {
                Ok::<_, io::Error>(None)
            } else {
                buffer.truncate(read);
                Ok(Some((buffer, state)))
            }
        },
    );
    let mut response = Response::new(Body::from_stream(stream));
    response
        .headers_mut()
        .insert(CONTENT_TYPE, HeaderValue::from_static("application/gzip"));
    response
        .headers_mut()
        .insert(CONTENT_DISPOSITION, content_disposition);
    response
        .headers_mut()
        .insert(CONTENT_LENGTH, content_length);
    response
        .headers_mut()
        .insert(CACHE_CONTROL, HeaderValue::from_static("no-store"));
    Ok(response)
}

struct BackupStreamState {
    file: tokio::fs::File,
    _workspace: TempDir,
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::error::Error;
    use std::fs;
    use std::sync::Arc;

    use base64::Engine;
    use base64::engine::general_purpose::URL_SAFE;
    use music_storage::{SchemaCompatibility, SqliteStorage, SqliteStorageOptions};
    use tempfile::tempdir;
    use uuid::Uuid;

    use super::{
        BACKUP_FORMAT_VERSION, BACKUP_KIND, BackupError, BackupService, MaintenanceGate,
        RestoreOptions, build_restore_journal, recover_interrupted_restore, restore_backup,
        restore_journal_path, verify_archive, write_restore_journal,
    };
    use crate::config::AppConfig;

    #[tokio::test]
    async fn backup_is_verified_and_contains_only_a_credential_key_fingerprint()
    -> Result<(), Box<dyn Error>> {
        let directory = tempdir()?;
        let modes = directory.path().join("modes");
        fs::create_dir_all(modes.join("table/presets"))?;
        fs::write(modes.join("table/mode.yaml"), "name: Table\n")?;
        fs::write(modes.join("table/presets/calm.yaml"), "gain: -2\n")?;
        let key = URL_SAFE.encode([7_u8; 32]);
        let values = BTreeMap::from([
            (
                "DATABASE_URL".to_owned(),
                format!("sqlite:///{}", directory.path().join("app.db").display()),
            ),
            (
                "MUSIC_DIR".to_owned(),
                directory.path().join("music").display().to_string(),
            ),
            (
                "SFX_LIBRARY_DIR".to_owned(),
                directory.path().join("sfx").display().to_string(),
            ),
            ("MODES_DIR".to_owned(), modes.display().to_string()),
            ("ASSISTANT_CREDENTIAL_KEY".to_owned(), key.clone()),
        ]);
        let config = Arc::new(AppConfig::from_values(&values)?);
        let storage =
            Arc::new(SqliteStorage::open(SqliteStorageOptions::new(&config.database_path)).await?);
        let service = BackupService::new(
            Arc::clone(&storage),
            Arc::clone(&config),
            MaintenanceGate::default(),
        );

        let prepared = service.prepare().await?;
        let manifest = verify_archive(&prepared.archive_path)?;

        assert_eq!(manifest.kind, BACKUP_KIND);
        assert_eq!(manifest.format_version, BACKUP_FORMAT_VERSION);
        assert_eq!(manifest.database.path, "app.db");
        assert_eq!(
            manifest.credential_key_id.as_deref(),
            Some("4bb06f8e4e3a7715")
        );
        assert_eq!(manifest.modes.len(), 2);
        assert!(!serde_json::to_string(&manifest)?.contains(&key));
        assert!(prepared.archive_bytes > 0);
        storage.close().await;
        Ok(())
    }

    #[tokio::test]
    async fn offline_restore_requires_confirmation_and_retains_replaced_state()
    -> Result<(), Box<dyn Error>> {
        let source = tempdir()?;
        let source_modes = source.path().join("modes");
        fs::create_dir_all(source_modes.join("table/presets"))?;
        fs::write(source_modes.join("table/mode.yaml"), "name: Restored\n")?;
        fs::write(source_modes.join("table/presets/calm.yaml"), "gain: -2\n")?;
        let key = URL_SAFE.encode([7_u8; 32]);
        let values = BTreeMap::from([
            (
                "DATABASE_URL".to_owned(),
                format!("sqlite:///{}", source.path().join("app.db").display()),
            ),
            (
                "MUSIC_DIR".to_owned(),
                source.path().join("music").display().to_string(),
            ),
            (
                "SFX_LIBRARY_DIR".to_owned(),
                source.path().join("sfx").display().to_string(),
            ),
            ("MODES_DIR".to_owned(), source_modes.display().to_string()),
            ("ASSISTANT_CREDENTIAL_KEY".to_owned(), key),
        ]);
        let config = Arc::new(AppConfig::from_values(&values)?);
        let storage =
            Arc::new(SqliteStorage::open(SqliteStorageOptions::new(&config.database_path)).await?);
        let service = BackupService::new(
            Arc::clone(&storage),
            Arc::clone(&config),
            MaintenanceGate::default(),
        );
        let prepared = service.prepare().await?;

        let target = tempdir()?;
        let target_database = target.path().join("app.db");
        let target_modes = target.path().join("modes");
        fs::write(&target_database, b"previous database")?;
        fs::create_dir(&target_modes)?;
        fs::write(target_modes.join("previous.yaml"), "name: Previous\n")?;
        let options = RestoreOptions {
            archive_path: prepared.archive_path.clone(),
            database_path: target_database.clone(),
            modes_path: target_modes.clone(),
            replace: true,
            server_stopped: false,
        };
        assert!(matches!(
            restore_backup(&config, options.clone()).await,
            Err(BackupError::RestoreConfirmationRequired)
        ));
        let mut mismatched_values = values.clone();
        mismatched_values.insert(
            "ASSISTANT_CREDENTIAL_KEY".to_owned(),
            URL_SAFE.encode([8_u8; 32]),
        );
        let mismatched_config = AppConfig::from_values(&mismatched_values)?;
        assert!(matches!(
            restore_backup(
                &mismatched_config,
                RestoreOptions {
                    server_stopped: true,
                    ..options.clone()
                }
            )
            .await,
            Err(BackupError::CredentialKeyMismatch)
        ));

        let outcome = restore_backup(
            &config,
            RestoreOptions {
                server_stopped: true,
                ..options
            },
        )
        .await?;

        assert_eq!(outcome.restored_mode_files, 2);
        assert_eq!(
            fs::read_to_string(target_modes.join("table/mode.yaml"))?,
            "name: Restored\n"
        );
        let report = SqliteStorage::doctor(&target_database).await?;
        assert_eq!(report.compatibility, SchemaCompatibility::Current);
        let previous_database = outcome
            .previous_database_path
            .ok_or("previous database was not retained")?;
        let previous_modes = outcome
            .previous_modes_path
            .ok_or("previous modes were not retained")?;
        assert_eq!(fs::read(previous_database)?, b"previous database");
        assert_eq!(
            fs::read_to_string(previous_modes.join("previous.yaml"))?,
            "name: Previous\n"
        );
        assert!(!restore_journal_path(&target_database)?.try_exists()?);
        storage.close().await;
        Ok(())
    }

    #[tokio::test]
    async fn backup_refuses_a_configured_master_key_inside_modes() -> Result<(), Box<dyn Error>> {
        let directory = tempdir()?;
        let modes = directory.path().join("modes");
        fs::create_dir(&modes)?;
        let key_path = modes.join("assistant-credential.key");
        fs::write(&key_path, URL_SAFE.encode([9_u8; 32]))?;
        let values = BTreeMap::from([
            (
                "DATABASE_URL".to_owned(),
                format!("sqlite:///{}", directory.path().join("app.db").display()),
            ),
            (
                "MUSIC_DIR".to_owned(),
                directory.path().join("music").display().to_string(),
            ),
            (
                "SFX_LIBRARY_DIR".to_owned(),
                directory.path().join("sfx").display().to_string(),
            ),
            ("MODES_DIR".to_owned(), modes.display().to_string()),
            (
                "ASSISTANT_CREDENTIAL_KEY_FILE".to_owned(),
                key_path.display().to_string(),
            ),
        ]);
        let config = Arc::new(AppConfig::from_values(&values)?);
        let storage =
            Arc::new(SqliteStorage::open(SqliteStorageOptions::new(&config.database_path)).await?);
        let service = BackupService::new(Arc::clone(&storage), config, MaintenanceGate::default());

        assert!(matches!(
            service.prepare().await,
            Err(BackupError::InvalidArchive(
                "credential key file must not be inside the modes directory"
            ))
        ));
        storage.close().await;
        Ok(())
    }

    #[test]
    fn interrupted_restore_recovery_preserves_new_targets_and_restores_old_targets()
    -> Result<(), Box<dyn Error>> {
        let directory = tempdir()?;
        let database = fs::canonicalize(directory.path())?.join("app.db");
        let modes = fs::canonicalize(directory.path())?.join("modes");
        fs::write(&database, b"old database")?;
        fs::create_dir(&modes)?;
        fs::write(modes.join("old.yaml"), "old\n")?;
        let operation_id = Uuid::new_v4().to_string();
        let journal = build_restore_journal(&operation_id, &database, &modes)?;
        let journal_path = restore_journal_path(&database)?;
        write_restore_journal(&journal_path, &journal)?;
        fs::rename(&journal.targets[0].target, &journal.targets[0].previous)?;
        fs::rename(&journal.targets[3].target, &journal.targets[3].previous)?;
        fs::write(&database, b"new database")?;
        fs::create_dir(&modes)?;
        fs::write(modes.join("new.yaml"), "new\n")?;

        let outcome = recover_interrupted_restore(&database, &modes, true)?;

        assert_eq!(fs::read(&database)?, b"old database");
        assert_eq!(fs::read_to_string(modes.join("old.yaml"))?, "old\n");
        assert_eq!(outcome.recovered_targets.len(), 2);
        assert_eq!(outcome.preserved_interrupted_targets.len(), 2);
        assert!(!journal_path.try_exists()?);
        Ok(())
    }
}
