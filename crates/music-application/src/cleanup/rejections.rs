//! Durable review decisions never apply authored changes or hide changed evidence.
use super::{CleanupFuture, CleanupService};
use music_domain::{IndexedTrack, LibraryPath};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

pub const MAX_REJECTION_MATCH: usize = 100;
pub const REJECTION_PAGE_SIZE: usize = 50;
const REVIEW_POLICY: &str = "cleanup-rejections/v1";

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CleanupReviewProposal {
    pub op_id: String,
    pub track_id: i64,
    pub path: String,
    pub kind: String,
    pub field: Option<String>,
    pub old: Value,
    pub new: Value,
    pub rules: Vec<String>,
    pub confidence: String,
    pub verified: bool,
    pub evidence: Option<Value>,
    pub evidence_context: Option<String>,
}

impl CleanupReviewProposal {
    pub fn validate(&self) -> Result<(), RejectionError> {
        let bounded = |text: &str, max| text.len() <= max && !text.chars().any(char::is_control);
        let value_valid = |value: &Value| {
            // Raw tags may contain the whitespace that cleanup is repairing.
            // They are stored as JSON data and rendered as escaped text.
            value.is_null()
                || value.as_i64().is_some()
                || value.as_str().is_some_and(|text| text.len() <= 4096)
        };
        if self.op_id.is_empty()
            || !bounded(&self.op_id, 256)
            || LibraryPath::parse(&self.path).is_err()
            || !matches!(self.confidence.as_str(), "high" | "low")
            || self.rules.is_empty()
            || self.rules.len() > 32
            || self
                .rules
                .iter()
                .any(|rule| rule.is_empty() || !bounded(rule, 64))
            || !value_valid(&self.old)
            || !value_valid(&self.new)
            || self.old == self.new
            || self
                .evidence_context
                .as_ref()
                .is_some_and(|text| !bounded(text, 2048))
            || self
                .evidence
                .as_ref()
                .is_some_and(|value| !value.is_object() || value.to_string().len() > 4096)
        {
            return Err(RejectionError::Invalid);
        }
        match self.kind.as_str() {
            "tag" if self.track_id > 0 => match self.field.as_deref() {
                Some(
                    "title"
                    | "artist"
                    | "album"
                    | "album_artist"
                    | "genre"
                    | "composer"
                    | "release_date"
                    | "original_release_date",
                ) if self.new.is_null() || self.new.is_string() => {}
                Some("track_no" | "disc_no" | "year")
                    if self.new.is_null()
                        || self
                            .new
                            .as_u64()
                            .is_some_and(|number| (1..=9999).contains(&number)) => {}
                _ => return Err(RejectionError::Invalid),
            },
            "rename" | "folder_rename" if self.field.is_none() => {
                if (self.kind == "rename" && self.track_id <= 0)
                    || (self.kind == "folder_rename" && self.track_id != 0)
                    || !self.new.as_str().is_some_and(|name| {
                        !name.is_empty()
                            && !name.contains(['/', '\\'])
                            && name != "."
                            && name != ".."
                    })
                {
                    return Err(RejectionError::Invalid);
                }
            }
            _ => return Err(RejectionError::Invalid),
        }
        Ok(())
    }

    /// Run IDs do not identify decisions. Source observations, grading and
    /// policy versions remain part of the decision fingerprint.
    pub fn fingerprint(&self, context: &str) -> String {
        let mut rules = self.rules.clone();
        rules.sort();
        rules.dedup();
        digest(
            &json!([
                REVIEW_POLICY,
                context,
                self.track_id,
                self.path,
                self.kind,
                self.field,
                self.old,
                self.new,
                rules,
                self.evidence,
                self.evidence_context,
                self.verified,
                self.confidence
            ])
            .to_string(),
        )
    }
}

#[derive(Debug, Clone)]
pub struct RejectionSnapshot {
    pub tracks: Vec<IndexedTrack>,
    pub catalog_revision: i64,
}

/// Also used inside storage transactions to reject a stale save/restore.
pub fn rejection_context(
    proposal: &CleanupReviewProposal,
    snapshot: &RejectionSnapshot,
) -> Option<String> {
    let context = if proposal.kind == "folder_rename" {
        let prefix = format!("{}/", proposal.path);
        let mut tracks = snapshot
            .tracks
            .iter()
            .filter(|track| track.path.as_str().starts_with(&prefix))
            .collect::<Vec<_>>();
        tracks.sort_by(|a, b| a.path.cmp(&b.path));
        if tracks.is_empty() || proposal.old.as_str() != proposal.path.rsplit('/').next() {
            return None;
        }
        tracks
            .into_iter()
            .map(crate::cleanup_enrichment::cleanup_enrichment_source_signature)
            .collect::<Result<Vec<_>, _>>()
            .ok()?
            .join(":")
    } else {
        let track = snapshot.tracks.iter().find(|track| {
            track.id.get() == proposal.track_id && track.path.as_str() == proposal.path
        })?;
        let current = match proposal.kind.as_str() {
            "rename" => json!(
                track
                    .path
                    .file_name()
                    .rsplit_once('.')
                    .map_or(track.path.file_name(), |(stem, _)| stem)
            ),
            "tag" => match proposal.field.as_deref()? {
                "title" => json!(track.metadata.title),
                "artist" => json!(track.metadata.artist),
                "album" => json!(track.metadata.album),
                "album_artist" => json!(track.metadata.album_artist),
                "genre" => json!(track.metadata.genre),
                "track_no" => json!(track.metadata.track_no),
                "disc_no" => json!(track.metadata.disc_no),
                "year" => json!(track.metadata.year),
                "release_date" => json!(track.metadata.release_date),
                "original_release_date" => json!(track.metadata.original_release_date),
                "composer" => json!(track.metadata.composer),
                _ => return None,
            },
            _ => return None,
        };
        if current != proposal.old {
            return None;
        }
        crate::cleanup_enrichment::review_context_signature(track, &snapshot.tracks).ok()?
    };
    let revision = if proposal.evidence.is_some() || proposal.verified {
        snapshot.catalog_revision
    } else {
        0
    };
    Some(digest(
        &json!([REVIEW_POLICY, context, revision]).to_string(),
    ))
}

fn digest(text: &str) -> String {
    format!("{:x}", Sha256::digest(text))
}

#[derive(Debug, Clone)]
pub struct CleanupRejection {
    pub id: i64,
    pub fingerprint: String,
    pub context_signature: String,
    pub proposal: CleanupReviewProposal,
    pub rejected_at: i64,
}

#[derive(Debug)]
pub enum RejectionError {
    Invalid,
    Stale,
    Missing,
    Dependency,
}

impl std::fmt::Display for RejectionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "cleanup rejection: {self:?}")
    }
}
impl std::error::Error for RejectionError {}

pub trait CleanupRejectionRepository: std::fmt::Debug + Send + Sync {
    fn rejection_snapshot(&self) -> CleanupFuture<'_, RejectionSnapshot>;
    fn match_rejections<'a>(
        &'a self,
        fingerprints: &'a [String],
    ) -> CleanupFuture<'a, Vec<Option<i64>>>;
    fn save_rejection<'a>(
        &'a self,
        proposal: &'a CleanupReviewProposal,
        context: &'a str,
    ) -> CleanupFuture<'a, Option<CleanupRejection>>;
    fn rejection_page<'a>(
        &'a self,
        before: Option<i64>,
        search: &'a str,
    ) -> CleanupFuture<'a, Vec<CleanupRejection>>;
    fn restore_rejection(&self, id: i64) -> CleanupFuture<'_, Option<CleanupRejection>>;
    fn forget_rejection(&self, id: i64) -> CleanupFuture<'_, ()>;
}

impl CleanupService {
    pub async fn rejected_matches(
        &self,
        proposals: &[CleanupReviewProposal],
    ) -> Result<Vec<Option<i64>>, RejectionError> {
        if proposals.len() > MAX_REJECTION_MATCH {
            return Err(RejectionError::Invalid);
        }
        for proposal in proposals {
            proposal.validate()?;
        }
        let snapshot = self
            .repository
            .rejection_snapshot()
            .await
            .map_err(|_| RejectionError::Dependency)?;
        let fingerprints = proposals
            .iter()
            .map(|proposal| {
                rejection_context(proposal, &snapshot)
                    .map_or(String::new(), |context| proposal.fingerprint(&context))
            })
            .collect::<Vec<_>>();
        self.repository
            .match_rejections(&fingerprints)
            .await
            .map_err(|_| RejectionError::Dependency)
    }

    pub async fn reject_proposal(
        &self,
        proposal: CleanupReviewProposal,
    ) -> Result<CleanupRejection, RejectionError> {
        proposal.validate()?;
        let snapshot = self
            .repository
            .rejection_snapshot()
            .await
            .map_err(|_| RejectionError::Dependency)?;
        let context = rejection_context(&proposal, &snapshot).ok_or(RejectionError::Stale)?;
        self.repository
            .save_rejection(&proposal, &context)
            .await
            .map_err(|_| RejectionError::Dependency)?
            .ok_or(RejectionError::Stale)
    }

    pub async fn rejected_page(
        &self,
        before: Option<i64>,
        search: &str,
    ) -> Result<Vec<(CleanupRejection, bool)>, RejectionError> {
        if before.is_some_and(|id| id <= 0) || search.len() > 256 {
            return Err(RejectionError::Invalid);
        }
        let rows = self
            .repository
            .rejection_page(before, search)
            .await
            .map_err(|_| RejectionError::Dependency)?;
        let snapshot = self
            .repository
            .rejection_snapshot()
            .await
            .map_err(|_| RejectionError::Dependency)?;
        Ok(rows
            .into_iter()
            .map(|record| {
                let current = rejection_context(&record.proposal, &snapshot).as_deref()
                    == Some(&record.context_signature);
                (record, current)
            })
            .collect())
    }

    pub async fn restore_rejected(&self, id: i64) -> Result<CleanupReviewProposal, RejectionError> {
        if id <= 0 {
            return Err(RejectionError::Invalid);
        }
        self.repository
            .restore_rejection(id)
            .await
            .map_err(|_| RejectionError::Dependency)?
            .map(|record| record.proposal)
            .ok_or(RejectionError::Stale)
    }

    pub async fn forget_rejected(&self, id: i64) -> Result<(), RejectionError> {
        if id <= 0 {
            return Err(RejectionError::Invalid);
        }
        self.repository
            .forget_rejection(id)
            .await
            .map_err(|_| RejectionError::Dependency)
    }
}
