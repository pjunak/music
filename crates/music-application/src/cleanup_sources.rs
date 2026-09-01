use std::error::Error;
use std::fmt::{self, Display, Formatter};
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use crate::assistant::{EncryptedProviderCredential, ProviderCredentialSource, ProviderSecret};

pub const MUSICBRAINZ_SOURCE_ID: &str = "musicbrainz";
pub const ACOUSTID_SOURCE_ID: &str = "acoustid";
pub const LASTFM_SOURCE_ID: &str = "lastfm";
const CLEANUP_CREDENTIAL_SCOPE_PREFIX: &str = "cleanup-source:";
const MAX_CREDENTIAL_CHARS: usize = 4_096;

pub type CleanupSourceDependencyError = Box<dyn Error + Send + Sync>;
pub type CleanupSourceFuture<'a, T> =
    Pin<Box<dyn Future<Output = Result<T, CleanupSourceDependencyError>> + Send + 'a>>;

pub trait CleanupSourceRepository: Send + Sync {
    fn cleanup_source_enabled(&self, source_id: &str) -> CleanupSourceFuture<'_, Option<bool>>;

    fn set_cleanup_source_enabled(
        &self,
        source_id: &str,
        enabled: bool,
    ) -> CleanupSourceFuture<'_, ()>;

    fn cleanup_source_credential(
        &self,
        source_id: &str,
    ) -> CleanupSourceFuture<'_, Option<EncryptedProviderCredential>>;

    fn store_cleanup_source_credential<'a>(
        &'a self,
        source_id: &'a str,
        credential: &'a EncryptedProviderCredential,
    ) -> CleanupSourceFuture<'a, ()>;

    fn clear_cleanup_source_credential(&self, source_id: &str) -> CleanupSourceFuture<'_, bool>;
}

#[derive(Debug, Clone, Eq, PartialEq)]
pub struct CleanupSource {
    pub id: String,
    pub label: String,
    pub description: String,
    pub enabled: bool,
    pub capabilities: Vec<String>,
    pub credential_kind: Option<String>,
    pub credential_saved: bool,
    pub credential_source: Option<String>,
    pub key_hint: Option<String>,
    pub configured: bool,
    pub available: bool,
    pub configuration_hint: Option<String>,
    pub unavailable_reason: Option<String>,
}

#[derive(Debug, Clone, Default, Eq, PartialEq)]
pub struct CleanupSourceRuntime {
    pub acoustid_configured: bool,
    pub fpcalc_available: bool,
    pub lastfm_configured: bool,
}

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum CleanupSourceError {
    UnknownSource,
    InvalidCredential,
    CredentialStorage,
    Dependency,
}

impl Display for CleanupSourceError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::UnknownSource => "cleanup source is not recognized",
            Self::InvalidCredential => "cleanup source credential is invalid",
            Self::CredentialStorage => "encrypted credential storage is unavailable",
            Self::Dependency => "cleanup source settings are unavailable",
        })
    }
}

impl Error for CleanupSourceError {}

#[derive(Clone)]
pub struct CleanupSourceService {
    repository: Arc<dyn CleanupSourceRepository>,
    credentials: Arc<dyn ProviderCredentialSource>,
    runtime: CleanupSourceRuntime,
}

impl fmt::Debug for CleanupSourceService {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("CleanupSourceService")
            .finish_non_exhaustive()
    }
}

impl CleanupSourceService {
    #[must_use]
    pub fn new(
        repository: Arc<dyn CleanupSourceRepository>,
        credentials: Arc<dyn ProviderCredentialSource>,
        runtime: CleanupSourceRuntime,
    ) -> Self {
        Self {
            repository,
            credentials,
            runtime,
        }
    }

    pub async fn sources(&self) -> Result<Vec<CleanupSource>, CleanupSourceError> {
        Ok(vec![
            self.musicbrainz().await?,
            self.acoustid().await?,
            self.lastfm().await?,
        ])
    }

    pub async fn musicbrainz_enabled(&self) -> Result<bool, CleanupSourceError> {
        self.repository
            .cleanup_source_enabled(MUSICBRAINZ_SOURCE_ID)
            .await
            .map(|stored| stored.unwrap_or(true))
            .map_err(|_| CleanupSourceError::Dependency)
    }

    pub async fn update(
        &self,
        source_id: &str,
        enabled: bool,
    ) -> Result<CleanupSource, CleanupSourceError> {
        if !matches!(
            source_id,
            MUSICBRAINZ_SOURCE_ID | ACOUSTID_SOURCE_ID | LASTFM_SOURCE_ID
        ) {
            return Err(CleanupSourceError::UnknownSource);
        }
        self.repository
            .set_cleanup_source_enabled(source_id, enabled)
            .await
            .map_err(|_| CleanupSourceError::Dependency)?;
        self.source(source_id, enabled).await
    }

    pub async fn enabled(&self, source_id: &str) -> Result<bool, CleanupSourceError> {
        let default = match source_id {
            MUSICBRAINZ_SOURCE_ID => true,
            ACOUSTID_SOURCE_ID | LASTFM_SOURCE_ID => false,
            _ => return Err(CleanupSourceError::UnknownSource),
        };
        self.repository
            .cleanup_source_enabled(source_id)
            .await
            .map(|stored| stored.unwrap_or(default))
            .map_err(|_| CleanupSourceError::Dependency)
    }

    async fn musicbrainz(&self) -> Result<CleanupSource, CleanupSourceError> {
        Ok(musicbrainz_source(self.musicbrainz_enabled().await?))
    }

    async fn acoustid(&self) -> Result<CleanupSource, CleanupSourceError> {
        self.source(ACOUSTID_SOURCE_ID, self.enabled(ACOUSTID_SOURCE_ID).await?)
            .await
    }

    async fn lastfm(&self) -> Result<CleanupSource, CleanupSourceError> {
        self.source(LASTFM_SOURCE_ID, self.enabled(LASTFM_SOURCE_ID).await?)
            .await
    }

    async fn source(
        &self,
        source_id: &str,
        enabled: bool,
    ) -> Result<CleanupSource, CleanupSourceError> {
        match source_id {
            MUSICBRAINZ_SOURCE_ID => Ok(musicbrainz_source(enabled)),
            ACOUSTID_SOURCE_ID => Ok(acoustid_source(
                enabled,
                &self.runtime,
                self.credential_state(ACOUSTID_SOURCE_ID).await?,
            )),
            LASTFM_SOURCE_ID => Ok(lastfm_source(
                enabled,
                self.credential_state(LASTFM_SOURCE_ID).await?,
            )),
            _ => Err(CleanupSourceError::UnknownSource),
        }
    }

    pub async fn save_credential(
        &self,
        source_id: &str,
        api_key: &str,
    ) -> Result<CleanupSource, CleanupSourceError> {
        let subject = cleanup_source_credential_subject(source_id)?;
        if !(1..=MAX_CREDENTIAL_CHARS).contains(&api_key.trim().chars().count()) {
            return Err(CleanupSourceError::InvalidCredential);
        }
        let cipher = self
            .credentials
            .current_cipher()
            .await
            .map_err(|_| CleanupSourceError::CredentialStorage)?;
        let credential = cipher
            .encrypt(&subject, api_key)
            .map_err(|_| CleanupSourceError::CredentialStorage)?;
        self.repository
            .store_cleanup_source_credential(source_id, &credential)
            .await
            .map_err(|_| CleanupSourceError::Dependency)?;
        // Keep the vault read lease through the database write so a concurrent
        // complete reset cannot remove the master key between encryption and persistence.
        drop(cipher);
        self.source(source_id, self.enabled(source_id).await?).await
    }

    pub async fn delete_credential(
        &self,
        source_id: &str,
    ) -> Result<CleanupSource, CleanupSourceError> {
        let _ = cleanup_source_credential_subject(source_id)?;
        self.repository
            .clear_cleanup_source_credential(source_id)
            .await
            .map_err(|_| CleanupSourceError::Dependency)?;
        self.source(source_id, self.enabled(source_id).await?).await
    }

    pub async fn saved_credential(
        &self,
        source_id: &str,
    ) -> Result<Option<ProviderSecret>, CleanupSourceError> {
        let subject = cleanup_source_credential_subject(source_id)?;
        let Some(credential) = self
            .repository
            .cleanup_source_credential(source_id)
            .await
            .map_err(|_| CleanupSourceError::Dependency)?
        else {
            return Ok(None);
        };
        let cipher = self
            .credentials
            .current_cipher()
            .await
            .map_err(|_| CleanupSourceError::CredentialStorage)?;
        cipher
            .decrypt(&subject, &credential.ciphertext, &credential.nonce)
            .map(Some)
            .map_err(|_| CleanupSourceError::CredentialStorage)
    }

    async fn credential_state(
        &self,
        source_id: &str,
    ) -> Result<CredentialState, CleanupSourceError> {
        let Some(credential) = self
            .repository
            .cleanup_source_credential(source_id)
            .await
            .map_err(|_| CleanupSourceError::Dependency)?
        else {
            return Ok(CredentialState::environment(environment_configured(
                source_id,
                &self.runtime,
            )));
        };
        let subject = cleanup_source_credential_subject(source_id)?;
        let readable = match self.credentials.current_cipher().await {
            Ok(cipher) => cipher
                .decrypt(&subject, &credential.ciphertext, &credential.nonce)
                .is_ok(),
            Err(_) => false,
        };
        Ok(CredentialState {
            saved: true,
            configured: true,
            readable,
            source: Some("saved".to_owned()),
            key_hint: Some(credential.hint),
        })
    }
}

pub fn cleanup_source_credential_subject(source_id: &str) -> Result<String, CleanupSourceError> {
    match source_id {
        ACOUSTID_SOURCE_ID | LASTFM_SOURCE_ID => {
            Ok(format!("{CLEANUP_CREDENTIAL_SCOPE_PREFIX}{source_id}"))
        }
        _ => Err(CleanupSourceError::UnknownSource),
    }
}

fn environment_configured(source_id: &str, runtime: &CleanupSourceRuntime) -> bool {
    match source_id {
        ACOUSTID_SOURCE_ID => runtime.acoustid_configured,
        LASTFM_SOURCE_ID => runtime.lastfm_configured,
        _ => false,
    }
}

struct CredentialState {
    saved: bool,
    configured: bool,
    readable: bool,
    source: Option<String>,
    key_hint: Option<String>,
}

impl CredentialState {
    fn environment(configured: bool) -> Self {
        Self {
            saved: false,
            configured,
            readable: configured,
            source: configured.then(|| "environment".to_owned()),
            key_hint: None,
        }
    }
}

fn musicbrainz_source(enabled: bool) -> CleanupSource {
    CleanupSource {
        id: MUSICBRAINZ_SOURCE_ID.to_owned(),
        label: "MusicBrainz".to_owned(),
        description:
            "Identifies recordings and supplies canonical release metadata from the public catalog."
                .to_owned(),
        enabled,
        capabilities: vec![
            "artist_name_verification".to_owned(),
            "album_name_verification".to_owned(),
            "recording_identity".to_owned(),
            "canonical_metadata".to_owned(),
        ],
        credential_kind: None,
        credential_saved: false,
        credential_source: None,
        key_hint: None,
        configured: true,
        available: true,
        configuration_hint: None,
        unavailable_reason: None,
    }
}

fn acoustid_source(
    enabled: bool,
    runtime: &CleanupSourceRuntime,
    credential: CredentialState,
) -> CleanupSource {
    let available = credential.readable && runtime.fpcalc_available;
    let unavailable_reason = if credential.saved && !credential.readable {
        Some("The saved key cannot be decrypted. Repair secure storage or remove it.".to_owned())
    } else if !credential.configured {
        Some("Save an AcoustID key below or configure CLEANUP_ACOUSTID_API_KEY.".to_owned())
    } else if !runtime.fpcalc_available {
        Some("The configured fpcalc executable is unavailable.".to_owned())
    } else {
        None
    };
    CleanupSource {
        id: ACOUSTID_SOURCE_ID.to_owned(),
        label: "AcoustID".to_owned(),
        description:
            "Uses a local Chromaprint fingerprint only when metadata cannot identify a recording."
                .to_owned(),
        enabled,
        capabilities: vec!["acoustic_fingerprint_identity".to_owned()],
        credential_kind: Some("application API key".to_owned()),
        credential_saved: credential.saved,
        credential_source: credential.source,
        key_hint: credential.key_hint,
        configured: credential.configured,
        available,
        configuration_hint: Some(
            "Encrypted storage · CLEANUP_ACOUSTID_API_KEY fallback · CLEANUP_FPCALC_PATH"
                .to_owned(),
        ),
        unavailable_reason,
    }
}

fn lastfm_source(enabled: bool, credential: CredentialState) -> CleanupSource {
    CleanupSource {
        id: LASTFM_SOURCE_ID.to_owned(),
        label: "Last.fm".to_owned(),
        description:
            "Looks up community tags after a recording has a confident MusicBrainz identity."
                .to_owned(),
        enabled,
        capabilities: vec!["community_tag_evidence".to_owned()],
        credential_kind: Some("API key".to_owned()),
        credential_saved: credential.saved,
        credential_source: credential.source,
        key_hint: credential.key_hint,
        configured: credential.configured,
        available: credential.readable,
        configuration_hint: Some("Encrypted storage · CLEANUP_LASTFM_API_KEY fallback".to_owned()),
        unavailable_reason: if credential.saved && !credential.readable {
            Some(
                "The saved key cannot be decrypted. Repair secure storage or remove it.".to_owned(),
            )
        } else if credential.configured {
            None
        } else {
            Some("Save a Last.fm key below or configure CLEANUP_LASTFM_API_KEY.".to_owned())
        },
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::sync::{Arc, Mutex};

    use crate::assistant::{
        EncryptedProviderCredential, ProviderCredentialCipher, ProviderCredentialError,
        ProviderCredentialFuture, ProviderCredentialSource, ProviderSecret,
    };

    use super::{
        ACOUSTID_SOURCE_ID, CleanupSourceDependencyError, CleanupSourceFuture,
        CleanupSourceRepository, CleanupSourceRuntime, CleanupSourceService, LASTFM_SOURCE_ID,
        MUSICBRAINZ_SOURCE_ID,
    };

    #[derive(Default)]
    struct InMemoryRepository {
        values: Mutex<BTreeMap<String, bool>>,
        credentials: Mutex<BTreeMap<String, EncryptedProviderCredential>>,
    }

    impl CleanupSourceRepository for InMemoryRepository {
        fn cleanup_source_enabled(&self, source_id: &str) -> CleanupSourceFuture<'_, Option<bool>> {
            let source_id = source_id.to_owned();
            Box::pin(async move {
                self.values
                    .lock()
                    .map(|values| values.get(&source_id).copied())
                    .map_err(|error| {
                        Box::new(std::io::Error::other(error.to_string()))
                            as CleanupSourceDependencyError
                    })
            })
        }

        fn set_cleanup_source_enabled(
            &self,
            source_id: &str,
            enabled: bool,
        ) -> CleanupSourceFuture<'_, ()> {
            let source_id = source_id.to_owned();
            Box::pin(async move {
                self.values
                    .lock()
                    .map(|mut values| {
                        values.insert(source_id, enabled);
                    })
                    .map_err(|error| {
                        Box::new(std::io::Error::other(error.to_string()))
                            as CleanupSourceDependencyError
                    })
            })
        }

        fn cleanup_source_credential(
            &self,
            source_id: &str,
        ) -> CleanupSourceFuture<'_, Option<EncryptedProviderCredential>> {
            let source_id = source_id.to_owned();
            Box::pin(async move {
                self.credentials
                    .lock()
                    .map(|values| values.get(&source_id).cloned())
                    .map_err(|error| {
                        Box::new(std::io::Error::other(error.to_string()))
                            as CleanupSourceDependencyError
                    })
            })
        }

        fn store_cleanup_source_credential<'a>(
            &'a self,
            source_id: &'a str,
            credential: &'a EncryptedProviderCredential,
        ) -> CleanupSourceFuture<'a, ()> {
            Box::pin(async move {
                self.credentials
                    .lock()
                    .map(|mut values| {
                        values.insert(source_id.to_owned(), credential.clone());
                    })
                    .map_err(|error| {
                        Box::new(std::io::Error::other(error.to_string()))
                            as CleanupSourceDependencyError
                    })
            })
        }

        fn clear_cleanup_source_credential(
            &self,
            source_id: &str,
        ) -> CleanupSourceFuture<'_, bool> {
            let source_id = source_id.to_owned();
            Box::pin(async move {
                self.credentials
                    .lock()
                    .map(|mut values| values.remove(&source_id).is_some())
                    .map_err(|error| {
                        Box::new(std::io::Error::other(error.to_string()))
                            as CleanupSourceDependencyError
                    })
            })
        }
    }

    #[derive(Debug)]
    struct TestCipher;

    impl ProviderCredentialCipher for TestCipher {
        fn encrypt(
            &self,
            connection_id: &str,
            api_key: &str,
        ) -> Result<EncryptedProviderCredential, ProviderCredentialError> {
            Ok(EncryptedProviderCredential {
                ciphertext: api_key.trim().to_owned(),
                nonce: connection_id.to_owned(),
                hint: "••••test".to_owned(),
            })
        }

        fn decrypt(
            &self,
            connection_id: &str,
            ciphertext: &str,
            nonce: &str,
        ) -> Result<ProviderSecret, ProviderCredentialError> {
            if connection_id != nonce {
                return Err(ProviderCredentialError {
                    code: "credential_unreadable".to_owned(),
                });
            }
            Ok(ProviderSecret::new(ciphertext))
        }
    }

    #[derive(Debug)]
    struct TestCredentialSource;

    impl ProviderCredentialSource for TestCredentialSource {
        fn current_cipher(&self) -> ProviderCredentialFuture<'_> {
            Box::pin(async { Ok(Arc::new(TestCipher) as Arc<dyn ProviderCredentialCipher>) })
        }
    }

    fn credential_source() -> Arc<dyn ProviderCredentialSource> {
        Arc::new(TestCredentialSource)
    }

    #[tokio::test]
    async fn musicbrainz_defaults_on_and_persists_an_explicit_choice()
    -> Result<(), super::CleanupSourceError> {
        let service = CleanupSourceService::new(
            Arc::new(InMemoryRepository::default()),
            credential_source(),
            CleanupSourceRuntime {
                acoustid_configured: true,
                fpcalc_available: true,
                lastfm_configured: true,
            },
        );
        assert!(service.musicbrainz_enabled().await?);

        let sources = service.sources().await?;
        assert_eq!(sources.len(), 3);
        assert!(
            sources
                .iter()
                .find(|source| source.id == ACOUSTID_SOURCE_ID)
                .is_some_and(|source| source.available)
        );
        assert!(
            sources
                .iter()
                .find(|source| source.id == LASTFM_SOURCE_ID)
                .is_some_and(|source| source.available)
        );

        service.update(MUSICBRAINZ_SOURCE_ID, false).await?;
        assert!(!service.musicbrainz_enabled().await?);
        Ok(())
    }

    #[tokio::test]
    async fn saved_credentials_override_environment_without_returning_the_secret()
    -> Result<(), super::CleanupSourceError> {
        let service = CleanupSourceService::new(
            Arc::new(InMemoryRepository::default()),
            credential_source(),
            CleanupSourceRuntime {
                acoustid_configured: false,
                fpcalc_available: true,
                lastfm_configured: true,
            },
        );

        let saved = service
            .save_credential(ACOUSTID_SOURCE_ID, "catalog-secret")
            .await?;
        assert!(saved.credential_saved);
        assert_eq!(saved.credential_source.as_deref(), Some("saved"));
        assert_eq!(saved.key_hint.as_deref(), Some("••••test"));
        assert!(saved.available);
        assert_eq!(
            service
                .saved_credential(ACOUSTID_SOURCE_ID)
                .await?
                .as_ref()
                .map(ProviderSecret::expose_secret),
            Some("catalog-secret")
        );

        let fallback = service.delete_credential(LASTFM_SOURCE_ID).await?;
        assert!(!fallback.credential_saved);
        assert_eq!(fallback.credential_source.as_deref(), Some("environment"));
        assert!(fallback.available);
        Ok(())
    }
}
