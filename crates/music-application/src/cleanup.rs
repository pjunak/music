use std::collections::BTreeSet;
use std::error::Error;
use std::fmt::{self, Display, Formatter};
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use music_domain::{
    CleanupFolderSuggestion, CleanupRuleSet, CleanupTrackPlan, LibraryPath, NameVerdicts, TrackId,
    analyze_cleanup, analyze_cleanup_folders, pending_cleanup_lookups,
};

use crate::library::{LibraryDependencyError, LibraryRepository};

pub type CleanupDependencyError = Box<dyn Error + Send + Sync>;
pub type CleanupFuture<'a, T> =
    Pin<Box<dyn Future<Output = Result<T, CleanupDependencyError>> + Send + 'a>>;

pub trait CleanupRepository: LibraryRepository {
    fn cleanup_name_verdicts(&self) -> CleanupFuture<'_, NameVerdicts>;
}

#[derive(Debug, Clone, Eq, PartialEq)]
pub enum CleanupScope {
    All,
    Folder {
        path: Option<LibraryPath>,
        recursive: bool,
    },
    Tracks(Vec<TrackId>),
}

#[derive(Debug, Clone, Eq, PartialEq)]
pub struct CleanupAnalysis {
    pub scanned: usize,
    pub plans: Vec<CleanupTrackPlan>,
    pub folders: Vec<CleanupFolderSuggestion>,
    pub pending_lookups: Vec<String>,
}

#[derive(Debug)]
pub enum CleanupError {
    EmptyTrackScope,
    Dependency {
        operation: &'static str,
        source: CleanupDependencyError,
    },
}

impl Display for CleanupError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        match self {
            Self::EmptyTrackScope => formatter.write_str("cleanup track scope must not be empty"),
            Self::Dependency { operation, .. } => write!(formatter, "failed to {operation}"),
        }
    }
}

impl Error for CleanupError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::EmptyTrackScope => None,
            Self::Dependency { source, .. } => Some(source.as_ref()),
        }
    }
}

#[derive(Debug, Clone)]
pub struct CleanupService {
    repository: Arc<dyn CleanupRepository>,
}

impl CleanupService {
    #[must_use]
    pub fn new(repository: Arc<dyn CleanupRepository>) -> Self {
        Self { repository }
    }

    pub async fn analyze(
        &self,
        scope: CleanupScope,
        rules: CleanupRuleSet,
    ) -> Result<CleanupAnalysis, CleanupError> {
        if matches!(&scope, CleanupScope::Tracks(track_ids) if track_ids.is_empty()) {
            return Err(CleanupError::EmptyTrackScope);
        }
        let (all_tracks, verdicts) = tokio::try_join!(
            async {
                self.repository
                    .all_tracks()
                    .await
                    .map_err(|source| dependency("load the cleanup library catalog", source))
            },
            async {
                self.repository
                    .cleanup_name_verdicts()
                    .await
                    .map_err(|source| dependency("load cached cleanup name verdicts", source))
            }
        )?;
        let scope_tracks = match scope {
            CleanupScope::All => all_tracks.clone(),
            CleanupScope::Tracks(track_ids) => {
                let selected = track_ids.into_iter().collect::<BTreeSet<_>>();
                all_tracks
                    .iter()
                    .filter(|track| selected.contains(&track.id))
                    .cloned()
                    .collect()
            }
            CleanupScope::Folder { path, recursive } => all_tracks
                .iter()
                .filter(|track| match path.as_ref() {
                    None if recursive => true,
                    None => track.path.parent().is_none(),
                    Some(path) if recursive => {
                        let prefix = format!("{}/", path.as_str());
                        track.path.as_str().starts_with(&prefix)
                    }
                    Some(path) => track.path.parent().as_ref() == Some(path),
                })
                .cloned()
                .collect(),
        };
        let plans = analyze_cleanup(&scope_tracks, &all_tracks, rules, Some(&verdicts));
        let folders = analyze_cleanup_folders(&scope_tracks, &all_tracks, rules);
        let pending_lookups = pending_cleanup_lookups(&plans, Some(&verdicts));
        Ok(CleanupAnalysis {
            scanned: scope_tracks.len(),
            plans,
            folders,
            pending_lookups,
        })
    }
}

fn dependency(operation: &'static str, source: LibraryDependencyError) -> CleanupError {
    CleanupError::Dependency { operation, source }
}
