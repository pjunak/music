use std::error::Error;
use std::fmt::{self, Display, Formatter};
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

pub const MUSICBRAINZ_SOURCE_ID: &str = "musicbrainz";

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
    pub fn new(repository: Arc<dyn CleanupSourceRepository>) -> Self {
        Self { repository }
    }

    pub async fn sources(&self) -> Result<Vec<CleanupSource>, CleanupSourceError> {
        Ok(vec![self.musicbrainz().await?])
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
        if source_id != MUSICBRAINZ_SOURCE_ID {
            return Err(CleanupSourceError::UnknownSource);
        }
        self.repository
            .set_cleanup_source_enabled(source_id, enabled)
            .await
            .map_err(|_| CleanupSourceError::Dependency)?;
        Ok(musicbrainz_source(enabled))
    }

    async fn musicbrainz(&self) -> Result<CleanupSource, CleanupSourceError> {
        Ok(musicbrainz_source(self.musicbrainz_enabled().await?))
    }
}

fn musicbrainz_source(enabled: bool) -> CleanupSource {
    CleanupSource {
        id: MUSICBRAINZ_SOURCE_ID.to_owned(),
        label: "MusicBrainz".to_owned(),
        description:
            "Checks ambiguous artist and album names against the public MusicBrainz catalog."
                .to_owned(),
        enabled,
        capabilities: vec![
            "artist_name_verification".to_owned(),
            "album_name_verification".to_owned(),
        ],
        credential_kind: None,
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::sync::{Arc, Mutex};

    use super::{
        CleanupSourceDependencyError, CleanupSourceFuture, CleanupSourceRepository,
        CleanupSourceService, MUSICBRAINZ_SOURCE_ID,
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
        let service = CleanupSourceService::new(Arc::new(InMemoryRepository::default()));
        assert!(service.musicbrainz_enabled().await?);

        service.update(MUSICBRAINZ_SOURCE_ID, false).await?;
        assert!(!service.musicbrainz_enabled().await?);
        Ok(())
    }
}
