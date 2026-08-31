use std::collections::BTreeSet;
use std::error::Error;
use std::fmt::{self, Display, Formatter};
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use music_domain::{
    CleanupFolderSuggestion, CleanupRuleSet, CleanupTrackPlan, IndexedTrack, LibraryPath,
    NameVerdicts, TrackId, analyze_cleanup, analyze_cleanup_folders, pending_cleanup_lookups,
};
use serde_json::{Map, Value, json};
use tokio::sync::Mutex;

use crate::library::{
    LibraryDependencyError, LibraryFileMutation, LibraryFileMutationOutcome,
    LibraryMutationRepository, LibraryRepository, LibraryStatus,
};
use crate::recovery::{RecoveryJournalEntry, RecoveryJournalId};

pub type CleanupDependencyError = Box<dyn Error + Send + Sync>;
pub type CleanupFuture<'a, T> =
    Pin<Box<dyn Future<Output = Result<T, CleanupDependencyError>> + Send + 'a>>;

pub trait CleanupRepository: LibraryRepository {
    fn cleanup_name_verdicts(&self) -> CleanupFuture<'_, NameVerdicts>;

    fn cleanup_batches(&self) -> CleanupFuture<'_, Vec<CleanupBatchSummary>>;

    fn cleanup_batch(&self, batch_id: i64) -> CleanupFuture<'_, Option<CleanupBatchDetail>>;
}

pub const MAX_CLEANUP_APPLY_OPERATIONS: usize = 500;
pub const MAX_CLEANUP_REVERT_ITEMS: usize = 5_000;
pub const MAX_CLEANUP_SCOPE_LABEL_CHARS: usize = 512;

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum CleanupOperationKind {
    Rename,
    Tag,
    FolderRename,
}

#[derive(Debug, Clone, Eq, PartialEq)]
pub enum CleanupInputValue {
    Integer(i64),
    Text(String),
}

impl CleanupInputValue {
    #[must_use]
    pub fn to_json(&self) -> Value {
        match self {
            Self::Integer(value) => Value::from(*value),
            Self::Text(value) => Value::String(value.clone()),
        }
    }
}

#[derive(Debug, Clone, Eq, PartialEq)]
pub struct CleanupApplyOperation {
    pub track_id: i64,
    pub kind: CleanupOperationKind,
    pub field: Option<String>,
    pub old: Option<CleanupInputValue>,
    pub new: Option<CleanupInputValue>,
    pub path: String,
}

#[derive(Debug, Clone, Eq, PartialEq)]
pub struct CleanupSkip {
    pub track_id: i64,
    pub reason: String,
}

#[derive(Debug, Clone, Eq, PartialEq)]
pub struct CleanupApplyResult {
    pub batch_id: Option<i64>,
    pub applied: usize,
    pub skipped: Vec<CleanupSkip>,
}

#[derive(Debug, Clone, Eq, PartialEq)]
pub struct CleanupRevertResult {
    pub reverted: usize,
    pub skipped: Vec<CleanupSkip>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct CleanupBatchAppend {
    batch_id: Option<i64>,
    scope_label: String,
    item: Map<String, Value>,
}

#[derive(Debug, Clone, Eq, PartialEq)]
pub struct CleanupRevertMutation {
    batch_id: Option<i64>,
    item_index: usize,
}

impl CleanupRevertMutation {
    pub fn new(
        batch_id: Option<i64>,
        item_index: usize,
    ) -> Result<Self, CleanupMutationValidationError> {
        if batch_id.is_some_and(|id| id <= 0) {
            return Err(CleanupMutationValidationError::InvalidBatchId);
        }
        Ok(Self {
            batch_id,
            item_index,
        })
    }

    pub fn from_journal(
        entry: &RecoveryJournalEntry,
    ) -> Result<Self, CleanupMutationValidationError> {
        let cleanup = entry
            .plan
            .get("cleanup_revert")
            .and_then(Value::as_object)
            .ok_or(CleanupMutationValidationError::InvalidJournalPlan)?;
        if cleanup.len() != 2 {
            return Err(CleanupMutationValidationError::InvalidJournalPlan);
        }
        let batch_id = match cleanup.get("batch_id") {
            Some(Value::Null) => None,
            Some(value) => Some(
                value
                    .as_i64()
                    .ok_or(CleanupMutationValidationError::InvalidJournalPlan)?,
            ),
            None => return Err(CleanupMutationValidationError::InvalidJournalPlan),
        };
        let item_index = cleanup
            .get("item_index")
            .and_then(Value::as_u64)
            .and_then(|value| usize::try_from(value).ok())
            .ok_or(CleanupMutationValidationError::InvalidJournalPlan)?;
        Self::new(batch_id, item_index)
    }

    #[must_use]
    pub const fn batch_id(&self) -> Option<i64> {
        self.batch_id
    }

    #[must_use]
    pub const fn item_index(&self) -> usize {
        self.item_index
    }

    pub fn journal_plan(
        &self,
        mutation: &LibraryFileMutation,
    ) -> Result<Value, CleanupMutationValidationError> {
        let mut plan = mutation.plan();
        let object = plan
            .as_object_mut()
            .ok_or(CleanupMutationValidationError::InvalidJournalPlan)?;
        object.insert(
            "cleanup_revert".to_owned(),
            json!({
                "batch_id": self.batch_id,
                "item_index": self.item_index,
            }),
        );
        Ok(plan)
    }
}

impl CleanupBatchAppend {
    pub fn new(
        batch_id: Option<i64>,
        scope_label: String,
        item: Map<String, Value>,
    ) -> Result<Self, CleanupMutationValidationError> {
        if batch_id.is_some_and(|id| id <= 0) {
            return Err(CleanupMutationValidationError::InvalidBatchId);
        }
        if scope_label.chars().count() > MAX_CLEANUP_SCOPE_LABEL_CHARS {
            return Err(CleanupMutationValidationError::ScopeLabelTooLong);
        }
        if item.is_empty() {
            return Err(CleanupMutationValidationError::InvalidJournalPlan);
        }
        Ok(Self {
            batch_id,
            scope_label,
            item,
        })
    }

    pub fn from_journal(
        entry: &RecoveryJournalEntry,
    ) -> Result<Self, CleanupMutationValidationError> {
        let cleanup = entry
            .plan
            .get("cleanup_batch")
            .and_then(Value::as_object)
            .ok_or(CleanupMutationValidationError::InvalidJournalPlan)?;
        if cleanup.len() != 3 {
            return Err(CleanupMutationValidationError::InvalidJournalPlan);
        }
        let batch_id = match cleanup.get("batch_id") {
            Some(Value::Null) => None,
            Some(value) => Some(
                value
                    .as_i64()
                    .ok_or(CleanupMutationValidationError::InvalidJournalPlan)?,
            ),
            None => return Err(CleanupMutationValidationError::InvalidJournalPlan),
        };
        let scope_label = cleanup
            .get("scope_label")
            .and_then(Value::as_str)
            .ok_or(CleanupMutationValidationError::InvalidJournalPlan)?
            .to_owned();
        let item = cleanup
            .get("item")
            .and_then(Value::as_object)
            .ok_or(CleanupMutationValidationError::InvalidJournalPlan)?
            .clone();
        Self::new(batch_id, scope_label, item)
    }

    #[must_use]
    pub const fn batch_id(&self) -> Option<i64> {
        self.batch_id
    }

    #[must_use]
    pub fn scope_label(&self) -> &str {
        &self.scope_label
    }

    #[must_use]
    pub const fn item(&self) -> &Map<String, Value> {
        &self.item
    }

    pub fn journal_plan(
        &self,
        mutation: &LibraryFileMutation,
    ) -> Result<Value, CleanupMutationValidationError> {
        let mut plan = mutation.plan();
        let object = plan
            .as_object_mut()
            .ok_or(CleanupMutationValidationError::InvalidJournalPlan)?;
        object.insert(
            "cleanup_batch".to_owned(),
            json!({
                "batch_id": self.batch_id,
                "scope_label": self.scope_label,
                "item": self.item,
            }),
        );
        Ok(plan)
    }
}

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum CleanupMutationValidationError {
    InvalidBatchId,
    ScopeLabelTooLong,
    InvalidJournalPlan,
}

impl Display for CleanupMutationValidationError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::InvalidBatchId => "cleanup batch id is invalid",
            Self::ScopeLabelTooLong => "cleanup scope label is too long",
            Self::InvalidJournalPlan => "cleanup mutation journal is invalid",
        })
    }
}

impl Error for CleanupMutationValidationError {}

#[derive(Debug, Clone, PartialEq)]
pub struct CleanupMutationCommit {
    pub status: LibraryStatus,
    pub affected_tracks: u64,
    pub track: Option<IndexedTrack>,
    pub batch_id: i64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct CleanupRevertMutationCommit {
    pub status: LibraryStatus,
    pub affected_tracks: u64,
    pub track: Option<IndexedTrack>,
}

pub trait CleanupMutationRepository: LibraryMutationRepository + CleanupRepository {
    fn commit_cleanup_mutation<'a>(
        &'a self,
        journal_id: &'a RecoveryJournalId,
        mutation: &'a LibraryFileMutation,
        outcome: LibraryFileMutationOutcome,
        append: &'a CleanupBatchAppend,
    ) -> CleanupFuture<'a, CleanupMutationCommit>;

    fn commit_cleanup_revert_mutation<'a>(
        &'a self,
        journal_id: &'a RecoveryJournalId,
        mutation: &'a LibraryFileMutation,
        outcome: LibraryFileMutationOutcome,
        revert: &'a CleanupRevertMutation,
    ) -> CleanupFuture<'a, CleanupRevertMutationCommit>;

    fn finish_cleanup_batch_revert<'a>(
        &'a self,
        journal_id: &'a RecoveryJournalId,
        batch_id: i64,
        reverted: usize,
        skipped: usize,
    ) -> CleanupFuture<'a, ()>;
}

pub const MAX_CLEANUP_BATCH_HISTORY: usize = 100;

#[derive(Debug, Clone, Eq, PartialEq)]
pub struct CleanupBatchSummary {
    pub id: i64,
    pub created_at_unix_seconds: i64,
    pub scope_label: String,
    pub item_count: usize,
    pub reverted_at_unix_seconds: Option<i64>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct CleanupBatchDetail {
    pub id: i64,
    pub created_at_unix_seconds: i64,
    pub scope_label: String,
    pub item_count: usize,
    pub reverted_at_unix_seconds: Option<i64>,
    pub items: Vec<serde_json::Map<String, serde_json::Value>>,
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
        self.analyze_with_online_evidence(scope, rules, true).await
    }

    pub async fn analyze_with_online_evidence(
        &self,
        scope: CleanupScope,
        rules: CleanupRuleSet,
        use_online_evidence: bool,
    ) -> Result<CleanupAnalysis, CleanupError> {
        if matches!(&scope, CleanupScope::Tracks(track_ids) if track_ids.is_empty()) {
            return Err(CleanupError::EmptyTrackScope);
        }
        let (all_tracks, verdicts) = if use_online_evidence {
            let (tracks, verdicts) = tokio::try_join!(
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
            (tracks, Some(verdicts))
        } else {
            let tracks = self
                .repository
                .all_tracks()
                .await
                .map_err(|source| dependency("load the cleanup library catalog", source))?;
            (tracks, None)
        };
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
        let plans = analyze_cleanup(&scope_tracks, &all_tracks, rules, verdicts.as_ref());
        let folders = analyze_cleanup_folders(&scope_tracks, &all_tracks, rules);
        let pending_lookups = if use_online_evidence {
            pending_cleanup_lookups(&plans, verdicts.as_ref())
        } else {
            Vec::new()
        };
        Ok(CleanupAnalysis {
            scanned: scope_tracks.len(),
            plans,
            folders,
            pending_lookups,
        })
    }

    pub async fn batches(&self) -> Result<Vec<CleanupBatchSummary>, CleanupError> {
        self.repository
            .cleanup_batches()
            .await
            .map_err(|source| dependency("load cleanup batch history", source))
    }

    pub async fn batch(&self, batch_id: i64) -> Result<Option<CleanupBatchDetail>, CleanupError> {
        self.repository
            .cleanup_batch(batch_id)
            .await
            .map_err(|source| dependency("load a cleanup batch", source))
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
        CleanupBatchAppend, CleanupFuture, CleanupNameLookup, CleanupNameScores,
        CleanupNameVerdict, CleanupRevertMutation, CleanupVerificationRepository,
        CleanupVerificationService,
    };
    use crate::library::LibraryFileMutation;
    use crate::recovery::{
        RecoveryDomain, RecoveryJournalDraft, RecoveryJournalEntry, RecoveryState,
    };
    use music_domain::{LibraryPath, TrackId};

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

    #[test]
    fn cleanup_batch_append_round_trips_with_its_library_mutation() -> Result<(), Box<dyn Error>> {
        let mutation = LibraryFileMutation::MoveTrack {
            track_id: TrackId::new(17)?,
            source: LibraryPath::parse("Album/01 - Song.wav")?,
            destination: LibraryPath::parse("Album/Song.wav")?,
        };
        let append = CleanupBatchAppend::new(
            Some(9),
            "Album".to_owned(),
            serde_json::Map::from_iter([
                ("kind".to_owned(), serde_json::json!("rename")),
                ("track_id".to_owned(), serde_json::json!(17)),
            ]),
        )?;
        let draft = RecoveryJournalDraft::new(
            RecoveryDomain::Cleanup,
            mutation.operation()?,
            append.journal_plan(&mutation)?,
        )?;
        let entry = RecoveryJournalEntry {
            id: draft.id,
            domain: draft.domain,
            operation: draft.operation,
            state: RecoveryState::Applying,
            plan: draft.plan,
            progress: draft.progress,
            created_at_unix_seconds: 1,
            updated_at_unix_seconds: 1,
            completed_at_unix_seconds: None,
        };
        assert_eq!(CleanupBatchAppend::from_journal(&entry)?, append);
        assert_eq!(LibraryFileMutation::from_journal(&entry)?, mutation);
        Ok(())
    }

    #[test]
    fn cleanup_revert_context_round_trips_with_its_library_mutation() -> Result<(), Box<dyn Error>>
    {
        let mutation = LibraryFileMutation::MoveTrack {
            track_id: TrackId::new(17)?,
            source: LibraryPath::parse("Album/Song.wav")?,
            destination: LibraryPath::parse("Album/01 - Song.wav")?,
        };
        let revert = CleanupRevertMutation::new(Some(9), 4)?;
        let draft = RecoveryJournalDraft::new(
            RecoveryDomain::Cleanup,
            mutation.operation()?,
            revert.journal_plan(&mutation)?,
        )?;
        let entry = RecoveryJournalEntry {
            id: draft.id,
            domain: draft.domain,
            operation: draft.operation,
            state: RecoveryState::Applying,
            plan: draft.plan,
            progress: draft.progress,
            created_at_unix_seconds: 1,
            updated_at_unix_seconds: 1,
            completed_at_unix_seconds: None,
        };
        assert_eq!(CleanupRevertMutation::from_journal(&entry)?, revert);
        assert_eq!(LibraryFileMutation::from_journal(&entry)?, mutation);
        Ok(())
    }
}
