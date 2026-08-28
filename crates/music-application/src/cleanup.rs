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
use tokio::sync::Mutex;

use crate::library::{LibraryDependencyError, LibraryRepository};

pub type CleanupDependencyError = Box<dyn Error + Send + Sync>;
pub type CleanupFuture<'a, T> =
    Pin<Box<dyn Future<Output = Result<T, CleanupDependencyError>> + Send + 'a>>;

pub trait CleanupRepository: LibraryRepository {
    fn cleanup_name_verdicts(&self) -> CleanupFuture<'_, NameVerdicts>;
}

pub const MAX_CLEANUP_VERIFY_NAMES: usize = 5;
const MAX_STORED_CLEANUP_NAME_CHARS: usize = 512;

pub trait CleanupVerificationRepository: std::fmt::Debug + Send + Sync {
    fn cleanup_name_verdict_exists<'a>(&'a self, loose_key: &'a str) -> CleanupFuture<'a, bool>;

    fn store_cleanup_name_verdict<'a>(
        &'a self,
        verdict: &'a CleanupNameVerdict,
    ) -> CleanupFuture<'a, bool>;
}

pub trait CleanupNameLookup: std::fmt::Debug + Send + Sync {
    fn fetch_name_scores<'a>(&'a self, name: &'a str) -> CleanupFuture<'a, CleanupNameScores>;
}

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub struct CleanupNameScores {
    artist: u8,
    album: u8,
}

impl CleanupNameScores {
    pub fn new(artist: i32, album: i32) -> Result<Self, CleanupNameScoreError> {
        Ok(Self {
            artist: u8::try_from(artist)
                .ok()
                .filter(|score| *score <= 100)
                .ok_or(CleanupNameScoreError)?,
            album: u8::try_from(album)
                .ok()
                .filter(|score| *score <= 100)
                .ok_or(CleanupNameScoreError)?,
        })
    }

    #[must_use]
    pub const fn artist(self) -> u8 {
        self.artist
    }

    #[must_use]
    pub const fn album(self) -> u8 {
        self.album
    }
}

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub struct CleanupNameScoreError;

impl Display for CleanupNameScoreError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        formatter.write_str("cleanup name scores must be between 0 and 100")
    }
}

impl Error for CleanupNameScoreError {}

#[derive(Debug, Clone, Eq, PartialEq)]
pub struct CleanupNameVerdict {
    loose_key: String,
    name: String,
    scores: CleanupNameScores,
}

impl CleanupNameVerdict {
    #[must_use]
    pub fn new(loose_key: String, name: &str, scores: CleanupNameScores) -> Self {
        Self {
            loose_key,
            name: name.chars().take(MAX_STORED_CLEANUP_NAME_CHARS).collect(),
            scores,
        }
    }

    #[must_use]
    pub fn loose_key(&self) -> &str {
        &self.loose_key
    }

    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }

    #[must_use]
    pub const fn scores(&self) -> CleanupNameScores {
        self.scores
    }
}

#[derive(Debug, Clone, Eq, PartialEq)]
pub struct CleanupVerificationResult {
    pub verified: usize,
    pub failed: Vec<String>,
}

#[derive(Debug)]
pub enum CleanupVerificationError {
    InvalidBatchSize,
    Dependency {
        operation: &'static str,
        source: CleanupDependencyError,
    },
}

impl Display for CleanupVerificationError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidBatchSize => write!(
                formatter,
                "cleanup verification requires between 1 and {MAX_CLEANUP_VERIFY_NAMES} names"
            ),
            Self::Dependency { operation, .. } => write!(formatter, "failed to {operation}"),
        }
    }
}

impl Error for CleanupVerificationError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::InvalidBatchSize => None,
            Self::Dependency { source, .. } => Some(source.as_ref()),
        }
    }
}

#[derive(Debug, Clone)]
pub struct CleanupVerificationService {
    repository: Arc<dyn CleanupVerificationRepository>,
    lookup: Arc<dyn CleanupNameLookup>,
    batch_gate: Arc<Mutex<()>>,
}

impl CleanupVerificationService {
    #[must_use]
    pub fn new(
        repository: Arc<dyn CleanupVerificationRepository>,
        lookup: Arc<dyn CleanupNameLookup>,
    ) -> Self {
        Self {
            repository,
            lookup,
            batch_gate: Arc::new(Mutex::new(())),
        }
    }

    pub async fn verify(
        &self,
        names: Vec<String>,
    ) -> Result<CleanupVerificationResult, CleanupVerificationError> {
        if !(1..=MAX_CLEANUP_VERIFY_NAMES).contains(&names.len()) {
            return Err(CleanupVerificationError::InvalidBatchSize);
        }
        let _batch = self.batch_gate.lock().await;
        let mut seen = BTreeSet::new();
        let mut result = CleanupVerificationResult {
            verified: 0,
            failed: Vec::new(),
        };
        for raw_name in names {
            let name = raw_name.trim();
            let loose_key = music_domain::cleanup_loose_key(name);
            if loose_key.len() < 2 || !seen.insert(loose_key.clone()) {
                continue;
            }
            let exists = self
                .repository
                .cleanup_name_verdict_exists(&loose_key)
                .await
                .map_err(|source| verification_dependency("read a cleanup name verdict", source))?;
            if exists {
                continue;
            }
            let scores = match self.lookup.fetch_name_scores(name).await {
                Ok(scores) => scores,
                Err(_) => {
                    result.failed.push(name.to_owned());
                    continue;
                }
            };
            let verdict = CleanupNameVerdict::new(loose_key, name, scores);
            let inserted = self
                .repository
                .store_cleanup_name_verdict(&verdict)
                .await
                .map_err(|source| {
                    verification_dependency("store a cleanup name verdict", source)
                })?;
            result.verified += usize::from(inserted);
        }
        Ok(result)
    }
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

fn verification_dependency(
    operation: &'static str,
    source: CleanupDependencyError,
) -> CleanupVerificationError {
    CleanupVerificationError::Dependency { operation, source }
}

#[cfg(test)]
mod tests {
    use std::collections::{BTreeMap, BTreeSet};
    use std::error::Error;
    use std::io;
    use std::sync::Arc;

    use tokio::sync::Mutex;

    use super::{
        CleanupFuture, CleanupNameLookup, CleanupNameScores, CleanupNameVerdict,
        CleanupVerificationRepository, CleanupVerificationService,
    };

    #[derive(Debug, Default)]
    struct MemoryVerdicts {
        values: Mutex<BTreeMap<String, (String, CleanupNameScores)>>,
    }

    impl CleanupVerificationRepository for MemoryVerdicts {
        fn cleanup_name_verdict_exists<'a>(
            &'a self,
            loose_key: &'a str,
        ) -> CleanupFuture<'a, bool> {
            Box::pin(async move { Ok(self.values.lock().await.contains_key(loose_key)) })
        }

        fn store_cleanup_name_verdict<'a>(
            &'a self,
            verdict: &'a CleanupNameVerdict,
        ) -> CleanupFuture<'a, bool> {
            Box::pin(async move {
                Ok(self
                    .values
                    .lock()
                    .await
                    .insert(
                        verdict.loose_key().to_owned(),
                        (verdict.name().to_owned(), verdict.scores()),
                    )
                    .is_none())
            })
        }
    }

    #[derive(Debug)]
    struct FakeLookup {
        scores: CleanupNameScores,
        failures: Mutex<BTreeSet<String>>,
        calls: Mutex<Vec<String>>,
    }

    impl FakeLookup {
        fn new(scores: CleanupNameScores, failures: impl IntoIterator<Item = String>) -> Self {
            Self {
                scores,
                failures: Mutex::new(failures.into_iter().collect()),
                calls: Mutex::new(Vec::new()),
            }
        }
    }

    impl CleanupNameLookup for FakeLookup {
        fn fetch_name_scores<'a>(&'a self, name: &'a str) -> CleanupFuture<'a, CleanupNameScores> {
            Box::pin(async move {
                self.calls.lock().await.push(name.to_owned());
                if self.failures.lock().await.contains(name) {
                    return Err(Box::new(io::Error::other("simulated lookup failure"))
                        as Box<dyn Error + Send + Sync>);
                }
                Ok(self.scores)
            })
        }
    }

    #[tokio::test]
    async fn verification_caches_successes_and_retries_only_failures() -> Result<(), Box<dyn Error>>
    {
        let repository = Arc::new(MemoryVerdicts::default());
        let lookup = Arc::new(FakeLookup::new(
            CleanupNameScores::new(100, 20)?,
            ["Flaky Lookup".to_owned()],
        ));
        let service = CleanupVerificationService::new(repository.clone(), lookup.clone());

        let first = service
            .verify(vec![
                " Andrey Vinogradov ".to_owned(),
                "Andrey Vinogradov".to_owned(),
                "x".to_owned(),
                "Flaky Lookup".to_owned(),
            ])
            .await?;
        assert_eq!(first.verified, 1);
        assert_eq!(first.failed, ["Flaky Lookup"]);
        assert_eq!(repository.values.lock().await.len(), 1);

        lookup.failures.lock().await.clear();
        let second = service
            .verify(vec![
                "Andrey Vinogradov".to_owned(),
                "Flaky Lookup".to_owned(),
            ])
            .await?;
        assert_eq!(second.verified, 1);
        assert!(second.failed.is_empty());
        assert_eq!(
            lookup.calls.lock().await.as_slice(),
            ["Andrey Vinogradov", "Flaky Lookup", "Flaky Lookup"]
        );
        Ok(())
    }

    #[tokio::test]
    async fn concurrent_batches_share_one_idempotency_gate() -> Result<(), Box<dyn Error>> {
        let repository = Arc::new(MemoryVerdicts::default());
        let lookup = Arc::new(FakeLookup::new(CleanupNameScores::new(95, 10)?, []));
        let service = CleanupVerificationService::new(repository, lookup.clone());
        let left = service.verify(vec!["Shared Name".to_owned()]);
        let right = service.verify(vec!["Shared Name".to_owned()]);
        let (left, right) = tokio::join!(left, right);
        assert_eq!(left?.verified + right?.verified, 1);
        assert_eq!(lookup.calls.lock().await.as_slice(), ["Shared Name"]);
        Ok(())
    }

    #[test]
    fn name_scores_reject_out_of_contract_values() {
        assert!(CleanupNameScores::new(0, 100).is_ok());
        assert!(CleanupNameScores::new(-1, 50).is_err());
        assert!(CleanupNameScores::new(50, 101).is_err());
    }
}
