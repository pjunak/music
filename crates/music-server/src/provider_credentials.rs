use std::fmt::{self, Display, Formatter};
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE;
use music_application::assistant::{
    ProviderCredentialCipher, ProviderCredentialError, ProviderCredentialFuture,
    ProviderCredentialResetOutcome, ProviderCredentialSource, ProviderRepository,
};
use music_storage::{CredentialVault, SecretString};
use rand::TryRngCore;
use tokio::sync::RwLock;
use zeroize::{Zeroize, Zeroizing};

use crate::config::AppConfig;

const MASTER_KEY_BYTES: usize = 32;
const MAX_ENCODED_KEY_BYTES: u64 = 128;

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub(crate) enum CredentialStorageSource {
    Environment,
    File,
}

#[derive(Debug, Clone, Eq, PartialEq)]
pub(crate) struct CredentialStorageStatus {
    pub(crate) ready: bool,
    pub(crate) error: Option<String>,
    pub(crate) source: Option<CredentialStorageSource>,
    pub(crate) key_id: Option<String>,
    pub(crate) key_file_path: Option<String>,
    pub(crate) host_directory_hint: Option<String>,
    pub(crate) can_initialize: bool,
    pub(crate) initialization_error: Option<String>,
}

#[derive(Debug, Clone, Eq, PartialEq)]
pub(crate) struct CredentialStorageReset {
    pub(crate) deleted_credentials: u64,
    pub(crate) master_key_removed: bool,
    pub(crate) master_key_removal_error: Option<String>,
    pub(crate) status: CredentialStorageStatus,
}

#[derive(Debug)]
pub(crate) struct CredentialStoreError {
    code: &'static str,
    source: Option<io::Error>,
}

impl CredentialStoreError {
    const fn public(code: &'static str) -> Self {
        Self { code, source: None }
    }

    fn io(code: &'static str, source: io::Error) -> Self {
        Self {
            code,
            source: Some(source),
        }
    }

    pub(crate) const fn code(&self) -> &'static str {
        self.code
    }
}

impl Display for CredentialStoreError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.code)
    }
}

impl std::error::Error for CredentialStoreError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        self.source
            .as_ref()
            .map(|source| source as &(dyn std::error::Error + 'static))
    }
}

#[derive(Debug)]
pub(crate) struct RuntimeCredentialStore {
    environment_key: Option<SecretString>,
    key_file: Option<PathBuf>,
    host_directory_hint: Option<String>,
    lifecycle: RwLock<()>,
}

impl RuntimeCredentialStore {
    pub(crate) fn new(config: &AppConfig) -> Self {
        Self {
            environment_key: config
                .assistant_credential_key
                .as_ref()
                .map(|key| SecretString::new(key.expose_secret())),
            key_file: config.assistant_credential_key_file.clone(),
            host_directory_hint: config.assistant_credential_host_directory_hint.clone(),
            lifecycle: RwLock::new(()),
        }
    }

    pub(crate) async fn status(&self, saved_credentials_exist: bool) -> CredentialStorageStatus {
        let _lifecycle = self.lifecycle.read().await;
        self.status_unlocked(saved_credentials_exist)
    }

    pub(crate) async fn initialize(
        &self,
        saved_credentials_exist: bool,
    ) -> Result<CredentialStorageStatus, CredentialStoreError> {
        let _lifecycle = self.lifecycle.write().await;
        let current = self.status_unlocked(saved_credentials_exist);
        if !current.can_initialize {
            return Err(CredentialStoreError::public(initialization_code(&current)));
        }
        let key_file = self
            .key_file
            .as_deref()
            .ok_or_else(|| CredentialStoreError::public("master_key_file_not_configured"))?;
        let mut key = [0_u8; MASTER_KEY_BYTES];
        if let Err(source) = rand::rngs::OsRng.try_fill_bytes(&mut key) {
            key.zeroize();
            return Err(CredentialStoreError::io(
                "master_key_generation_failed",
                io::Error::other(source),
            ));
        }
        let encoded = Zeroizing::new(URL_SAFE.encode(key));
        key.zeroize();
        write_new_key_file(key_file, encoded.as_bytes())?;
        let initialized = self.status_unlocked(false);
        if !initialized.ready {
            return Err(CredentialStoreError::public(
                initialized
                    .error
                    .as_deref()
                    .and_then(stable_error_code)
                    .unwrap_or("master_key_initialization_failed"),
            ));
        }
        Ok(initialized)
    }

    pub(crate) async fn reset(
        &self,
        repository: &dyn ProviderRepository,
    ) -> Result<CredentialStorageReset, CredentialStoreError> {
        let _lifecycle = self.lifecycle.write().await;
        let target = self.preflight_key_removal()?;
        let deleted_credentials =
            match repository
                .reset_provider_credentials()
                .await
                .map_err(|source| {
                    CredentialStoreError::io(
                        "credential_reset_storage_failed",
                        io::Error::other(source),
                    )
                })? {
                ProviderCredentialResetOutcome::Applied {
                    deleted_credentials,
                } => deleted_credentials,
                ProviderCredentialResetOutcome::ModelJobActive => {
                    return Err(CredentialStoreError::public("model_job_active"));
                }
            };
        let removal_error = remove_preflighted_key(&target).err();
        let master_key_removal_error = removal_error.as_ref().map(|error| error.code().to_owned());
        let status = self.status_unlocked(false);
        Ok(CredentialStorageReset {
            deleted_credentials,
            master_key_removed: removal_error.is_none(),
            master_key_removal_error,
            status,
        })
    }

    fn status_unlocked(&self, saved_credentials_exist: bool) -> CredentialStorageStatus {
        let key_file_path = self
            .key_file
            .as_ref()
            .map(|path| path.display().to_string());
        let environment_key = self.environment_key_value();
        let file_exists = self
            .key_file
            .as_deref()
            .is_some_and(path_exists_without_following);
        let mut source = if environment_key.is_some() {
            Some(CredentialStorageSource::Environment)
        } else if file_exists {
            Some(CredentialStorageSource::File)
        } else {
            None
        };
        let (ready, error, key_id) = match self.configured_vault_unlocked() {
            Ok((vault, resolved_source)) => {
                source = Some(resolved_source);
                (true, None, Some(vault.key_id().to_owned()))
            }
            Err(error) => (false, Some(error.code().to_owned()), None),
        };
        let initialization_error = if ready {
            Some("master_key_already_configured".to_owned())
        } else if environment_key.is_some() {
            Some("master_key_managed_by_environment".to_owned())
        } else if self.key_file.is_none() {
            Some("master_key_file_not_configured".to_owned())
        } else if file_exists {
            Some("master_key_file_exists".to_owned())
        } else if saved_credentials_exist {
            Some("saved_credentials_require_existing_key".to_owned())
        } else {
            self.key_file
                .as_deref()
                .and_then(initialization_target_error)
                .map(str::to_owned)
        };
        CredentialStorageStatus {
            ready,
            error,
            source,
            key_id,
            key_file_path,
            host_directory_hint: self.host_directory_hint.clone(),
            can_initialize: initialization_error.is_none(),
            initialization_error,
        }
    }

    fn configured_vault_unlocked(
        &self,
    ) -> Result<(CredentialVault, CredentialStorageSource), CredentialStoreError> {
        if let Some(encoded) = self.environment_key_value() {
            return CredentialVault::from_encoded_key(encoded)
                .map(|vault| (vault, CredentialStorageSource::Environment))
                .map_err(|_| CredentialStoreError::public("invalid_master_key"));
        }
        let path = self
            .key_file
            .as_deref()
            .ok_or_else(|| CredentialStoreError::public("master_key_not_configured"))?;
        let encoded = read_key_file(path)?;
        CredentialVault::from_encoded_key(&encoded)
            .map(|vault| (vault, CredentialStorageSource::File))
            .map_err(|_| CredentialStoreError::public("invalid_master_key"))
    }

    fn environment_key_value(&self) -> Option<&str> {
        self.environment_key
            .as_ref()
            .map(SecretString::expose_secret)
            .map(str::trim)
            .filter(|value| !value.is_empty())
    }

    fn preflight_key_removal(&self) -> Result<KeyRemovalTarget, CredentialStoreError> {
        if self.environment_key_value().is_some() {
            return Err(CredentialStoreError::public(
                "master_key_managed_by_environment",
            ));
        }
        let path = self
            .key_file
            .as_ref()
            .ok_or_else(|| CredentialStoreError::public("master_key_file_not_configured"))?;
        if !path.is_absolute() {
            return Err(CredentialStoreError::public(
                "master_key_file_path_not_absolute",
            ));
        }
        let identity = match fs::symlink_metadata(path) {
            Ok(metadata) => {
                if metadata.file_type().is_symlink() || !metadata.is_file() {
                    return Err(CredentialStoreError::public("master_key_file_unsafe"));
                }
                Some(FileIdentity::from_metadata(&metadata))
            }
            Err(source) if source.kind() == io::ErrorKind::NotFound => None,
            Err(source) => {
                return Err(CredentialStoreError::io(
                    "master_key_file_unreadable",
                    source,
                ));
            }
        };
        validate_parent(path)?;
        Ok(KeyRemovalTarget {
            path: path.clone(),
            identity,
        })
    }
}

impl ProviderCredentialSource for RuntimeCredentialStore {
    fn current_cipher(&self) -> ProviderCredentialFuture<'_> {
        Box::pin(async move {
            let _lifecycle = self.lifecycle.read().await;
            let (vault, _) = self
                .configured_vault_unlocked()
                .map_err(provider_credential_error)?;
            Ok(Arc::new(vault) as Arc<dyn ProviderCredentialCipher>)
        })
    }
}

fn provider_credential_error(error: CredentialStoreError) -> ProviderCredentialError {
    ProviderCredentialError {
        code: error.code().to_owned(),
    }
}

fn initialization_code(status: &CredentialStorageStatus) -> &'static str {
    status
        .initialization_error
        .as_deref()
        .and_then(stable_error_code)
        .unwrap_or("master_key_initialization_unavailable")
}

fn stable_error_code(code: &str) -> Option<&'static str> {
    Some(match code {
        "master_key_not_configured" => "master_key_not_configured",
        "invalid_master_key" => "invalid_master_key",
        "master_key_file_unreadable" => "master_key_file_unreadable",
        "master_key_file_unsafe" => "master_key_file_unsafe",
        "master_key_file_permissions" => "master_key_file_permissions",
        "master_key_already_configured" => "master_key_already_configured",
        "master_key_file_exists" => "master_key_file_exists",
        "master_key_managed_by_environment" => "master_key_managed_by_environment",
        "saved_credentials_require_existing_key" => "saved_credentials_require_existing_key",
        "master_key_file_not_configured" => "master_key_file_not_configured",
        "master_key_file_path_not_absolute" => "master_key_file_path_not_absolute",
        "master_key_directory_unavailable" => "master_key_directory_unavailable",
        "master_key_directory_unsafe" => "master_key_directory_unsafe",
        "master_key_directory_permissions" => "master_key_directory_permissions",
        "master_key_directory_not_writable" => "master_key_directory_not_writable",
        "master_key_file_write_failed" => "master_key_file_write_failed",
        "master_key_initialization_failed" => "master_key_initialization_failed",
        "master_key_generation_failed" => "master_key_generation_failed",
        "master_key_file_delete_failed" => "master_key_file_delete_failed",
        "master_key_storage_changed" => "master_key_storage_changed",
        "model_job_active" => "model_job_active",
        "credential_reset_storage_failed" => "credential_reset_storage_failed",
        _ => return None,
    })
}

fn read_key_file(path: &Path) -> Result<Zeroizing<String>, CredentialStoreError> {
    let metadata = fs::symlink_metadata(path).map_err(|source| {
        if source.kind() == io::ErrorKind::NotFound {
            CredentialStoreError::io("master_key_not_configured", source)
        } else {
            CredentialStoreError::io("master_key_file_unreadable", source)
        }
    })?;
    validate_key_metadata(&metadata)?;
    let file = File::open(path)
        .map_err(|source| CredentialStoreError::io("master_key_file_unreadable", source))?;
    validate_key_metadata(
        &file
            .metadata()
            .map_err(|source| CredentialStoreError::io("master_key_file_unreadable", source))?,
    )?;
    let mut bytes = Zeroizing::new(Vec::new());
    file.take(MAX_ENCODED_KEY_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|source| CredentialStoreError::io("master_key_file_unreadable", source))?;
    if bytes.len() as u64 > MAX_ENCODED_KEY_BYTES {
        return Err(CredentialStoreError::public("invalid_master_key"));
    }
    let encoded = String::from_utf8(bytes.to_vec())
        .map_err(|_| CredentialStoreError::public("invalid_master_key"))?;
    if !encoded.is_ascii() {
        return Err(CredentialStoreError::public("invalid_master_key"));
    }
    Ok(Zeroizing::new(encoded.trim().to_owned()))
}

fn validate_key_metadata(metadata: &fs::Metadata) -> Result<(), CredentialStoreError> {
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        return Err(CredentialStoreError::public("master_key_file_unsafe"));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if metadata.permissions().mode() & 0o077 != 0 {
            return Err(CredentialStoreError::public("master_key_file_permissions"));
        }
    }
    Ok(())
}

fn initialization_target_error(path: &Path) -> Option<&'static str> {
    if !path.is_absolute() {
        return Some("master_key_file_path_not_absolute");
    }
    if path_exists_without_following(path) {
        return Some("master_key_file_exists");
    }
    validate_parent(path).err().map(|error| error.code())
}

fn validate_parent(path: &Path) -> Result<(), CredentialStoreError> {
    let parent = path
        .parent()
        .ok_or_else(|| CredentialStoreError::public("master_key_directory_unavailable"))?;
    let metadata = fs::symlink_metadata(parent)
        .map_err(|source| CredentialStoreError::io("master_key_directory_unavailable", source))?;
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return Err(CredentialStoreError::public("master_key_directory_unsafe"));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if metadata.permissions().mode() & 0o077 != 0 {
            return Err(CredentialStoreError::public(
                "master_key_directory_permissions",
            ));
        }
    }
    Ok(())
}

fn write_new_key_file(path: &Path, encoded: &[u8]) -> Result<(), CredentialStoreError> {
    if !path.is_absolute() {
        return Err(CredentialStoreError::public(
            "master_key_file_path_not_absolute",
        ));
    }
    validate_parent(path)?;
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(path).map_err(|source| {
        if source.kind() == io::ErrorKind::AlreadyExists {
            CredentialStoreError::io("master_key_file_exists", source)
        } else {
            CredentialStoreError::io("master_key_file_write_failed", source)
        }
    })?;
    let result = file.write_all(encoded).and_then(|()| file.sync_all());
    if let Err(source) = result {
        drop(file);
        let _ignored = fs::remove_file(path);
        return Err(CredentialStoreError::io(
            "master_key_file_write_failed",
            source,
        ));
    }
    Ok(())
}

fn path_exists_without_following(path: &Path) -> bool {
    match fs::symlink_metadata(path) {
        Ok(_) => true,
        Err(error) => error.kind() != io::ErrorKind::NotFound,
    }
}

#[derive(Debug)]
struct KeyRemovalTarget {
    path: PathBuf,
    identity: Option<FileIdentity>,
}

#[derive(Debug, Clone, Eq, PartialEq)]
struct FileIdentity {
    length: u64,
    modified: Option<std::time::SystemTime>,
    created: Option<std::time::SystemTime>,
}

impl FileIdentity {
    fn from_metadata(metadata: &fs::Metadata) -> Self {
        Self {
            length: metadata.len(),
            modified: metadata.modified().ok(),
            created: metadata.created().ok(),
        }
    }
}

fn remove_preflighted_key(target: &KeyRemovalTarget) -> Result<bool, CredentialStoreError> {
    let metadata = match fs::symlink_metadata(&target.path) {
        Ok(metadata) => metadata,
        Err(source) if source.kind() == io::ErrorKind::NotFound => return Ok(false),
        Err(source) => {
            return Err(CredentialStoreError::io(
                "master_key_file_delete_failed",
                source,
            ));
        }
    };
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        return Err(CredentialStoreError::public("master_key_file_unsafe"));
    }
    if target.identity.as_ref() != Some(&FileIdentity::from_metadata(&metadata)) {
        return Err(CredentialStoreError::public("master_key_storage_changed"));
    }
    fs::remove_file(&target.path)
        .map_err(|source| CredentialStoreError::io("master_key_file_delete_failed", source))?;
    Ok(true)
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::error::Error;

    use music_application::assistant::{ProviderCredentialSource, ProviderRepository};
    use music_storage::{SqliteStorage, SqliteStorageOptions};

    use super::*;

    fn config(root: &Path) -> Result<AppConfig, crate::ConfigError> {
        AppConfig::from_values(&BTreeMap::from([
            (
                "DATABASE_URL".to_owned(),
                format!("sqlite:///{}", root.join("app.db").display()),
            ),
            (
                "ASSISTANT_CREDENTIAL_KEY_FILE".to_owned(),
                root.join("credentials/provider.key").display().to_string(),
            ),
        ]))
    }

    #[tokio::test]
    async fn initializes_reads_and_resets_a_fixed_file_backed_key()
    -> Result<(), Box<dyn Error + Send + Sync>> {
        let directory = tempfile::tempdir()?;
        let key_parent = directory.path().join("credentials");
        fs::create_dir(&key_parent)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&key_parent, fs::Permissions::from_mode(0o700))?;
        }
        let config = config(directory.path())?;
        let store = RuntimeCredentialStore::new(&config);
        let initial = store.status(false).await;
        assert!(!initial.ready);
        assert!(initial.can_initialize);
        let initialized = store.initialize(false).await?;
        assert!(initialized.ready);
        assert_eq!(initialized.source, Some(CredentialStorageSource::File));
        assert_eq!(initialized.key_id.as_deref().map(str::len), Some(16));
        assert!(store.current_cipher().await.is_ok());

        let storage = SqliteStorage::open(SqliteStorageOptions::new(&config.database_path)).await?;
        let repository: &dyn ProviderRepository = &storage;
        let reset = store.reset(repository).await?;
        assert_eq!(reset.deleted_credentials, 0);
        assert!(reset.master_key_removed);
        assert!(!reset.status.ready);
        assert!(reset.status.can_initialize);
        Ok(())
    }

    #[tokio::test]
    async fn refuses_initialization_when_ciphertext_survives_without_a_key()
    -> Result<(), Box<dyn Error + Send + Sync>> {
        let directory = tempfile::tempdir()?;
        let key_parent = directory.path().join("credentials");
        fs::create_dir(&key_parent)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&key_parent, fs::Permissions::from_mode(0o700))?;
        }
        let store = RuntimeCredentialStore::new(&config(directory.path())?);
        let status = store.status(true).await;
        assert!(!status.can_initialize);
        assert_eq!(
            status.initialization_error.as_deref(),
            Some("saved_credentials_require_existing_key")
        );
        assert_eq!(
            store.initialize(true).await.err().map(|error| error.code()),
            Some("saved_credentials_require_existing_key")
        );
        Ok(())
    }
}
