use std::cmp::Ordering;
use std::collections::{BTreeMap, BTreeSet};
use std::error::Error;
use std::fmt::{self, Display, Formatter};
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use music_domain::{IndexedTrack, TrackId};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use unicode_casefold::UnicodeCaseFold;
use unicode_general_category::{GeneralCategory, get_general_category};
use unicode_normalization::UnicodeNormalization;

pub const AUTOMATIC_RULE_SCHEMA: &str = "automatic-playlist/v1";
pub const AUTOMATIC_PREVIEW_SCHEMA: &str = "automatic-playlist-preview/v1";
pub const AUTOMATIC_APPLY_SCHEMA: &str = "automatic-playlist-apply/v1";
pub const MAX_PLAYLIST_ITEMS: usize = 20_000;

pub type PlaylistDependencyError = Box<dyn Error + Send + Sync>;
pub type PlaylistFuture<'a, T> =
    Pin<Box<dyn Future<Output = Result<T, PlaylistDependencyError>> + Send + 'a>>;

#[derive(Debug, Clone, Eq, PartialEq)]
pub struct PlaylistRecord {
    pub id: i64,
    pub name: String,
    pub mode_id: Option<String>,
    pub category: Option<String>,
    pub automatic_rule_json: String,
    pub automatic_source_signature: Option<String>,
    pub automatic_refreshed_at_unix_seconds: Option<i64>,
    pub created_at_unix_seconds: i64,
    pub updated_at_unix_seconds: i64,
}

impl PlaylistRecord {
    #[must_use]
    pub fn is_automatic(&self) -> bool {
        !self.automatic_rule_json.is_empty()
    }

    pub fn automatic_rule(&self) -> Result<Option<AutomaticPlaylistRule>, AutomaticRuleError> {
        if !self.is_automatic() {
            return Ok(None);
        }
        let rule = serde_json::from_str::<AutomaticPlaylistRule>(&self.automatic_rule_json)
            .map_err(|_| AutomaticRuleError::StoredRuleInvalid)?;
        rule.normalized().map(Some)
    }
}

#[derive(Debug, Clone, Eq, PartialEq)]
pub struct PlaylistCreate {
    pub name: String,
    pub mode_id: Option<String>,
    pub category: Option<String>,
}

#[derive(Debug, Clone, Eq, PartialEq)]
pub enum PatchValue<T> {
    Unchanged,
    Set(T),
}

#[derive(Debug, Clone, Eq, PartialEq)]
pub struct PlaylistPatch {
    pub name: PatchValue<String>,
    pub mode_id: PatchValue<String>,
    pub category: PatchValue<Option<String>>,
}

impl Default for PlaylistPatch {
    fn default() -> Self {
        Self {
            name: PatchValue::Unchanged,
            mode_id: PatchValue::Unchanged,
            category: PatchValue::Unchanged,
        }
    }
}

#[derive(Debug, Clone, Default, Eq, PartialEq)]
pub struct PlaylistFilter {
    pub mode_id: Option<String>,
    pub category: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct PlaylistItemRecord {
    pub position: i64,
    pub track_id: i64,
    pub track: Option<IndexedTrack>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct PlaylistItems {
    pub playlist: PlaylistRecord,
    pub items: Vec<PlaylistItemRecord>,
}

#[derive(Debug, Clone, Copy, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AutomaticMatch {
    #[default]
    Any,
    All,
}

#[derive(Debug, Clone, Copy, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AutomaticTagSources {
    #[default]
    Manual,
    ManualAndLocal,
}

#[derive(Debug, Clone, Copy, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AutomaticOrder {
    #[default]
    Title,
    Newest,
    BpmAscending,
    BpmDescending,
}

#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AutomaticPlaylistRule {
    #[serde(rename = "schema")]
    pub schema_version: String,
    #[serde(default)]
    pub include_tags: Vec<String>,
    #[serde(default)]
    pub r#match: AutomaticMatch,
    #[serde(default)]
    pub exclude_tags: Vec<String>,
    #[serde(default)]
    pub tag_sources: AutomaticTagSources,
    #[serde(default)]
    pub min_bpm: Option<u32>,
    #[serde(default)]
    pub max_bpm: Option<u32>,
    #[serde(default = "default_true")]
    pub include_unknown_bpm: bool,
    #[serde(default = "default_maximum_tracks")]
    pub maximum_tracks: u16,
    #[serde(default)]
    pub order_by: AutomaticOrder,
}

impl AutomaticPlaylistRule {
    pub fn normalized(mut self) -> Result<Self, AutomaticRuleError> {
        if self.schema_version != AUTOMATIC_RULE_SCHEMA {
            return Err(AutomaticRuleError::UnsupportedSchema);
        }
        if self.include_tags.len() > 32 || self.exclude_tags.len() > 32 {
            return Err(AutomaticRuleError::TooManyTags);
        }
        self.include_tags = normalize_tags(&self.include_tags)?;
        self.exclude_tags = normalize_tags(&self.exclude_tags)?;
        if self
            .include_tags
            .iter()
            .any(|tag| self.exclude_tags.contains(tag))
        {
            return Err(AutomaticRuleError::OverlappingTags);
        }
        if self
            .min_bpm
            .is_some_and(|value| !(1..=999).contains(&value))
            || self
                .max_bpm
                .is_some_and(|value| !(1..=999).contains(&value))
        {
            return Err(AutomaticRuleError::BpmOutOfRange);
        }
        if matches!((self.min_bpm, self.max_bpm), (Some(minimum), Some(maximum)) if minimum > maximum)
        {
            return Err(AutomaticRuleError::InvalidBpmRange);
        }
        if !(1..=1_000).contains(&self.maximum_tracks) {
            return Err(AutomaticRuleError::MaximumTracksOutOfRange);
        }
        Ok(self)
    }
}

const fn default_true() -> bool {
    true
}

const fn default_maximum_tracks() -> u16 {
    200
}

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum AutomaticRuleError {
    UnsupportedSchema,
    TooManyTags,
    BlankTag,
    TagTooLong,
    InvalidTagCharacter,
    OverlappingTags,
    BpmOutOfRange,
    InvalidBpmRange,
    MaximumTracksOutOfRange,
    StoredRuleInvalid,
    SignatureSerialization,
}

impl Display for AutomaticRuleError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::UnsupportedSchema => "automatic playlist schema is unsupported",
            Self::TooManyTags => "automatic playlist tag lists cannot exceed 32 entries",
            Self::BlankTag => "tags cannot be blank",
            Self::TagTooLong => "tags cannot exceed 64 characters",
            Self::InvalidTagCharacter => "tags cannot contain control characters",
            Self::OverlappingTags => "included and excluded tags must be disjoint",
            Self::BpmOutOfRange => "BPM bounds must be between 1 and 999",
            Self::InvalidBpmRange => "min_bpm cannot be greater than max_bpm",
            Self::MaximumTracksOutOfRange => "maximum_tracks must be between 1 and 1000",
            Self::StoredRuleInvalid => "stored automatic playlist rule is invalid",
            Self::SignatureSerialization => "automatic playlist signature could not be encoded",
        })
    }
}

impl Error for AutomaticRuleError {}

fn normalize_tags(tags: &[String]) -> Result<Vec<String>, AutomaticRuleError> {
    let mut unique = BTreeSet::new();
    let mut normalized = Vec::new();
    for tag in tags {
        let compatibility_normalized = tag.nfkc().collect::<String>();
        let folded = compatibility_normalized
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ")
            .case_fold()
            .collect::<String>();
        if folded.is_empty() {
            return Err(AutomaticRuleError::BlankTag);
        }
        if folded.chars().count() > 64 {
            return Err(AutomaticRuleError::TagTooLong);
        }
        if folded.chars().any(is_unicode_other) {
            return Err(AutomaticRuleError::InvalidTagCharacter);
        }
        if unique.insert(folded.clone()) {
            normalized.push(folded);
        }
    }
    Ok(normalized)
}

fn is_unicode_other(character: char) -> bool {
    matches!(
        get_general_category(character),
        GeneralCategory::Control
            | GeneralCategory::Format
            | GeneralCategory::PrivateUse
            | GeneralCategory::Surrogate
            | GeneralCategory::Unassigned
    )
}

#[derive(Debug, Clone, PartialEq)]
pub struct AutomaticSourceTrack {
    pub track: IndexedTrack,
    pub tags: BTreeSet<String>,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct AutomaticPlaylistSource {
    pub tracks: Vec<AutomaticSourceTrack>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct AutomaticPlaylistResolution {
    pub source_signature: String,
    pub library_tracks: usize,
    pub tracks: Vec<IndexedTrack>,
}

pub fn resolve_automatic_playlist(
    rule: &AutomaticPlaylistRule,
    mut source: AutomaticPlaylistSource,
) -> Result<AutomaticPlaylistResolution, AutomaticRuleError> {
    let rule = rule.clone().normalized()?;
    source.tracks.sort_by_key(|entry| entry.track.id);
    let source_signature = automatic_source_signature(&rule, &source)?;
    let library_tracks = source.tracks.len();
    let mut tracks = source
        .tracks
        .into_iter()
        .filter(|entry| automatic_match(entry, &rule))
        .map(|entry| entry.track)
        .collect::<Vec<_>>();
    tracks.sort_by(|left, right| automatic_order(left, right, rule.order_by));
    tracks.truncate(usize::from(rule.maximum_tracks));
    Ok(AutomaticPlaylistResolution {
        source_signature,
        library_tracks,
        tracks,
    })
}

fn automatic_match(entry: &AutomaticSourceTrack, rule: &AutomaticPlaylistRule) -> bool {
    let included = match rule.r#match {
        AutomaticMatch::Any => {
            rule.include_tags.is_empty()
                || rule.include_tags.iter().any(|tag| entry.tags.contains(tag))
        }
        AutomaticMatch::All => rule.include_tags.iter().all(|tag| entry.tags.contains(tag)),
    };
    if !included || rule.exclude_tags.iter().any(|tag| entry.tags.contains(tag)) {
        return false;
    }
    match entry.track.metadata.bpm {
        None => rule.include_unknown_bpm,
        Some(bpm) => {
            rule.min_bpm.is_none_or(|minimum| bpm >= minimum)
                && rule.max_bpm.is_none_or(|maximum| bpm <= maximum)
        }
    }
}

fn automatic_order(left: &IndexedTrack, right: &IndexedTrack, order: AutomaticOrder) -> Ordering {
    let left_title = automatic_title(left);
    let right_title = automatic_title(right);
    match order {
        AutomaticOrder::Title => left_title
            .cmp(&right_title)
            .then_with(|| left.id.cmp(&right.id)),
        AutomaticOrder::Newest => right
            .added_at_unix_seconds
            .cmp(&left.added_at_unix_seconds)
            .then_with(|| left_title.cmp(&right_title))
            .then_with(|| left.id.cmp(&right.id)),
        AutomaticOrder::BpmAscending => compare_bpm(left, right, false)
            .then_with(|| left_title.cmp(&right_title))
            .then_with(|| left.id.cmp(&right.id)),
        AutomaticOrder::BpmDescending => compare_bpm(left, right, true)
            .then_with(|| left_title.cmp(&right_title))
            .then_with(|| left.id.cmp(&right.id)),
    }
}

fn compare_bpm(left: &IndexedTrack, right: &IndexedTrack, descending: bool) -> Ordering {
    match (left.metadata.bpm, right.metadata.bpm) {
        (Some(left), Some(right)) if descending => right.cmp(&left),
        (Some(left), Some(right)) => left.cmp(&right),
        (Some(_), None) => Ordering::Less,
        (None, Some(_)) => Ordering::Greater,
        (None, None) => Ordering::Equal,
    }
}

fn automatic_title(track: &IndexedTrack) -> String {
    let title = if !track.display_title.is_empty() {
        &track.display_title
    } else if !track.metadata.title.is_empty() {
        &track.metadata.title
    } else {
        track.path.as_str()
    };
    title.case_fold().collect()
}

fn automatic_source_signature(
    rule: &AutomaticPlaylistRule,
    source: &AutomaticPlaylistSource,
) -> Result<String, AutomaticRuleError> {
    let rule_value =
        serde_json::to_value(rule).map_err(|_| AutomaticRuleError::SignatureSerialization)?;
    let tracks = source
        .tracks
        .iter()
        .map(|entry| {
            let track = &entry.track;
            BTreeMap::from([
                (
                    "bpm",
                    track
                        .metadata
                        .bpm
                        .map_or(Value::Null, |value| Value::from(u64::from(value))),
                ),
                ("display_title", Value::String(track.display_title.clone())),
                ("id", Value::from(track.id.get())),
                ("mtime", Value::from(track.mtime_unix_seconds)),
                ("path", Value::String(track.path.as_str().to_owned())),
                ("size_bytes", Value::from(track.size_bytes)),
                (
                    "tags",
                    Value::Array(entry.tags.iter().cloned().map(Value::String).collect()),
                ),
                ("title", Value::String(track.metadata.title.clone())),
            ])
        })
        .collect::<Vec<_>>();
    let payload = BTreeMap::from([
        ("rule", rule_value),
        (
            "tracks",
            serde_json::to_value(tracks).map_err(|_| AutomaticRuleError::SignatureSerialization)?,
        ),
    ]);
    let encoded =
        serde_json::to_vec(&payload).map_err(|_| AutomaticRuleError::SignatureSerialization)?;
    Ok(format!("{:x}", Sha256::digest(encoded)))
}

#[derive(Debug, Clone, PartialEq)]
pub enum PlaylistMutation<T> {
    Applied(T),
    PlaylistNotFound,
    TrackNotFound,
    PositionOutOfRange,
    AutomaticItemsManaged,
    CapacityExceeded,
}

#[derive(Debug, Clone, PartialEq)]
pub enum AutomaticMaterialization {
    Applied {
        playlist: PlaylistRecord,
        resolution: AutomaticPlaylistResolution,
    },
    Unchanged {
        playlist: PlaylistRecord,
        resolution: AutomaticPlaylistResolution,
    },
    PlaylistNotFound,
    StalePreview,
    RuleChanged,
}

pub trait PlaylistRepository: std::fmt::Debug + Send + Sync {
    fn create<'a>(&'a self, request: &'a PlaylistCreate) -> PlaylistFuture<'a, PlaylistRecord>;
    fn list<'a>(&'a self, filter: &'a PlaylistFilter) -> PlaylistFuture<'a, Vec<PlaylistRecord>>;
    fn get(&self, playlist_id: i64) -> PlaylistFuture<'_, Option<PlaylistRecord>>;
    fn update<'a>(
        &'a self,
        playlist_id: i64,
        patch: &'a PlaylistPatch,
    ) -> PlaylistFuture<'a, Option<PlaylistRecord>>;
    fn delete(&self, playlist_id: i64) -> PlaylistFuture<'_, bool>;
    fn items(&self, playlist_id: i64) -> PlaylistFuture<'_, Option<PlaylistItems>>;
    fn add_track(
        &self,
        playlist_id: i64,
        track_id: TrackId,
        position: Option<usize>,
    ) -> PlaylistFuture<'_, PlaylistMutation<PlaylistItemRecord>>;
    fn remove_track(
        &self,
        playlist_id: i64,
        position: usize,
    ) -> PlaylistFuture<'_, PlaylistMutation<()>>;
    fn move_track(
        &self,
        playlist_id: i64,
        from_position: usize,
        to_position: usize,
    ) -> PlaylistFuture<'_, PlaylistMutation<()>>;
    fn automatic_source(
        &self,
        tag_sources: AutomaticTagSources,
    ) -> PlaylistFuture<'_, AutomaticPlaylistSource>;
    fn materialize_automatic<'a>(
        &'a self,
        playlist_id: i64,
        rule: &'a AutomaticPlaylistRule,
        expected_source_signature: Option<&'a str>,
        force: bool,
    ) -> PlaylistFuture<'a, AutomaticMaterialization>;
    fn disable_automatic(&self, playlist_id: i64) -> PlaylistFuture<'_, Option<PlaylistRecord>>;
}

#[derive(Debug)]
pub enum PlaylistServiceError {
    NotFound,
    TrackNotFound,
    PositionOutOfRange,
    AutomaticItemsManaged,
    NotAutomatic,
    StalePreview,
    CapacityExceeded,
    ConcurrentChange,
    InvalidRule(AutomaticRuleError),
    Dependency(PlaylistDependencyError),
}

impl Display for PlaylistServiceError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotFound => formatter.write_str("playlist not found"),
            Self::TrackNotFound => formatter.write_str("track not in library"),
            Self::PositionOutOfRange => formatter.write_str("playlist position out of range"),
            Self::AutomaticItemsManaged => {
                formatter.write_str("automatic playlist items are managed by its rule")
            }
            Self::NotAutomatic => formatter.write_str("playlist does not have an automatic rule"),
            Self::StalePreview => formatter.write_str("automatic playlist preview is stale"),
            Self::CapacityExceeded => formatter.write_str("playlist item capacity exceeded"),
            Self::ConcurrentChange => formatter.write_str("playlist changed during the request"),
            Self::InvalidRule(error) => Display::fmt(error, formatter),
            Self::Dependency(_) => formatter.write_str("playlist repository operation failed"),
        }
    }
}

impl Error for PlaylistServiceError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::InvalidRule(source) => Some(source),
            Self::Dependency(source) => Some(source.as_ref()),
            _ => None,
        }
    }
}

impl From<PlaylistDependencyError> for PlaylistServiceError {
    fn from(error: PlaylistDependencyError) -> Self {
        Self::Dependency(error)
    }
}

impl From<AutomaticRuleError> for PlaylistServiceError {
    fn from(error: AutomaticRuleError) -> Self {
        Self::InvalidRule(error)
    }
}

#[derive(Debug, Clone)]
pub struct PlaylistService {
    repository: Arc<dyn PlaylistRepository>,
}

impl PlaylistService {
    #[must_use]
    pub fn new(repository: Arc<dyn PlaylistRepository>) -> Self {
        Self { repository }
    }

    pub async fn create(
        &self,
        request: &PlaylistCreate,
    ) -> Result<PlaylistRecord, PlaylistServiceError> {
        self.repository.create(request).await.map_err(Into::into)
    }

    pub async fn list(
        &self,
        filter: &PlaylistFilter,
    ) -> Result<Vec<PlaylistRecord>, PlaylistServiceError> {
        self.repository.list(filter).await.map_err(Into::into)
    }

    pub async fn get(&self, playlist_id: i64) -> Result<PlaylistRecord, PlaylistServiceError> {
        self.repository
            .get(playlist_id)
            .await?
            .ok_or(PlaylistServiceError::NotFound)
    }

    pub async fn update(
        &self,
        playlist_id: i64,
        patch: &PlaylistPatch,
    ) -> Result<PlaylistRecord, PlaylistServiceError> {
        self.repository
            .update(playlist_id, patch)
            .await?
            .ok_or(PlaylistServiceError::NotFound)
    }

    pub async fn delete(&self, playlist_id: i64) -> Result<(), PlaylistServiceError> {
        self.repository
            .delete(playlist_id)
            .await?
            .then_some(())
            .ok_or(PlaylistServiceError::NotFound)
    }

    pub async fn items(&self, playlist_id: i64) -> Result<PlaylistItems, PlaylistServiceError> {
        for _ in 0..2 {
            let playlist = self.get(playlist_id).await?;
            if playlist.is_automatic() {
                match playlist.automatic_rule() {
                    Ok(Some(rule)) => {
                        match self
                            .repository
                            .materialize_automatic(playlist_id, &rule, None, false)
                            .await?
                        {
                            AutomaticMaterialization::RuleChanged => {
                                tokio::task::yield_now().await;
                                continue;
                            }
                            AutomaticMaterialization::PlaylistNotFound => {
                                return Err(PlaylistServiceError::NotFound);
                            }
                            AutomaticMaterialization::Applied { .. }
                            | AutomaticMaterialization::Unchanged { .. } => {}
                            AutomaticMaterialization::StalePreview => {
                                return Err(PlaylistServiceError::ConcurrentChange);
                            }
                        }
                    }
                    Ok(None) => {}
                    Err(_) => {
                        // A damaged stored rule must not make the last successfully
                        // materialized rows unavailable to playback or export.
                    }
                }
            }
            break;
        }
        self.repository
            .items(playlist_id)
            .await?
            .ok_or(PlaylistServiceError::NotFound)
    }

    pub async fn track_ids(&self, playlist_id: i64) -> Result<Vec<TrackId>, PlaylistServiceError> {
        let items = self.items(playlist_id).await?;
        Ok(items
            .items
            .into_iter()
            .filter_map(|item| item.track.map(|track| track.id))
            .collect())
    }

    pub async fn add_track(
        &self,
        playlist_id: i64,
        track_id: TrackId,
        position: Option<usize>,
    ) -> Result<PlaylistItemRecord, PlaylistServiceError> {
        map_mutation(
            self.repository
                .add_track(playlist_id, track_id, position)
                .await?,
        )
    }

    pub async fn remove_track(
        &self,
        playlist_id: i64,
        position: usize,
    ) -> Result<(), PlaylistServiceError> {
        map_mutation(self.repository.remove_track(playlist_id, position).await?)
    }

    pub async fn move_track(
        &self,
        playlist_id: i64,
        from_position: usize,
        to_position: usize,
    ) -> Result<(), PlaylistServiceError> {
        map_mutation(
            self.repository
                .move_track(playlist_id, from_position, to_position)
                .await?,
        )
    }

    pub async fn preview(
        &self,
        playlist_id: i64,
        rule: AutomaticPlaylistRule,
    ) -> Result<AutomaticPlaylistResolution, PlaylistServiceError> {
        let _ = self.get(playlist_id).await?;
        let rule = rule.normalized()?;
        let source = self.repository.automatic_source(rule.tag_sources).await?;
        resolve_automatic_playlist(&rule, source).map_err(Into::into)
    }

    pub async fn configure(
        &self,
        playlist_id: i64,
        rule: AutomaticPlaylistRule,
        expected_source_signature: &str,
    ) -> Result<(PlaylistRecord, AutomaticPlaylistResolution), PlaylistServiceError> {
        let rule = rule.normalized()?;
        match self
            .repository
            .materialize_automatic(playlist_id, &rule, Some(expected_source_signature), true)
            .await?
        {
            AutomaticMaterialization::Applied {
                playlist,
                resolution,
            }
            | AutomaticMaterialization::Unchanged {
                playlist,
                resolution,
            } => Ok((playlist, resolution)),
            AutomaticMaterialization::PlaylistNotFound => Err(PlaylistServiceError::NotFound),
            AutomaticMaterialization::StalePreview => Err(PlaylistServiceError::StalePreview),
            AutomaticMaterialization::RuleChanged => Err(PlaylistServiceError::ConcurrentChange),
        }
    }

    pub async fn refresh(
        &self,
        playlist_id: i64,
    ) -> Result<(PlaylistRecord, AutomaticPlaylistResolution), PlaylistServiceError> {
        for _ in 0..2 {
            let playlist = self.get(playlist_id).await?;
            let rule = playlist
                .automatic_rule()?
                .ok_or(PlaylistServiceError::NotAutomatic)?;
            match self
                .repository
                .materialize_automatic(playlist_id, &rule, None, true)
                .await?
            {
                AutomaticMaterialization::Applied {
                    playlist,
                    resolution,
                }
                | AutomaticMaterialization::Unchanged {
                    playlist,
                    resolution,
                } => return Ok((playlist, resolution)),
                AutomaticMaterialization::PlaylistNotFound => {
                    return Err(PlaylistServiceError::NotFound);
                }
                AutomaticMaterialization::StalePreview => {
                    return Err(PlaylistServiceError::StalePreview);
                }
                AutomaticMaterialization::RuleChanged => tokio::task::yield_now().await,
            }
        }
        Err(PlaylistServiceError::ConcurrentChange)
    }

    pub async fn disable_automatic(
        &self,
        playlist_id: i64,
    ) -> Result<PlaylistRecord, PlaylistServiceError> {
        self.repository
            .disable_automatic(playlist_id)
            .await?
            .ok_or(PlaylistServiceError::NotFound)
    }
}

fn map_mutation<T>(outcome: PlaylistMutation<T>) -> Result<T, PlaylistServiceError> {
    match outcome {
        PlaylistMutation::Applied(value) => Ok(value),
        PlaylistMutation::PlaylistNotFound => Err(PlaylistServiceError::NotFound),
        PlaylistMutation::TrackNotFound => Err(PlaylistServiceError::TrackNotFound),
        PlaylistMutation::PositionOutOfRange => Err(PlaylistServiceError::PositionOutOfRange),
        PlaylistMutation::AutomaticItemsManaged => Err(PlaylistServiceError::AutomaticItemsManaged),
        PlaylistMutation::CapacityExceeded => Err(PlaylistServiceError::CapacityExceeded),
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use music_domain::{LibraryPath, TrackMetadata};

    use super::*;

    fn track(
        id: i64,
        title: &str,
        bpm: Option<u32>,
        added_at: i64,
        tags: &[&str],
    ) -> Result<AutomaticSourceTrack, Box<dyn Error>> {
        Ok(AutomaticSourceTrack {
            track: IndexedTrack {
                id: TrackId::new(id)?,
                path: LibraryPath::parse(format!("{title}.mp3"))?,
                metadata: TrackMetadata {
                    title: title.to_owned(),
                    artist: String::new(),
                    album_artist: String::new(),
                    album: String::new(),
                    track_no: None,
                    disc_no: None,
                    year: None,
                    genre: String::new(),
                    bpm,
                },
                duration: Duration::from_secs(60),
                display_title: title.to_owned(),
                origin: String::new(),
                size_bytes: 10,
                mtime_unix_seconds: 20,
                added_at_unix_seconds: added_at,
            },
            tags: tags.iter().map(|tag| (*tag).to_owned()).collect(),
        })
    }

    fn rule() -> AutomaticPlaylistRule {
        AutomaticPlaylistRule {
            schema_version: AUTOMATIC_RULE_SCHEMA.to_owned(),
            include_tags: vec![" Calm  ".to_owned()],
            r#match: AutomaticMatch::Any,
            exclude_tags: Vec::new(),
            tag_sources: AutomaticTagSources::Manual,
            min_bpm: None,
            max_bpm: Some(100),
            include_unknown_bpm: true,
            maximum_tracks: 200,
            order_by: AutomaticOrder::Title,
        }
    }

    #[test]
    fn rules_normalize_and_reject_ambiguous_filters() -> Result<(), Box<dyn Error>> {
        let mut source = rule();
        source.include_tags = vec![" Calm  ".to_owned(), "MASSE".to_owned()];
        source.exclude_tags = vec!["Maße".to_owned()];
        assert_eq!(
            source.normalized(),
            Err(AutomaticRuleError::OverlappingTags)
        );
        let normalized = rule().normalized()?;
        assert_eq!(normalized.include_tags, ["calm"]);
        let mut invalid = normalized;
        invalid.exclude_tags = vec!["CALM".to_owned()];
        assert_eq!(
            invalid.normalized(),
            Err(AutomaticRuleError::OverlappingTags)
        );
        Ok(())
    }

    #[test]
    fn resolution_is_deterministic_and_bpm_unknowns_follow_the_rule() -> Result<(), Box<dyn Error>>
    {
        let source = AutomaticPlaylistSource {
            tracks: vec![
                track(3, "Zulu", Some(120), 3, &["calm"])?,
                track(2, "Alpha", None, 2, &["calm"])?,
                track(1, "Beta", Some(90), 1, &["calm"])?,
            ],
        };
        let first = resolve_automatic_playlist(&rule(), source.clone())?;
        let second = resolve_automatic_playlist(&rule(), source)?;
        assert_eq!(first.source_signature, second.source_signature);
        assert_eq!(
            first.source_signature,
            "eb2326faf5d07d8c9b1dccdddd82830a0c7e497c5d0fa653a31befaa64e6a152"
        );
        assert_eq!(first.library_tracks, 3);
        assert_eq!(
            first
                .tracks
                .iter()
                .map(|track| track.id.get())
                .collect::<Vec<_>>(),
            [2, 1]
        );
        Ok(())
    }

    #[test]
    fn title_order_uses_full_unicode_case_folding() -> Result<(), Box<dyn Error>> {
        let source = AutomaticPlaylistSource {
            tracks: vec![
                track(2, "MASSE", Some(90), 2, &["calm"])?,
                track(1, "Maße", Some(90), 1, &["calm"])?,
            ],
        };
        let resolution = resolve_automatic_playlist(&rule(), source)?;
        assert_eq!(
            resolution
                .tracks
                .iter()
                .map(|track| track.id.get())
                .collect::<Vec<_>>(),
            [1, 2]
        );
        Ok(())
    }
}
