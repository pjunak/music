use std::collections::{BTreeMap, BTreeSet};
use std::error::Error;
use std::fmt::{self, Display, Formatter};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use super::{RenameTagOutcome, TagUsage, normalize_manual_tag};

pub const TAG_VOCABULARY_SCHEMA: &str = "assistant-tag-vocabulary/v1";
pub const TAG_CLEANUP_PREVIEW_SCHEMA: &str = "assistant-tag-cleanup-preview/v2";
pub const TAG_CLEANUP_APPLY_SCHEMA: &str = "assistant-tag-cleanup-apply/v1";
pub const TAG_VOCABULARY_SEED_VERSION: u32 = 5;
const MAX_VOCABULARY_TAGS: usize = 200;

#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TagVocabularyEntry {
    pub id: String,
    pub name: String,
    pub description: String,
    #[serde(default)]
    pub aliases: Vec<String>,
    #[serde(default)]
    pub context_cues: Vec<String>,
}

#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TagVocabularyGroup {
    pub key: String,
    pub label: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub tags: Vec<TagVocabularyEntry>,
}

#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TagVocabularyDocument {
    pub schema_version: String,
    pub groups: Vec<TagVocabularyGroup>,
}

#[derive(Debug, Clone, Eq, PartialEq)]
pub struct TagVocabularyRecord {
    pub revision: u32,
    pub seed_version: u32,
    pub document: TagVocabularyDocument,
}

#[derive(Debug, Clone, Eq, PartialEq)]
pub struct TagVocabularySnapshot {
    pub revision: u32,
    pub fingerprint: String,
    pub document: TagVocabularyDocument,
}

impl TagVocabularySnapshot {
    pub fn entries(&self) -> impl Iterator<Item = &TagVocabularyEntry> {
        self.document.groups.iter().flat_map(|group| &group.tags)
    }
}

#[derive(Debug, Clone, Eq, PartialEq)]
pub enum VocabularyError {
    InvalidSchema,
    InvalidGroupCount,
    InvalidGroupKey,
    InvalidGroupLabel,
    InvalidGroupDescription,
    InvalidTagCount,
    InvalidTagId,
    InvalidTagName,
    InvalidTagDescription,
    TooManyAliases,
    TooManyContextCues,
    DuplicateGroup,
    DuplicateTagId,
    DuplicateTagName,
    AliasConflictsWithName(String),
    DuplicateAlias(String),
    InvalidSeed,
    Serialization,
}

impl Display for VocabularyError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidSchema => formatter.write_str("tag vocabulary schema is unsupported"),
            Self::InvalidGroupCount => {
                formatter.write_str("the vocabulary must contain between 1 and 20 groups")
            }
            Self::InvalidGroupKey => formatter.write_str("a vocabulary group key is invalid"),
            Self::InvalidGroupLabel => formatter.write_str("a vocabulary group label is invalid"),
            Self::InvalidGroupDescription => {
                formatter.write_str("a vocabulary group description is too long")
            }
            Self::InvalidTagCount => formatter.write_str(
                "the vocabulary must contain between 1 and 200 tags and no group may exceed 100 tags",
            ),
            Self::InvalidTagId => formatter.write_str("a vocabulary tag ID is invalid"),
            Self::InvalidTagName => formatter.write_str("a vocabulary tag name is invalid"),
            Self::InvalidTagDescription => {
                formatter.write_str("a vocabulary tag description is invalid")
            }
            Self::TooManyAliases => formatter.write_str("a vocabulary tag has too many aliases"),
            Self::TooManyContextCues => {
                formatter.write_str("a vocabulary tag has too many context cues")
            }
            Self::DuplicateGroup => formatter.write_str("vocabulary group keys must be unique"),
            Self::DuplicateTagId => formatter.write_str("vocabulary tag IDs must be unique"),
            Self::DuplicateTagName => formatter.write_str("vocabulary tag names must be unique"),
            Self::AliasConflictsWithName(alias) => write!(
                formatter,
                "alias '{alias}' conflicts with a canonical tag name"
            ),
            Self::DuplicateAlias(alias) => {
                write!(formatter, "alias '{alias}' belongs to multiple vocabulary tags")
            }
            Self::InvalidSeed => formatter.write_str("the bundled tag vocabulary is invalid"),
            Self::Serialization => formatter.write_str("the tag vocabulary could not be encoded"),
        }
    }
}

impl Error for VocabularyError {}

impl TagVocabularyDocument {
    pub fn normalized(mut self) -> Result<Self, VocabularyError> {
        if self.schema_version != TAG_VOCABULARY_SCHEMA {
            return Err(VocabularyError::InvalidSchema);
        }
        if !(1..=20).contains(&self.groups.len()) {
            return Err(VocabularyError::InvalidGroupCount);
        }
        let mut group_keys = BTreeSet::new();
        let mut tag_ids = BTreeSet::new();
        let mut tag_names = BTreeSet::new();
        let mut tag_count = 0usize;
        for group in &mut self.groups {
            group.key = group.key.trim().to_owned();
            group.label = collapse_whitespace(&group.label);
            group.description = collapse_whitespace(&group.description);
            if !valid_identifier(&group.key, 32, false) {
                return Err(VocabularyError::InvalidGroupKey);
            }
            if group.label.is_empty() || group.label.chars().count() > 64 {
                return Err(VocabularyError::InvalidGroupLabel);
            }
            if group.description.chars().count() > 300 {
                return Err(VocabularyError::InvalidGroupDescription);
            }
            if !group_keys.insert(group.key.clone()) {
                return Err(VocabularyError::DuplicateGroup);
            }
            if group.tags.len() > 100 {
                return Err(VocabularyError::InvalidTagCount);
            }
            for tag in &mut group.tags {
                tag.id = tag.id.trim().to_owned();
                tag.name =
                    normalize_manual_tag(&tag.name).map_err(|_| VocabularyError::InvalidTagName)?;
                tag.description = collapse_whitespace(&tag.description);
                if !valid_identifier(&tag.id, 64, true) {
                    return Err(VocabularyError::InvalidTagId);
                }
                if !(2..=300).contains(&tag.description.chars().count()) {
                    return Err(VocabularyError::InvalidTagDescription);
                }
                if tag.aliases.len() > 24 {
                    return Err(VocabularyError::TooManyAliases);
                }
                if tag.context_cues.len() > 32 {
                    return Err(VocabularyError::TooManyContextCues);
                }
                tag.aliases = normalize_terms(&tag.aliases)?;
                tag.context_cues = normalize_terms(&tag.context_cues)?;
                if tag.aliases.contains(&tag.name) {
                    return Err(VocabularyError::AliasConflictsWithName(tag.name.clone()));
                }
                if !tag_ids.insert(tag.id.clone()) {
                    return Err(VocabularyError::DuplicateTagId);
                }
                if !tag_names.insert(tag.name.clone()) {
                    return Err(VocabularyError::DuplicateTagName);
                }
                tag_count += 1;
            }
        }
        if !(1..=MAX_VOCABULARY_TAGS).contains(&tag_count) {
            return Err(VocabularyError::InvalidTagCount);
        }
        let mut alias_owners = BTreeMap::<String, String>::new();
        for tag in self.groups.iter().flat_map(|group| &group.tags) {
            for alias in &tag.aliases {
                if tag_names.contains(alias) {
                    return Err(VocabularyError::AliasConflictsWithName(alias.clone()));
                }
                if alias_owners
                    .insert(alias.clone(), tag.name.clone())
                    .is_some()
                {
                    return Err(VocabularyError::DuplicateAlias(alias.clone()));
                }
            }
        }
        Ok(self)
    }
}

fn collapse_whitespace(value: &str) -> String {
    value.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn valid_identifier(value: &str, maximum: usize, allow_dot: bool) -> bool {
    let bytes = value.as_bytes();
    if !(2..=maximum).contains(&bytes.len())
        || !bytes[0].is_ascii_lowercase() && !bytes[0].is_ascii_digit()
    {
        return false;
    }
    bytes.iter().all(|byte| {
        byte.is_ascii_lowercase()
            || byte.is_ascii_digit()
            || *byte == b'_'
            || *byte == b'-'
            || allow_dot && *byte == b'.'
    })
}

fn normalize_terms(values: &[String]) -> Result<Vec<String>, VocabularyError> {
    let mut seen = BTreeSet::new();
    let mut result = Vec::new();
    for value in values {
        let normalized =
            normalize_manual_tag(value).map_err(|_| VocabularyError::InvalidTagName)?;
        if seen.insert(normalized.clone()) {
            result.push(normalized);
        }
    }
    Ok(result)
}

pub fn vocabulary_fingerprint(document: &TagVocabularyDocument) -> Result<String, VocabularyError> {
    // Python's persisted snapshots use sorted JSON object keys. Serializing through
    // `Value` preserves that canonical ordering with serde_json's default map type,
    // keeping fingerprints stable across the migration boundary.
    let canonical = serde_json::to_value(document).map_err(|_| VocabularyError::Serialization)?;
    let encoded = serde_json::to_vec(&canonical).map_err(|_| VocabularyError::Serialization)?;
    Ok(format!("{:x}", Sha256::digest(encoded)))
}

pub fn default_vocabulary() -> Result<TagVocabularyDocument, VocabularyError> {
    serde_json::from_str::<TagVocabularyDocument>(include_str!("../default_vocabulary.json"))
        .map_err(|_| VocabularyError::InvalidSeed)?
        .normalized()
        .map_err(|_| VocabularyError::InvalidSeed)
}

#[derive(Debug, Clone, Eq, PartialEq)]
pub enum CleanupSuggestionReason {
    VocabularyAlias,
    VocabularyPlural,
    VocabularyTypo,
}

impl CleanupSuggestionReason {
    #[must_use]
    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::VocabularyAlias => "vocabulary_alias",
            Self::VocabularyPlural => "vocabulary_plural",
            Self::VocabularyTypo => "vocabulary_typo",
        }
    }
}

#[derive(Debug, Clone, Eq, PartialEq)]
pub struct CleanupSuggestion {
    pub id: String,
    pub source: String,
    pub target: String,
    pub reason_code: CleanupSuggestionReason,
    pub reason: String,
    pub source_track_count: u64,
    pub target_track_count: u64,
    pub merged: bool,
}

#[derive(Debug, Clone, Eq, PartialEq)]
pub struct CleanupPreview {
    pub catalog_signature: String,
    pub vocabulary_fingerprint: String,
    pub suggestions: Vec<CleanupSuggestion>,
}

#[derive(Debug, Clone, Eq, PartialEq, Ord, PartialOrd)]
pub struct CleanupSelection {
    pub source: String,
    pub target: String,
}

#[derive(Debug, Clone, Eq, PartialEq)]
pub struct CleanupApplyOutcome {
    pub applied: Vec<RenameTagOutcome>,
    pub catalog_signature: String,
}

#[derive(Debug, Clone, Eq, PartialEq)]
pub enum CleanupMutation {
    Applied(CleanupApplyOutcome),
    StaleCatalog,
    StaleVocabulary,
    InvalidSelection,
}

pub fn catalog_signature(usage: &[TagUsage]) -> Result<String, VocabularyError> {
    let payload = serde_json::json!({
        "schema": TAG_CLEANUP_PREVIEW_SCHEMA,
        "usage": usage.iter().map(|item| serde_json::json!([item.tag, item.track_count])).collect::<Vec<_>>(),
    });
    let encoded = serde_json::to_vec(&payload).map_err(|_| VocabularyError::Serialization)?;
    Ok(format!("{:x}", Sha256::digest(encoded)))
}

pub fn build_cleanup_preview(
    usage: &[TagUsage],
    vocabulary: &TagVocabularySnapshot,
) -> Result<CleanupPreview, VocabularyError> {
    let counts = usage
        .iter()
        .map(|item| (item.tag.as_str(), item.track_count))
        .collect::<BTreeMap<_, _>>();
    let canonical = vocabulary
        .entries()
        .map(|entry| entry.name.as_str())
        .collect::<Vec<_>>();
    let canonical_set = canonical.iter().copied().collect::<BTreeSet<_>>();
    let aliases = vocabulary
        .entries()
        .flat_map(|entry| {
            entry
                .aliases
                .iter()
                .map(move |alias| (alias.as_str(), entry.name.as_str()))
        })
        .collect::<BTreeMap<_, _>>();
    let mut suggestions = Vec::new();
    for (source, source_count) in &counts {
        if canonical_set.contains(source) {
            continue;
        }
        let (candidates, reason_code, reason) = if let Some(target) = aliases.get(source) {
            (
                vec![*target],
                CleanupSuggestionReason::VocabularyAlias,
                "Matches an alias defined in the controlled vocabulary.",
            )
        } else if let Some(target) = source
            .strip_suffix('s')
            .filter(|target| canonical_set.contains(target))
        {
            (
                vec![target],
                CleanupSuggestionReason::VocabularyPlural,
                "Matches the plural form of a canonical tag.",
            )
        } else {
            (
                canonical
                    .iter()
                    .copied()
                    .filter(|target| is_single_edit(source, target))
                    .collect::<Vec<_>>(),
                CleanupSuggestionReason::VocabularyTypo,
                "One clear spelling edit from a canonical tag.",
            )
        };
        if candidates.len() != 1 {
            continue;
        }
        let target = candidates[0];
        let target_count = counts.get(target).copied().unwrap_or_default();
        let id_payload = format!(
            "{TAG_CLEANUP_PREVIEW_SCHEMA}\0{}\0{source}\0{target}\0{}",
            vocabulary.fingerprint,
            reason_code.as_str(),
        );
        suggestions.push(CleanupSuggestion {
            id: format!("{:x}", Sha256::digest(id_payload.as_bytes())),
            source: (*source).to_owned(),
            target: target.to_owned(),
            reason_code,
            reason: reason.to_owned(),
            source_track_count: *source_count,
            target_track_count: target_count,
            merged: target_count > 0,
        });
    }
    Ok(CleanupPreview {
        catalog_signature: catalog_signature(usage)?,
        vocabulary_fingerprint: vocabulary.fingerprint.clone(),
        suggestions,
    })
}

fn is_single_edit(source: &str, target: &str) -> bool {
    let source = source.chars().collect::<Vec<_>>();
    let target = target.chars().collect::<Vec<_>>();
    if source == target || source.len().abs_diff(target.len()) > 1 {
        return false;
    }
    if source.len() == target.len() {
        let mismatches = source
            .iter()
            .zip(&target)
            .enumerate()
            .filter_map(|(index, (left, right))| (left != right).then_some(index))
            .collect::<Vec<_>>();
        return mismatches.len() == 1
            || mismatches.len() == 2
                && mismatches[1] == mismatches[0] + 1
                && source[mismatches[0]] == target[mismatches[1]]
                && source[mismatches[1]] == target[mismatches[0]];
    }
    let (shorter, longer) = if source.len() < target.len() {
        (&source, &target)
    } else {
        (&target, &source)
    };
    let (mut short_index, mut long_index, mut skipped) = (0usize, 0usize, false);
    while short_index < shorter.len() && long_index < longer.len() {
        if shorter[short_index] == longer[long_index] {
            short_index += 1;
            long_index += 1;
        } else if skipped {
            return false;
        } else {
            skipped = true;
            long_index += 1;
        }
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bundled_vocabulary_is_complete_and_stable() -> Result<(), Box<dyn Error>> {
        let document = default_vocabulary()?;
        assert_eq!(document.groups.len(), 4);
        assert_eq!(
            document
                .groups
                .iter()
                .map(|group| group.tags.len())
                .sum::<usize>(),
            138
        );
        assert_eq!(
            vocabulary_fingerprint(&document)?,
            "8c48b7559b1e9651555301d956b76db083774901a8447259e4c7c755fe69ebb0"
        );
        Ok(())
    }

    #[test]
    fn cleanup_only_proposes_unambiguous_vocabulary_repairs() -> Result<(), Box<dyn Error>> {
        let document = default_vocabulary()?;
        let snapshot = TagVocabularySnapshot {
            revision: 1,
            fingerprint: vocabulary_fingerprint(&document)?,
            document,
        };
        let preview = build_cleanup_preview(
            &[
                TagUsage {
                    tag: "inn".to_owned(),
                    track_count: 2,
                },
                TagUsage {
                    tag: "calms".to_owned(),
                    track_count: 1,
                },
                TagUsage {
                    tag: "unknown".to_owned(),
                    track_count: 4,
                },
            ],
            &snapshot,
        )?;
        assert!(
            preview
                .suggestions
                .iter()
                .any(|item| item.source == "inn" && item.target == "tavern")
        );
        assert!(
            preview
                .suggestions
                .iter()
                .any(|item| item.source == "calms" && item.target == "calm")
        );
        assert!(
            !preview
                .suggestions
                .iter()
                .any(|item| item.source == "unknown")
        );
        Ok(())
    }
}
