use std::error::Error;
use std::fmt::{self, Display, Formatter};
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

pub const MUSICBRAINZ_SOURCE_ID: &str = "musicbrainz";
pub const ACOUSTID_SOURCE_ID: &str = "acoustid";
pub const LASTFM_SOURCE_ID: &str = "lastfm";

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
}

#[derive(Debug, Clone, Eq, PartialEq)]
pub struct CleanupSource {
    pub id: String,
    pub label: String,
    pub description: String,
    pub enabled: bool,
    pub capabilities: Vec<String>,
    pub credential_kind: Option<String>,
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
    Dependency,
}

impl Display for CleanupSourceError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::UnknownSource => "cleanup source is not recognized",
            Self::Dependency => "cleanup source settings are unavailable",
        })
    }
}

impl Error for CleanupSourceError {}

#[derive(Clone)]
pub struct CleanupSourceService {
    repository: Arc<dyn CleanupSourceRepository>,
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
        runtime: CleanupSourceRuntime,
    ) -> Self {
        Self {
            repository,
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
        self.source(source_id, enabled)
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
    }

    async fn lastfm(&self) -> Result<CleanupSource, CleanupSourceError> {
        self.source(LASTFM_SOURCE_ID, self.enabled(LASTFM_SOURCE_ID).await?)
    }

    fn source(&self, source_id: &str, enabled: bool) -> Result<CleanupSource, CleanupSourceError> {
        match source_id {
            MUSICBRAINZ_SOURCE_ID => Ok(musicbrainz_source(enabled)),
            ACOUSTID_SOURCE_ID => Ok(acoustid_source(enabled, &self.runtime)),
            LASTFM_SOURCE_ID => Ok(lastfm_source(enabled, &self.runtime)),
            _ => Err(CleanupSourceError::UnknownSource),
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
        configured: true,
        available: true,
        configuration_hint: None,
        unavailable_reason: None,
    }
}

fn acoustid_source(enabled: bool, runtime: &CleanupSourceRuntime) -> CleanupSource {
    let configured = runtime.acoustid_configured;
    let available = configured && runtime.fpcalc_available;
    let unavailable_reason = if !configured {
        Some("Set CLEANUP_ACOUSTID_API_KEY and restart the server.".to_owned())
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
        configured,
        available,
        configuration_hint: Some("CLEANUP_ACOUSTID_API_KEY · CLEANUP_FPCALC_PATH".to_owned()),
        unavailable_reason,
    }
}

fn lastfm_source(enabled: bool, runtime: &CleanupSourceRuntime) -> CleanupSource {
    let configured = runtime.lastfm_configured;
    CleanupSource {
        id: LASTFM_SOURCE_ID.to_owned(),
        label: "Last.fm".to_owned(),
        description:
            "Looks up community tags after a recording has a confident MusicBrainz identity."
                .to_owned(),
        enabled,
        capabilities: vec!["community_tag_evidence".to_owned()],
        credential_kind: Some("API key".to_owned()),
        configured,
        available: configured,
        configuration_hint: Some("CLEANUP_LASTFM_API_KEY".to_owned()),
        unavailable_reason: (!configured)
            .then(|| "Set CLEANUP_LASTFM_API_KEY and restart the server.".to_owned()),
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::sync::{Arc, Mutex};

    use super::{
        ACOUSTID_SOURCE_ID, CleanupSourceDependencyError, CleanupSourceFuture,
        CleanupSourceRepository, CleanupSourceRuntime, CleanupSourceService, LASTFM_SOURCE_ID,
        MUSICBRAINZ_SOURCE_ID,
    };

    #[derive(Default)]
    struct InMemoryRepository {
        values: Mutex<BTreeMap<String, bool>>,
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
    }

    #[tokio::test]
    async fn musicbrainz_defaults_on_and_persists_an_explicit_choice()
    -> Result<(), super::CleanupSourceError> {
        let service = CleanupSourceService::new(
            Arc::new(InMemoryRepository::default()),
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
}
