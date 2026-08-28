use std::collections::{BTreeMap, BTreeSet};

use regex::{Captures, Regex, regex};
use unicode_normalization::{UnicodeNormalization, char::is_combining_mark};

use crate::{IndexedTrack, TrackId};

pub type NameVerdicts = BTreeMap<String, (i32, i32)>;

#[derive(Debug, Clone, Copy, Eq, PartialEq, Ord, PartialOrd, Hash)]
#[repr(u8)]
pub enum CleanupRule {
    StripTrackNumbers,
    StripArtist,
    StripAlbum,
    StripJunk,
    NormalizeSeparators,
    NormalizeCase,
    TagTitle,
    TagArtist,
    TagAlbum,
    TagNumber,
    TagYear,
    RenameFolders,
}

impl CleanupRule {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::StripTrackNumbers => "strip_track_numbers",
            Self::StripArtist => "strip_artist",
            Self::StripAlbum => "strip_album",
            Self::StripJunk => "strip_junk",
            Self::NormalizeSeparators => "normalize_separators",
            Self::NormalizeCase => "normalize_case",
            Self::TagTitle => "tag_title",
            Self::TagArtist => "tag_artist",
            Self::TagAlbum => "tag_album",
            Self::TagNumber => "tag_number",
            Self::TagYear => "tag_year",
            Self::RenameFolders => "rename_folders",
        }
    }
}

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub struct CleanupRuleSet(u16);

impl CleanupRuleSet {
    #[must_use]
    pub const fn contains(self, rule: CleanupRule) -> bool {
        self.0 & (1_u16 << rule as u8) != 0
    }
}

impl FromIterator<CleanupRule> for CleanupRuleSet {
    fn from_iter<T: IntoIterator<Item = CleanupRule>>(rules: T) -> Self {
        Self(
            rules
                .into_iter()
                .fold(0_u16, |bits, rule| bits | (1_u16 << rule as u8)),
        )
    }
}

pub const ALL_CLEANUP_RULES: CleanupRuleSet = CleanupRuleSet((1_u16 << 12) - 1);
pub const DEFAULT_CLEANUP_RULES: CleanupRuleSet =
    CleanupRuleSet(ALL_CLEANUP_RULES.0 & !(1_u16 << CleanupRule::NormalizeCase as u8));

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum CleanupConfidence {
    High,
    Low,
}

impl CleanupConfidence {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::High => "high",
            Self::Low => "low",
        }
    }
}

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum CleanupSuggestionKind {
    Rename,
    Tag,
}

impl CleanupSuggestionKind {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Rename => "rename",
            Self::Tag => "tag",
        }
    }
}

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum CleanupTagField {
    Title,
    Artist,
    Album,
    TrackNumber,
    DiscNumber,
    Year,
}

impl CleanupTagField {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Title => "title",
            Self::Artist => "artist",
            Self::Album => "album",
            Self::TrackNumber => "track_no",
            Self::DiscNumber => "disc_no",
            Self::Year => "year",
        }
    }
}

#[derive(Debug, Clone, Eq, PartialEq)]
pub enum CleanupValue {
    Text(String),
    Number(u32),
}

#[derive(Debug, Clone, Eq, PartialEq)]
pub struct CleanupSuggestion {
    pub track_id: TrackId,
    pub kind: CleanupSuggestionKind,
    pub field: Option<CleanupTagField>,
    pub old: Option<CleanupValue>,
    pub new: Option<CleanupValue>,
    pub rules: Vec<String>,
    pub confidence: CleanupConfidence,
    pub verified: bool,
}

impl CleanupSuggestion {
    #[must_use]
    pub fn operation_id(&self) -> String {
        match self.kind {
            CleanupSuggestionKind::Rename => format!("{}:rename", self.track_id.get()),
            CleanupSuggestionKind::Tag => format!(
                "{}:tag:{}",
                self.track_id.get(),
                self.field.map_or("", CleanupTagField::as_str)
            ),
        }
    }
}

#[derive(Debug, Clone, Eq, PartialEq)]
pub struct CleanupTrackPlan {
    pub track_id: TrackId,
    pub path: String,
    pub operations: Vec<CleanupSuggestion>,
    pub notes: Vec<String>,
    pub wants_lookup: Vec<String>,
}

#[derive(Debug, Clone, Eq, PartialEq)]
pub struct CleanupFolderSuggestion {
    pub path: String,
    pub old: String,
    pub new: String,
    pub rules: Vec<String>,
    pub confidence: CleanupConfidence,
}

impl CleanupFolderSuggestion {
    #[must_use]
    pub fn operation_id(&self) -> String {
        format!("folder:{}", self.path)
    }
}

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum NameVerdictKind {
    Artist,
    Album,
    Both,
    Unknown,
}

#[must_use]
pub const fn verdict_kind(artist_score: i32, album_score: i32) -> NameVerdictKind {
    let artist_strong = artist_score >= 90;
    let album_strong = album_score >= 90;
    if artist_strong && album_strong {
        NameVerdictKind::Both
    } else if artist_strong && album_score <= 70 {
        NameVerdictKind::Artist
    } else if album_strong && artist_score <= 70 {
        NameVerdictKind::Album
    } else if artist_strong || album_strong {
        NameVerdictKind::Both
    } else {
        NameVerdictKind::Unknown
    }
}

#[must_use]
pub fn cleanup_loose_key(value: &str) -> String {
    let mut output = String::new();
    for character in value
        .nfkd()
        .filter(|character| !is_combining_mark(*character))
    {
        for lower in character.to_lowercase() {
            if lower == 'ß' {
                output.push_str("ss");
            } else if lower.is_ascii_alphanumeric() {
                output.push(lower);
            }
        }
    }
    output
}

fn loose_eq(left: &str, right: &str) -> bool {
    let left = cleanup_loose_key(left);
    !left.is_empty() && left == cleanup_loose_key(right)
}

fn case_key(value: &str) -> String {
    value
        .chars()
        .flat_map(char::to_lowercase)
        .flat_map(|character| {
            if character == 'ß' {
                "ss".chars().collect::<Vec<_>>()
            } else {
                vec![character]
            }
        })
        .collect()
}

fn known_kind(name: Option<&str>, verdicts: Option<&NameVerdicts>) -> Option<NameVerdictKind> {
    let name = name?;
    let scores = verdicts?.get(&cleanup_loose_key(name))?;
    Some(verdict_kind(scores.0, scores.1))
}

fn collapse_whitespace(value: &str) -> String {
    value.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn normalize_separators(stem: &str) -> String {
    let mut output = stem.replace("%20", " ");
    if output.contains('_') && !output.contains(' ') {
        output = output.replace('_', " ");
    }
    collapse_whitespace(&output)
}

fn empty_group_regex() -> &'static Regex {
    regex!(r"\(\s*\)|\[\s*\]")
}

fn tidy(stem: &str) -> String {
    let without_empty_groups = empty_group_regex().replace_all(stem, "");
    collapse_whitespace(&without_empty_groups)
        .trim_matches([' ', '-', '–', '—', '.', '_'])
        .to_owned()
}

fn smart_title(stem: &str) -> String {
    stem.split(' ')
        .map(|word| {
            let mut characters = word.chars();
            let Some(first) = characters.next() else {
                return String::new();
            };
            first
                .to_uppercase()
                .chain(characters.flat_map(char::to_lowercase))
                .collect::<String>()
        })
        .collect::<Vec<_>>()
        .join(" ")
}

fn number_separated_regex() -> &'static Regex {
    regex!(r"^[(\[]?(\d{1,3})(?:[-.](\d{1,2}))?[)\]]?\s*[-–—._]+\s*")
}

fn number_bare_regex() -> &'static Regex {
    regex!(r"^[(\[]?(\d{1,3})(?:[-.](\d{1,2}))?[)\]]?\s+")
}

#[derive(Debug, Clone, Eq, PartialEq)]
struct NumberMatch {
    track: Option<u32>,
    disc: Option<u32>,
    rest: String,
    strong: bool,
}

fn match_leading_number(stem: &str) -> Option<NumberMatch> {
    let captures = number_separated_regex()
        .captures(stem)
        .or_else(|| number_bare_regex().captures(stem))?;
    let matched = captures.get(0)?;
    let rest = stem.get(matched.end()..)?.to_owned();
    if rest.trim().is_empty() {
        return None;
    }
    let first = captures.get(1)?.as_str().parse::<u32>().ok()?;
    let second = captures
        .get(2)
        .and_then(|value| value.as_str().parse::<u32>().ok());
    let (disc, track) = second.map_or((None, Some(first)), |second| {
        if first <= 9 {
            (Some(first), Some(second))
        } else {
            (None, None)
        }
    });
    Some(NumberMatch {
        track,
        disc,
        rest,
        strong: matched
            .as_str()
            .chars()
            .any(|character| matches!(character, '-' | '–' | '—')),
    })
}

fn segment_separator_regex() -> &'static Regex {
    regex!(r"(?:\s+-\s+|\s+-|-\s+|_-_)")
}

fn segments(stem: &str) -> Vec<String> {
    segment_separator_regex()
        .split(stem)
        .map(str::trim)
        .filter(|part| !part.is_empty())
        .map(str::to_owned)
        .collect()
}

fn junk_inner_regex() -> &'static Regex {
    regex!(
        r"(?i)^(?:(?:official\s+)?(?:music\s+|lyrics?\s+)?(?:audio|video|visuali[sz]er)|official|lyrics?|with\s+lyrics?|audio\s+only|official\s+(?:song|track)|(?:hq|hd)\s+(?:audio|video|version)|hd|hq|4k|full\s+(?:album|song|track)|free\s+download|downloaded\s+from\b.*|youtube(?:\.com)?|\d{2,4}\s*kbps|\d{2,4}\s*kb/?s|cbr|vbr\s*v?\d?)$"
    )
}

fn junk_domain_regex() -> &'static Regex {
    regex!(
        r"(?i)(?:www\.)?[a-z0-9][a-z0-9.-]*\.(?:com|net|org|ru|info|me|cc|to|io|biz|pl|cz|sk|fm|co)\b"
    )
}

fn bracket_group_regex() -> &'static Regex {
    regex!(r"[(\[]([^()\[\]]*)[)\]]")
}

fn trailing_dash_junk_regex() -> &'static Regex {
    regex!(
        r"(?i)[-–—]\s*(?:official(?:\s+(?:audio|video|music\s+video))?|lyrics?|audio|youtube)\s*$"
    )
}

fn edge_site_regex() -> &'static Regex {
    regex!(
        r"(?i)^(?:www\.)?[a-z0-9.-]+\.(?:com|net|org|ru|info|me|cc|to|io|biz|pl|cz|sk|fm|co)[\s_–—-]+|[\s_–—-]+(?:www\.)?[a-z0-9.-]+\.(?:com|net|org|ru|info|me|cc|to|io|biz|pl|cz|sk|fm|co)$"
    )
}

fn is_junk_group(inner: &str) -> bool {
    let inner = inner.trim();
    junk_inner_regex().is_match(inner) || junk_domain_regex().is_match(inner)
}

fn strip_junk(stem: &str) -> String {
    let without_groups = bracket_group_regex().replace_all(stem, |captures: &Captures<'_>| {
        if captures
            .get(1)
            .is_some_and(|inner| is_junk_group(inner.as_str()))
        {
            String::new()
        } else {
            captures
                .get(0)
                .map_or_else(String::new, |whole| whole.as_str().to_owned())
        }
    });
    let without_site = edge_site_regex().replace_all(&without_groups, "");
    trailing_dash_junk_regex()
        .replace_all(&without_site, "")
        .into_owned()
}

fn is_generic_name(name: &str) -> bool {
    matches!(
        cleanup_loose_key(name).as_str(),
        "" | "uploads"
            | "upload"
            | "music"
            | "new"
            | "misc"
            | "various"
            | "downloads"
            | "download"
            | "import"
            | "imports"
            | "inbox"
            | "unsorted"
            | "sorted"
            | "tracks"
            | "songs"
            | "audio"
            | "files"
            | "library"
            | "collection"
            | "collections"
            | "mp3"
            | "mp3s"
            | "flac"
            | "flacs"
            | "temp"
            | "tmp"
            | "other"
            | "stuff"
            | "mixed"
            | "mix"
            | "singles"
            | "albums"
            | "album"
            | "artists"
            | "artist"
            | "compilations"
            | "compilation"
            | "playlists"
            | "playlist"
            | "soundtracks"
            | "soundtrack"
            | "ost"
            | "musiclibrary"
            | "random"
    )
}

fn disc_folder_regex() -> &'static Regex {
    regex!(r"(?i)^(?:cd|disc|disk)[\s._-]*(\d{1,2})$")
}

fn part_folder_regex() -> &'static Regex {
    regex!(r"(?i)^(?:pt|part)[\s._-]*(\d{1,3})$")
}

fn pure_number_regex() -> &'static Regex {
    regex!(r"^\d+$")
}

fn folder_year_regex() -> &'static Regex {
    regex!(r"^\s*((?:19|20)\d{2})\s*[-–—.]\s*(.+)$|^(.+?)\s*[(\[]((?:19|20)\d{2})[)\]]\s*$")
}

fn folder_label_separator_regex() -> &'static Regex {
    regex!(r"\s+-\s+")
}

fn split_folder_label(name: &str) -> (Option<String>, String, Option<u32>) {
    let mut base = normalize_separators(name);
    let mut year = None;
    if let Some(captures) = folder_year_regex().captures(&base) {
        if let (Some(year_value), Some(rest)) = (captures.get(1), captures.get(2)) {
            year = year_value.as_str().parse().ok();
            base = rest.as_str().trim().to_owned();
        } else if let (Some(rest), Some(year_value)) = (captures.get(3), captures.get(4)) {
            year = year_value.as_str().parse().ok();
            base = rest.as_str().trim().to_owned();
        }
    }
    let parts = folder_label_separator_regex()
        .split(&base)
        .map(str::trim)
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>();
    if parts.len() == 2 {
        (Some(parts[0].to_owned()), parts[1].to_owned(), year)
    } else {
        (None, base, year)
    }
}

fn most_common(values: impl IntoIterator<Item = String>) -> Option<(String, usize, usize)> {
    let mut counts: Vec<(String, usize)> = Vec::new();
    let mut total = 0;
    for value in values {
        total += 1;
        if let Some((_, count)) = counts.iter_mut().find(|(candidate, _)| *candidate == value) {
            *count += 1;
        } else {
            counts.push((value, 1));
        }
    }
    let mut winner: Option<(String, usize)> = None;
    for (value, count) in counts {
        if winner.as_ref().is_none_or(|(_, best)| count > *best) {
            winner = Some((value, count));
        }
    }
    winner.map(|(value, count)| (value, count, total))
}

fn dominant(values: impl IntoIterator<Item = String>) -> (Option<String>, bool) {
    let cleaned = values
        .into_iter()
        .filter(|value| !value.is_empty())
        .collect::<Vec<_>>();
    if cleaned.len() < 2 {
        return (None, false);
    }
    let Some((key, count, total)) =
        most_common(cleaned.iter().map(|value| cleanup_loose_key(value)))
    else {
        return (None, false);
    };
    if key.is_empty() || count < 2 || count * 3 < total * 2 {
        return (None, false);
    }
    let canonical = most_common(
        cleaned
            .iter()
            .filter(|value| cleanup_loose_key(value) == key)
            .cloned(),
    )
    .map(|(value, _, _)| value);
    (canonical, count == total)
}

#[derive(Debug)]
struct FolderContext<'a> {
    name: String,
    parent_name: String,
    grandparent: String,
    numbers_corroborated: bool,
    first_segment: Option<String>,
    second_segment: Option<String>,
    disc_from_folder: Option<u32>,
    folder_artist: Option<String>,
    folder_album: Option<String>,
    folder_year: Option<u32>,
    albumish: bool,
    dominant_artist: Option<String>,
    dominant_artist_unanimous: bool,
    dominant_album: Option<String>,
    dominant_album_unanimous: bool,
    verdicts: Option<&'a NameVerdicts>,
}

fn numbers_corroborated(stems: &[String]) -> bool {
    let matches = stems
        .iter()
        .filter_map(|stem| match_leading_number(stem))
        .collect::<Vec<_>>();
    if matches.len() < 2 || matches.len() * 10 < stems.len() * 6 {
        return false;
    }
    let numbers = matches
        .iter()
        .filter_map(|matched| matched.track)
        .collect::<Vec<_>>();
    if numbers.len() < 2 {
        return false;
    }
    numbers.iter().copied().collect::<BTreeSet<_>>().len() * 10 >= numbers.len() * 8
}

fn shared_segment(segment_lists: &[Vec<String>], index: usize) -> Option<String> {
    let present = segment_lists
        .iter()
        .filter(|parts| parts.len() > index + 1)
        .map(|parts| parts[index].clone())
        .collect::<Vec<_>>();
    if present.len() < 2 {
        return None;
    }
    let (key, count, _) = most_common(present.iter().map(|part| cleanup_loose_key(part)))?;
    if key.is_empty() || count < 2 || count * 10 < segment_lists.len() * 8 {
        return None;
    }
    most_common(
        present
            .into_iter()
            .filter(|part| cleanup_loose_key(part) == key),
    )
    .map(|(value, _, _)| value)
}

fn parent(path: &str) -> String {
    path.rsplit_once('/')
        .map_or_else(String::new, |(parent, _)| parent.to_owned())
}

fn leaf(path: &str) -> &str {
    path.rsplit_once('/').map_or(path, |(_, leaf)| leaf)
}

fn stem_and_suffix(path: &str) -> (&str, &str) {
    let name = leaf(path);
    let Some(index) = name.rfind('.') else {
        return (name, "");
    };
    if index == 0 || index + 1 == name.len() {
        (name, "")
    } else {
        (&name[..index], &name[index..])
    }
}

fn build_context<'a>(
    folder: &str,
    tracks: &[&IndexedTrack],
    verdicts: Option<&'a NameVerdicts>,
) -> FolderContext<'a> {
    let parts = folder
        .split('/')
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>();
    let parent_name = parts.last().copied().unwrap_or("").to_owned();
    let disc = disc_folder_regex()
        .captures(parent_name.trim())
        .and_then(|captures| captures.get(1))
        .and_then(|value| value.as_str().parse::<u32>().ok());
    let album_parts = if disc.is_some() && !parts.is_empty() {
        &parts[..parts.len() - 1]
    } else {
        &parts[..]
    };
    let name = album_parts.last().copied().unwrap_or("").to_owned();
    let grandparent = album_parts
        .len()
        .checked_sub(2)
        .and_then(|index| album_parts.get(index))
        .copied()
        .unwrap_or("")
        .to_owned();
    let (folder_artist, folder_album, folder_year) = if name.is_empty() {
        (None, None, None)
    } else {
        let (artist, album, year) = split_folder_label(&name);
        (artist, Some(album), year)
    };

    let stems = tracks
        .iter()
        .map(|track| normalize_separators(stem_and_suffix(track.path.as_str()).0))
        .collect::<Vec<_>>();
    let first_pass = numbers_corroborated(&stems);
    let denumbered = stems
        .iter()
        .map(|stem| match_leading_number(stem).map_or_else(|| stem.clone(), |matched| matched.rest))
        .collect::<Vec<_>>();
    let segment_lists = denumbered
        .iter()
        .map(|stem| segments(stem))
        .collect::<Vec<_>>();
    let first_segment = shared_segment(&segment_lists, 0);
    let second_segment = first_segment.as_ref().and_then(|first| {
        let sharing = segment_lists
            .iter()
            .filter(|parts| parts.first().is_some_and(|part| loose_eq(part, first)))
            .map(|parts| parts.iter().skip(1).cloned().collect::<Vec<_>>())
            .collect::<Vec<_>>();
        shared_segment(&sharing, 0)
    });
    let second_pass = first_segment.as_ref().is_some_and(|first| {
        let stripped = segment_lists
            .iter()
            .map(|parts| {
                let mut drop_count =
                    usize::from(parts.first().is_some_and(|part| loose_eq(part, first)));
                if drop_count == 1
                    && second_segment
                        .as_ref()
                        .is_some_and(|second| parts.len() > 2 && loose_eq(&parts[1], second))
                {
                    drop_count = 2;
                }
                parts[drop_count..].join(" - ")
            })
            .collect::<Vec<_>>();
        numbers_corroborated(&stripped)
    });

    let (dominant_artist, dominant_artist_unanimous) =
        dominant(tracks.iter().map(|track| track.metadata.artist.clone()));
    let (dominant_album, dominant_album_unanimous) = dominant(
        tracks
            .iter()
            .filter(|track| {
                let album = &track.metadata.album;
                !album.is_empty()
                    && !disc_folder_regex().is_match(album.trim())
                    && (track.metadata.artist.is_empty()
                        || !loose_eq(album, &track.metadata.artist))
                    && !loose_eq(album, &name)
                    && !loose_eq(album, &parent_name)
            })
            .map(|track| track.metadata.album.clone()),
    );

    FolderContext {
        name,
        parent_name,
        grandparent,
        numbers_corroborated: first_pass || second_pass,
        first_segment,
        second_segment,
        disc_from_folder: disc,
        folder_artist,
        folder_album,
        folder_year,
        albumish: folder_year.is_some() || disc.is_some() || first_pass || second_pass,
        dominant_artist,
        dominant_artist_unanimous,
        dominant_album,
        dominant_album_unanimous,
        verdicts,
    }
}

#[derive(Debug, Default)]
struct Extraction {
    track_number: Option<u32>,
    disc_number: Option<u32>,
    number_confidence: Option<CleanupConfidence>,
    artist: Option<String>,
    artist_confidence: Option<CleanupConfidence>,
    album: Option<String>,
    album_confidence: Option<CleanupConfidence>,
}

fn album_emptyish(track: &IndexedTrack, context: &FolderContext<'_>) -> bool {
    let album = &track.metadata.album;
    album.is_empty()
        || loose_eq(album, &context.name)
        || loose_eq(album, &context.parent_name)
        || (!track.metadata.artist.is_empty() && loose_eq(album, &track.metadata.artist))
        || (!track.metadata.album_artist.is_empty()
            && loose_eq(album, &track.metadata.album_artist))
}

#[derive(Debug)]
struct TagGuess {
    field: CleanupTagField,
    value: String,
    confidence: CleanupConfidence,
}

#[derive(Debug)]
struct SegmentClassification {
    rule: CleanupRule,
    confidence: CleanupConfidence,
    guess: Option<TagGuess>,
}

fn classify_segment(
    segment: &str,
    position: usize,
    part_count: usize,
    track: &IndexedTrack,
    context: &FolderContext<'_>,
) -> Option<SegmentClassification> {
    let artist_empty = track.metadata.artist.is_empty();
    let album_empty = album_emptyish(track, context);
    let classification = |rule, confidence, guess| SegmentClassification {
        rule,
        confidence,
        guess,
    };
    let guess = |field, confidence| TagGuess {
        field,
        value: segment.to_owned(),
        confidence,
    };

    if loose_eq(segment, &track.metadata.artist) || loose_eq(segment, &track.metadata.album_artist)
    {
        return Some(classification(
            CleanupRule::StripArtist,
            CleanupConfidence::High,
            None,
        ));
    }
    if loose_eq(segment, &track.metadata.album) && !album_empty {
        return Some(classification(
            CleanupRule::StripAlbum,
            CleanupConfidence::High,
            None,
        ));
    }
    if context
        .folder_artist
        .as_ref()
        .is_some_and(|value| loose_eq(segment, value))
    {
        return Some(classification(
            CleanupRule::StripArtist,
            CleanupConfidence::High,
            artist_empty.then(|| guess(CleanupTagField::Artist, CleanupConfidence::High)),
        ));
    }
    if context
        .folder_album
        .as_ref()
        .is_some_and(|value| loose_eq(segment, value))
        && context.folder_artist.is_some()
    {
        return Some(classification(
            CleanupRule::StripAlbum,
            CleanupConfidence::High,
            album_empty.then(|| guess(CleanupTagField::Album, CleanupConfidence::High)),
        ));
    }
    if loose_eq(segment, &context.grandparent) {
        return Some(classification(
            CleanupRule::StripArtist,
            CleanupConfidence::High,
            artist_empty.then(|| guess(CleanupTagField::Artist, CleanupConfidence::High)),
        ));
    }
    if loose_eq(segment, &context.name)
        || context
            .folder_album
            .as_ref()
            .is_some_and(|value| loose_eq(segment, value))
    {
        if known_kind(Some(segment), context.verdicts) == Some(NameVerdictKind::Album) {
            return Some(classification(
                CleanupRule::StripAlbum,
                CleanupConfidence::High,
                album_empty.then(|| guess(CleanupTagField::Album, CleanupConfidence::High)),
            ));
        }
        let confidence =
            if known_kind(Some(segment), context.verdicts) == Some(NameVerdictKind::Artist) {
                CleanupConfidence::High
            } else {
                CleanupConfidence::Low
            };
        return Some(classification(
            CleanupRule::StripArtist,
            CleanupConfidence::High,
            artist_empty.then(|| guess(CleanupTagField::Artist, confidence)),
        ));
    }
    if position == 0
        && context
            .first_segment
            .as_ref()
            .is_some_and(|value| loose_eq(segment, value))
    {
        if known_kind(Some(segment), context.verdicts) == Some(NameVerdictKind::Album)
            && album_empty
        {
            return Some(classification(
                CleanupRule::StripAlbum,
                CleanupConfidence::High,
                Some(guess(CleanupTagField::Album, CleanupConfidence::High)),
            ));
        }
        let confidence =
            if known_kind(Some(segment), context.verdicts) == Some(NameVerdictKind::Artist) {
                CleanupConfidence::High
            } else {
                CleanupConfidence::Low
            };
        return Some(classification(
            CleanupRule::StripArtist,
            CleanupConfidence::High,
            artist_empty.then(|| guess(CleanupTagField::Artist, confidence)),
        ));
    }
    if position == 1
        && context
            .second_segment
            .as_ref()
            .is_some_and(|value| loose_eq(segment, value))
    {
        let confidence =
            if known_kind(Some(segment), context.verdicts) == Some(NameVerdictKind::Album) {
                CleanupConfidence::High
            } else {
                CleanupConfidence::Low
            };
        return Some(classification(
            CleanupRule::StripAlbum,
            CleanupConfidence::High,
            album_empty.then(|| guess(CleanupTagField::Album, confidence)),
        ));
    }
    if position == 0
        && context.first_segment.is_none()
        && part_count == 2
        && segment.trim().parse::<u32>().is_err()
    {
        if known_kind(Some(segment), context.verdicts) == Some(NameVerdictKind::Album)
            && album_empty
        {
            return Some(classification(
                CleanupRule::StripAlbum,
                CleanupConfidence::High,
                Some(guess(CleanupTagField::Album, CleanupConfidence::High)),
            ));
        }
        if artist_empty {
            let confidence =
                if known_kind(Some(segment), context.verdicts) == Some(NameVerdictKind::Artist) {
                    CleanupConfidence::High
                } else {
                    CleanupConfidence::Low
                };
            return Some(classification(
                CleanupRule::StripArtist,
                confidence,
                Some(guess(CleanupTagField::Artist, confidence)),
            ));
        }
    }
    None
}

fn fire_rule(
    fired: &mut BTreeMap<CleanupRule, CleanupConfidence>,
    rule: CleanupRule,
    confidence: CleanupConfidence,
) {
    fired
        .entry(rule)
        .and_modify(|current| {
            if *current == CleanupConfidence::Low || confidence == CleanupConfidence::Low {
                *current = CleanupConfidence::Low;
            }
        })
        .or_insert(confidence);
}

fn consume_number(
    stem: &str,
    context: &FolderContext<'_>,
    enabled: CleanupRuleSet,
    extraction: &mut Extraction,
    fired: &mut BTreeMap<CleanupRule, CleanupConfidence>,
) -> String {
    let live = match_leading_number(stem);
    let normalized =
        (!enabled.contains(CleanupRule::NormalizeSeparators)).then(|| normalize_separators(stem));
    let chosen = live
        .clone()
        .or_else(|| normalized.as_deref().and_then(match_leading_number));
    let Some(chosen) = chosen else {
        return stem.to_owned();
    };
    let strip_confidence = if context.numbers_corroborated || chosen.strong {
        CleanupConfidence::High
    } else {
        CleanupConfidence::Low
    };
    if extraction.track_number.is_none()
        && let Some(track_number) = chosen.track
    {
        extraction.track_number = Some(track_number);
        extraction.disc_number = chosen.disc;
        extraction.number_confidence = Some(if context.numbers_corroborated {
            CleanupConfidence::High
        } else {
            CleanupConfidence::Low
        });
    }
    let Some(live) = live else {
        return stem.to_owned();
    };
    if !enabled.contains(CleanupRule::StripTrackNumbers) {
        return stem.to_owned();
    }
    fire_rule(fired, CleanupRule::StripTrackNumbers, strip_confidence);
    live.rest
}

fn transform_stem(
    original: &str,
    track: &IndexedTrack,
    context: &FolderContext<'_>,
    enabled: CleanupRuleSet,
    extraction: &mut Extraction,
) -> (String, BTreeMap<CleanupRule, CleanupConfidence>) {
    let mut fired = BTreeMap::new();
    let mut stem = original.to_owned();
    let normalized = normalize_separators(&stem);
    if enabled.contains(CleanupRule::NormalizeSeparators) && normalized != stem {
        fire_rule(
            &mut fired,
            CleanupRule::NormalizeSeparators,
            CleanupConfidence::High,
        );
        stem = normalized;
    }

    stem = consume_number(&stem, context, enabled, extraction, &mut fired);
    let parts = segments(&stem);
    let mut drop_count = 0;
    for position in 0..2 {
        if parts.len().saturating_sub(drop_count) < 2 || position >= parts.len() {
            break;
        }
        let Some(classification) =
            classify_segment(&parts[position], position, parts.len(), track, context)
        else {
            break;
        };
        if let Some(guess) = classification.guess {
            match guess.field {
                CleanupTagField::Artist if extraction.artist.is_none() => {
                    extraction.artist = Some(guess.value);
                    extraction.artist_confidence = Some(guess.confidence);
                }
                CleanupTagField::Album if extraction.album.is_none() => {
                    extraction.album = Some(guess.value);
                    extraction.album_confidence = Some(guess.confidence);
                }
                _ => {}
            }
        }
        if enabled.contains(classification.rule) {
            drop_count += 1;
            fire_rule(&mut fired, classification.rule, classification.confidence);
        } else if position == 0 {
            break;
        }
    }
    if drop_count != 0 {
        stem = parts[drop_count..].join(" - ");
        stem = consume_number(&stem, context, enabled, extraction, &mut fired);
    }

    if enabled.contains(CleanupRule::StripJunk) {
        let without_junk = strip_junk(&stem);
        if without_junk != stem {
            fire_rule(&mut fired, CleanupRule::StripJunk, CleanupConfidence::High);
            stem = without_junk;
        }
    }
    if enabled.contains(CleanupRule::NormalizeCase)
        && !stem.is_empty()
        && (stem
            .chars()
            .all(|character| !character.is_alphabetic() || character.is_uppercase())
            || stem
                .chars()
                .all(|character| !character.is_alphabetic() || character.is_lowercase()))
    {
        let recased = smart_title(&stem);
        if recased != stem {
            fire_rule(
                &mut fired,
                CleanupRule::NormalizeCase,
                CleanupConfidence::High,
            );
            stem = recased;
        }
    }
    if !fired.is_empty() {
        stem = tidy(&stem);
    }
    if stem.is_empty() {
        (original.to_owned(), BTreeMap::new())
    } else {
        (stem, fired)
    }
}

fn worst_confidence(confidences: impl IntoIterator<Item = CleanupConfidence>) -> CleanupConfidence {
    if confidences
        .into_iter()
        .any(|confidence| confidence == CleanupConfidence::Low)
    {
        CleanupConfidence::Low
    } else {
        CleanupConfidence::High
    }
}

fn text(value: impl Into<String>) -> Option<CleanupValue> {
    Some(CleanupValue::Text(value.into()))
}

fn number(value: u32) -> Option<CleanupValue> {
    Some(CleanupValue::Number(value))
}

fn tag_suggestion(
    track_id: TrackId,
    field: CleanupTagField,
    old: Option<CleanupValue>,
    new: Option<CleanupValue>,
    rule: CleanupRule,
    confidence: CleanupConfidence,
    verified: bool,
) -> CleanupSuggestion {
    CleanupSuggestion {
        track_id,
        kind: CleanupSuggestionKind::Tag,
        field: Some(field),
        old,
        new,
        rules: vec![rule.as_str().to_owned()],
        confidence,
        verified,
    }
}

fn plan_track(
    track: &IndexedTrack,
    context: &FolderContext<'_>,
    enabled: CleanupRuleSet,
) -> CleanupTrackPlan {
    let mut plan = CleanupTrackPlan {
        track_id: track.id,
        path: track.path.as_str().to_owned(),
        operations: Vec::new(),
        notes: Vec::new(),
        wants_lookup: Vec::new(),
    };
    let original_stem = stem_and_suffix(track.path.as_str()).0;
    let mut extraction = Extraction::default();
    let (new_stem, fired) = transform_stem(original_stem, track, context, enabled, &mut extraction);

    if enabled.contains(CleanupRule::TagTitle) {
        let title_rules = if enabled.contains(CleanupRule::NormalizeCase) {
            ALL_CLEANUP_RULES
        } else {
            DEFAULT_CLEANUP_RULES
        };
        let mut title_extraction = Extraction::default();
        let (new_title, title_fired) = transform_stem(
            &track.metadata.title,
            track,
            context,
            title_rules,
            &mut title_extraction,
        );
        if !title_fired.is_empty() && !new_title.is_empty() && new_title != track.metadata.title {
            plan.operations.push(tag_suggestion(
                track.id,
                CleanupTagField::Title,
                text(track.metadata.title.clone()),
                text(new_title),
                CleanupRule::TagTitle,
                worst_confidence(title_fired.values().copied()),
                false,
            ));
        }
    }

    let artist_allowed =
        enabled.contains(CleanupRule::TagArtist) && track.metadata.artist.is_empty();
    let album_allowed = enabled.contains(CleanupRule::TagAlbum) && album_emptyish(track, context);

    let mut artist_new = None;
    let mut artist_confidence = CleanupConfidence::Low;
    let mut artist_flippable = false;
    if artist_allowed {
        if !track.metadata.album_artist.is_empty() {
            artist_new = Some(track.metadata.album_artist.clone());
            artist_confidence = CleanupConfidence::High;
        } else if let Some(extracted) = extraction.artist.clone() {
            artist_new = Some(extracted);
            artist_confidence = extraction
                .artist_confidence
                .unwrap_or(CleanupConfidence::Low);
            artist_flippable = artist_confidence == CleanupConfidence::Low;
        } else if let Some(dominant) = context.dominant_artist.clone() {
            artist_new = Some(dominant);
            artist_confidence = if context.dominant_artist_unanimous {
                CleanupConfidence::High
            } else {
                CleanupConfidence::Low
            };
        } else if context
            .folder_artist
            .as_ref()
            .is_some_and(|artist| !is_generic_name(artist))
        {
            artist_new = context.folder_artist.clone();
            artist_flippable = true;
        } else if context.albumish && !is_generic_name(&context.grandparent) {
            artist_new = Some(context.grandparent.clone());
            artist_flippable = true;
        }
    }

    let mut album_new = None;
    let mut album_confidence = CleanupConfidence::Low;
    let mut album_flippable = false;
    if album_allowed {
        let artistish = [
            (!track.metadata.artist.is_empty()).then_some(track.metadata.artist.clone()),
            (!track.metadata.album_artist.is_empty())
                .then_some(track.metadata.album_artist.clone()),
            extraction.artist.clone(),
            artist_new.clone(),
            context.dominant_artist.clone(),
            context.folder_artist.clone(),
        ]
        .into_iter()
        .flatten()
        .collect::<Vec<_>>();
        if let Some(extracted) = extraction.album.clone() {
            album_confidence = if context
                .folder_album
                .as_ref()
                .is_some_and(|folder_album| loose_eq(&extracted, folder_album))
            {
                CleanupConfidence::High
            } else {
                extraction
                    .album_confidence
                    .unwrap_or(CleanupConfidence::Low)
            };
            album_new = Some(extracted);
            album_flippable = album_confidence == CleanupConfidence::Low;
        } else if let Some(dominant) = context.dominant_album.clone() {
            album_new = Some(dominant);
            album_confidence = if context.dominant_album_unanimous {
                CleanupConfidence::High
            } else {
                CleanupConfidence::Low
            };
        } else if let Some(folder_album) = context.folder_album.as_ref().filter(|folder_album| {
            !is_generic_name(folder_album)
                && !artistish
                    .iter()
                    .any(|artist| loose_eq(folder_album, artist))
        }) {
            album_new = Some(folder_album.clone());
            album_confidence = if context.albumish {
                CleanupConfidence::High
            } else {
                CleanupConfidence::Low
            };
            album_flippable = true;
        }
    }

    let artist_verdict = known_kind(artist_new.as_deref(), context.verdicts);
    let album_verdict = known_kind(album_new.as_deref(), context.verdicts);
    let artist_confidence_before = artist_confidence;
    let album_confidence_before = album_confidence;
    let flip_artist = artist_verdict == Some(NameVerdictKind::Album) && artist_flippable;
    let flip_album = album_verdict == Some(NameVerdictKind::Artist) && album_flippable;
    if artist_verdict == Some(NameVerdictKind::Artist) {
        artist_confidence = CleanupConfidence::High;
    }
    if album_verdict == Some(NameVerdictKind::Album) {
        album_confidence = CleanupConfidence::High;
    }
    let swapped_artist = artist_new.clone();
    let swapped_album = album_new.clone();
    if flip_artist {
        artist_new = None;
    }
    if flip_album {
        album_new = None;
    }
    if flip_album
        && artist_allowed
        && artist_new.is_none()
        && let Some(swapped) = swapped_album.clone()
    {
        artist_new = Some(swapped);
        artist_confidence = CleanupConfidence::High;
    }
    if flip_artist
        && album_allowed
        && album_new.is_none()
        && let Some(swapped) = swapped_artist.clone()
    {
        album_new = Some(swapped);
        album_confidence = CleanupConfidence::High;
    }

    for (candidate, confidence, flippable) in [
        (
            swapped_artist.as_deref(),
            artist_confidence_before,
            artist_flippable,
        ),
        (
            swapped_album.as_deref(),
            album_confidence_before,
            album_flippable,
        ),
    ] {
        if let Some(candidate) = candidate.filter(|candidate| {
            (flippable || confidence == CleanupConfidence::Low)
                && known_kind(Some(candidate), context.verdicts).is_none()
        }) && !plan.wants_lookup.iter().any(|wanted| wanted == candidate)
        {
            plan.wants_lookup.push(candidate.to_owned());
        }
    }

    if let Some(artist) = artist_new {
        let verified = known_kind(Some(&artist), context.verdicts) == Some(NameVerdictKind::Artist);
        plan.operations.push(tag_suggestion(
            track.id,
            CleanupTagField::Artist,
            text(""),
            text(artist),
            CleanupRule::TagArtist,
            artist_confidence,
            verified,
        ));
    }
    if let Some(album) = album_new.filter(|album| *album != track.metadata.album) {
        let verified = known_kind(Some(&album), context.verdicts) == Some(NameVerdictKind::Album);
        plan.operations.push(tag_suggestion(
            track.id,
            CleanupTagField::Album,
            text(track.metadata.album.clone()),
            text(album),
            CleanupRule::TagAlbum,
            album_confidence,
            verified,
        ));
    }

    if enabled.contains(CleanupRule::TagNumber) {
        if track.metadata.track_no.is_none()
            && let Some(track_number) = extraction.track_number
        {
            plan.operations.push(tag_suggestion(
                track.id,
                CleanupTagField::TrackNumber,
                None,
                number(track_number),
                CleanupRule::TagNumber,
                extraction
                    .number_confidence
                    .unwrap_or(CleanupConfidence::Low),
                false,
            ));
        }
        let (disc_number, disc_confidence) = if let Some(disc_number) = extraction.disc_number {
            (
                Some(disc_number),
                extraction
                    .number_confidence
                    .unwrap_or(CleanupConfidence::Low),
            )
        } else {
            (context.disc_from_folder, CleanupConfidence::High)
        };
        if track.metadata.disc_no.is_none()
            && let Some(disc_number) = disc_number
        {
            plan.operations.push(tag_suggestion(
                track.id,
                CleanupTagField::DiscNumber,
                None,
                number(disc_number),
                CleanupRule::TagNumber,
                disc_confidence,
                false,
            ));
        }
    }
    if enabled.contains(CleanupRule::TagYear)
        && track.metadata.year.is_none()
        && let Some(year) = context.folder_year
    {
        plan.operations.push(tag_suggestion(
            track.id,
            CleanupTagField::Year,
            None,
            number(year),
            CleanupRule::TagYear,
            CleanupConfidence::High,
            false,
        ));
    }

    if new_stem != original_stem && !fired.is_empty() {
        let mut rules = fired
            .keys()
            .map(|rule| rule.as_str().to_owned())
            .collect::<Vec<_>>();
        rules.sort_unstable();
        plan.operations.push(CleanupSuggestion {
            track_id: track.id,
            kind: CleanupSuggestionKind::Rename,
            field: None,
            old: text(original_stem),
            new: text(new_stem),
            rules,
            confidence: worst_confidence(fired.values().copied()),
            verified: false,
        });
    }

    plan
}

fn group_by_folder(tracks: &[IndexedTrack]) -> BTreeMap<String, Vec<&IndexedTrack>> {
    let mut grouped = BTreeMap::<String, Vec<&IndexedTrack>>::new();
    for track in tracks {
        grouped
            .entry(parent(track.path.as_str()))
            .or_default()
            .push(track);
    }
    grouped
}

#[must_use]
pub fn analyze_cleanup(
    scope_tracks: &[IndexedTrack],
    all_tracks: &[IndexedTrack],
    enabled: CleanupRuleSet,
    verdicts: Option<&NameVerdicts>,
) -> Vec<CleanupTrackPlan> {
    let all_by_folder = group_by_folder(all_tracks);
    let scope_by_folder = group_by_folder(scope_tracks);
    let mut plans = Vec::new();
    for (folder, mut group) in scope_by_folder {
        group.sort_by(|left, right| left.path.as_str().cmp(right.path.as_str()));
        let context_tracks = if group.len() >= 2 {
            group.as_slice()
        } else {
            all_by_folder
                .get(&folder)
                .map_or(group.as_slice(), Vec::as_slice)
        };
        let context = build_context(&folder, context_tracks, verdicts);
        let mut folder_plans = group
            .iter()
            .map(|track| plan_track(track, &context, enabled))
            .collect::<Vec<_>>();

        let mut existing = BTreeMap::new();
        for track in all_by_folder
            .get(&folder)
            .map_or(context_tracks, Vec::as_slice)
        {
            existing.insert(case_key(leaf(track.path.as_str())), track.id);
        }
        let mut proposed = BTreeMap::<String, TrackId>::new();
        for plan in &mut folder_plans {
            let rename_index = plan
                .operations
                .iter()
                .position(|operation| operation.kind == CleanupSuggestionKind::Rename);
            let Some(rename_index) = rename_index else {
                continue;
            };
            let new_stem = match plan.operations[rename_index].new.as_ref() {
                Some(CleanupValue::Text(value)) => value,
                _ => continue,
            };
            let suffix = stem_and_suffix(&plan.path).1;
            let target_name = format!("{new_stem}{suffix}");
            let key = case_key(&target_name);
            if existing
                .get(&key)
                .is_some_and(|track_id| *track_id != plan.track_id)
            {
                plan.operations.remove(rename_index);
                plan.notes.push(format!(
                    "rename dropped: \"{target_name}\" already exists in this folder"
                ));
                continue;
            }
            if proposed.contains_key(&key) {
                plan.operations.remove(rename_index);
                plan.notes.push(format!(
                    "rename dropped: another track would also become \"{target_name}\""
                ));
                continue;
            }
            proposed.insert(key, plan.track_id);
        }
        plans.extend(folder_plans.into_iter().filter(|plan| {
            !plan.operations.is_empty() || !plan.notes.is_empty() || !plan.wants_lookup.is_empty()
        }));
    }
    plans
}

fn sanitize_leaf(name: &str) -> String {
    collapse_whitespace(&name.replace(['/', '\\'], "-"))
        .trim_matches([' ', '-', '–', '—', '.', '_'])
        .chars()
        .take(120)
        .collect()
}

fn disc_part_canonical(name: &str) -> Option<(String, &'static str)> {
    let name = name.trim();
    if let Some(number) = disc_folder_regex()
        .captures(name)
        .and_then(|captures| captures.get(1))
        .and_then(|value| value.as_str().parse::<u32>().ok())
    {
        return Some((format!("Disc {number}"), "disc_canonical"));
    }
    part_folder_regex()
        .captures(name)
        .and_then(|captures| captures.get(1))
        .and_then(|value| value.as_str().parse::<u32>().ok())
        .map(|number| (format!("Part {number}"), "part_canonical"))
}

fn tidy_folder_leaf(name: &str, normalize_case: bool) -> (String, Vec<String>) {
    let mut rules = Vec::new();
    let mut output = name.to_owned();
    let normalized = normalize_separators(&output);
    if normalized != output {
        rules.push(CleanupRule::NormalizeSeparators.as_str().to_owned());
        output = normalized;
    }
    let without_junk = strip_junk(&output);
    if without_junk != output {
        rules.push(CleanupRule::StripJunk.as_str().to_owned());
        output = without_junk;
    }
    if normalize_case
        && !output.is_empty()
        && (output
            .chars()
            .all(|character| !character.is_alphabetic() || character.is_uppercase())
            || output
                .chars()
                .all(|character| !character.is_alphabetic() || character.is_lowercase()))
    {
        let recased = smart_title(&output);
        if recased != output {
            rules.push(CleanupRule::NormalizeCase.as_str().to_owned());
            output = recased;
        }
    }
    if !rules.is_empty() {
        output = tidy(&output);
    }
    (sanitize_leaf(&output), rules)
}

#[derive(Debug)]
struct FolderClues {
    album: Option<String>,
    artist: Option<String>,
    year: Option<u32>,
}

fn single_or_dominant(values: Vec<String>) -> Option<String> {
    let cleaned = values
        .into_iter()
        .filter(|value| !value.is_empty())
        .collect::<Vec<_>>();
    if cleaned.len() == 1 {
        cleaned.into_iter().next()
    } else {
        dominant(cleaned).0
    }
}

fn folder_clues(folder: &str, tracks: &[&IndexedTrack]) -> FolderClues {
    let folder_leaf = leaf(folder);
    let albums = tracks
        .iter()
        .filter(|track| {
            let album = &track.metadata.album;
            !album.is_empty()
                && !disc_folder_regex().is_match(album.trim())
                && !part_folder_regex().is_match(album.trim())
                && (track.metadata.artist.is_empty() || !loose_eq(album, &track.metadata.artist))
                && !loose_eq(album, folder_leaf)
        })
        .map(|track| track.metadata.album.clone())
        .collect();
    let years = tracks
        .iter()
        .filter_map(|track| track.metadata.year)
        .collect::<Vec<_>>();
    let year = years
        .first()
        .copied()
        .filter(|first| years.iter().all(|year| year == first));
    FolderClues {
        album: single_or_dominant(albums),
        artist: single_or_dominant(
            tracks
                .iter()
                .map(|track| track.metadata.artist.clone())
                .collect(),
        ),
        year,
    }
}

fn folder_name_usable(name: &str, clues: &FolderClues) -> bool {
    !name.is_empty()
        && !is_generic_name(name)
        && !pure_number_regex().is_match(name.trim())
        && cleanup_loose_key(name).len() >= 2
        && clues
            .artist
            .as_ref()
            .is_none_or(|artist| !loose_eq(name, artist))
}

fn rebuild_folder_name(folder: &str, clues: &FolderClues) -> Option<String> {
    let mut name = clues.album.clone()?;
    if let Some(year) = clues.year {
        name = format!("{name} ({year})");
    }
    let parent_path = parent(folder);
    let parent_leaf = leaf(&parent_path);
    if let Some(artist) = clues.artist.as_ref().filter(|artist| {
        !loose_eq(artist, clues.album.as_deref().unwrap_or("")) && !loose_eq(artist, parent_leaf)
    }) {
        name = format!("{artist} - {name}");
    }
    let sanitized = sanitize_leaf(&name);
    (!sanitized.is_empty()).then_some(sanitized)
}

fn plan_folder(
    folder: &str,
    tracks: &[&IndexedTrack],
    normalize_case: bool,
) -> Option<CleanupFolderSuggestion> {
    let old = leaf(folder);
    if old.is_empty() {
        return None;
    }
    if let Some((new, rule)) = disc_part_canonical(old) {
        return (new != old).then(|| CleanupFolderSuggestion {
            path: folder.to_owned(),
            old: old.to_owned(),
            new,
            rules: vec![rule.to_owned()],
            confidence: CleanupConfidence::High,
        });
    }
    if is_generic_name(old) {
        return None;
    }
    let clues = folder_clues(folder, tracks);
    let (tidied, rules) = tidy_folder_leaf(old, normalize_case);
    if !folder_name_usable(&tidied, &clues)
        && let Some(rebuilt) = rebuild_folder_name(folder, &clues).filter(|name| name != old)
    {
        return Some(CleanupFolderSuggestion {
            path: folder.to_owned(),
            old: old.to_owned(),
            new: rebuilt,
            rules: vec!["rebuild_from_tags".to_owned()],
            confidence: CleanupConfidence::Low,
        });
    }
    if !tidied.is_empty() && tidied != old {
        Some(CleanupFolderSuggestion {
            path: folder.to_owned(),
            old: old.to_owned(),
            new: tidied,
            rules: if rules.is_empty() {
                vec![CleanupRule::NormalizeSeparators.as_str().to_owned()]
            } else {
                rules
            },
            confidence: CleanupConfidence::High,
        })
    } else {
        None
    }
}

fn tracks_under<'a>(all_tracks: &'a [IndexedTrack], folder: &str) -> Vec<&'a IndexedTrack> {
    let prefix = format!("{folder}/");
    all_tracks
        .iter()
        .filter(|track| track.path.as_str().starts_with(&prefix))
        .collect()
}

fn all_folder_paths(tracks: &[IndexedTrack]) -> BTreeSet<String> {
    let mut output = BTreeSet::new();
    for track in tracks {
        let mut current = parent(track.path.as_str());
        while !current.is_empty() {
            output.insert(current.clone());
            current = parent(&current);
        }
    }
    output
}

#[must_use]
pub fn analyze_cleanup_folders(
    scope_tracks: &[IndexedTrack],
    all_tracks: &[IndexedTrack],
    enabled: CleanupRuleSet,
) -> Vec<CleanupFolderSuggestion> {
    if !enabled.contains(CleanupRule::RenameFolders) {
        return Vec::new();
    }
    let normalize_case = enabled.contains(CleanupRule::NormalizeCase);
    let mut candidates = BTreeSet::new();
    for folder in group_by_folder(scope_tracks).keys() {
        if folder.is_empty() {
            continue;
        }
        candidates.insert(folder.clone());
        if disc_part_canonical(leaf(folder)).is_some() {
            let parent = parent(folder);
            if !parent.is_empty() {
                candidates.insert(parent);
            }
        }
    }
    let mut suggestions = candidates
        .into_iter()
        .filter_map(|folder| {
            let tracks = tracks_under(all_tracks, &folder);
            plan_folder(&folder, &tracks, normalize_case)
        })
        .collect::<Vec<_>>();
    suggestions.sort_by(|left, right| {
        right
            .path
            .matches('/')
            .count()
            .cmp(&left.path.matches('/').count())
            .then_with(|| left.path.cmp(&right.path))
    });
    let existing = all_folder_paths(all_tracks)
        .into_iter()
        .map(|path| case_key(&path))
        .collect::<BTreeSet<_>>();
    let mut taken = BTreeSet::new();
    let mut kept = Vec::new();
    for suggestion in suggestions {
        let parent = parent(&suggestion.path);
        let new_path = if parent.is_empty() {
            suggestion.new.clone()
        } else {
            format!("{parent}/{}", suggestion.new)
        };
        let key = case_key(&new_path);
        if key != case_key(&suggestion.path) && (existing.contains(&key) || taken.contains(&key)) {
            continue;
        }
        taken.insert(key);
        kept.push(suggestion);
    }
    kept.sort_by(|left, right| left.path.cmp(&right.path));
    kept
}

#[must_use]
pub fn pending_cleanup_lookups(
    plans: &[CleanupTrackPlan],
    verdicts: Option<&NameVerdicts>,
) -> Vec<String> {
    let mut seen = BTreeSet::new();
    let mut output = Vec::new();
    let mut add = |name: &str| {
        let name = name.trim();
        let key = cleanup_loose_key(name);
        if key.len() < 2
            || seen.contains(&key)
            || verdicts.is_some_and(|verdicts| verdicts.contains_key(&key))
        {
            return;
        }
        seen.insert(key);
        output.push(name.to_owned());
    };
    for plan in plans {
        for name in &plan.wants_lookup {
            add(name);
        }
        for operation in &plan.operations {
            if operation.kind == CleanupSuggestionKind::Tag
                && matches!(
                    operation.field,
                    Some(CleanupTagField::Artist | CleanupTagField::Album)
                )
                && operation.confidence == CleanupConfidence::Low
                && let Some(CleanupValue::Text(value)) = &operation.new
            {
                add(value);
            }
        }
    }
    output
}

#[cfg(test)]
mod tests {
    use std::error::Error;
    use std::time::Duration;

    use crate::{LibraryPath, TrackMetadata};

    use super::*;

    fn track(id: i64, path: &str) -> Result<IndexedTrack, Box<dyn Error>> {
        let path = LibraryPath::parse(path)?;
        let parent = parent(path.as_str());
        Ok(IndexedTrack {
            id: TrackId::new(id)?,
            metadata: TrackMetadata {
                title: stem_and_suffix(path.as_str()).0.to_owned(),
                artist: String::new(),
                album_artist: String::new(),
                album: leaf(&parent).to_owned(),
                track_no: None,
                disc_no: None,
                year: None,
                genre: String::new(),
                bpm: None,
            },
            path,
            duration: Duration::ZERO,
            display_title: String::new(),
            origin: String::new(),
            size_bytes: 0,
            mtime_unix_seconds: 0,
            added_at_unix_seconds: 0,
        })
    }

    fn operation(
        plans: &[CleanupTrackPlan],
        track_id: TrackId,
        kind: CleanupSuggestionKind,
        field: Option<CleanupTagField>,
    ) -> Option<&CleanupSuggestion> {
        plans
            .iter()
            .find(|plan| plan.track_id == track_id)?
            .operations
            .iter()
            .find(|operation| operation.kind == kind && operation.field == field)
    }

    fn text_value(value: &Option<CleanupValue>) -> Option<&str> {
        match value {
            Some(CleanupValue::Text(value)) => Some(value),
            _ => None,
        }
    }

    fn number_value(value: &Option<CleanupValue>) -> Option<u32> {
        match value {
            Some(CleanupValue::Number(value)) => Some(*value),
            _ => None,
        }
    }

    #[test]
    fn numbered_album_is_corroborated_and_tagged() -> Result<(), Box<dyn Error>> {
        let tracks = vec![
            track(1, "Album/01 - Alpha.mp3")?,
            track(2, "Album/02 - Beta.mp3")?,
            track(3, "Album/03 - Gamma.mp3")?,
        ];
        let plans = analyze_cleanup(&tracks, &tracks, DEFAULT_CLEANUP_RULES, None);
        for (track, expected_number, expected_name) in tracks
            .iter()
            .zip([1, 2, 3])
            .zip(["Alpha", "Beta", "Gamma"])
            .map(|((track, number), name)| (track, number, name))
        {
            let rename = operation(&plans, track.id, CleanupSuggestionKind::Rename, None)
                .ok_or("missing rename")?;
            assert_eq!(text_value(&rename.new), Some(expected_name));
            assert_eq!(rename.confidence, CleanupConfidence::High);
            let number = operation(
                &plans,
                track.id,
                CleanupSuggestionKind::Tag,
                Some(CleanupTagField::TrackNumber),
            )
            .ok_or("missing track number")?;
            assert_eq!(number_value(&number.new), Some(expected_number));
            assert_eq!(number.confidence, CleanupConfidence::High);
        }
        Ok(())
    }

    #[test]
    fn artist_segments_junk_separators_and_case_follow_rule_strength() -> Result<(), Box<dyn Error>>
    {
        let mut accented = track(1, "Pop/beyonce - Halo.mp3")?;
        accented.metadata.title = "Halo".to_owned();
        accented.metadata.artist = "Beyoncé".to_owned();
        let junk = track(2, "Misc/Song_Title_[320kbps].mp3")?;
        let loud = track(3, "Misc/MY LOUD SONG.mp3")?;
        let all = vec![accented.clone(), junk.clone(), loud.clone()];
        let plans = analyze_cleanup(&all, &all, DEFAULT_CLEANUP_RULES, None);
        assert_eq!(
            operation(&plans, accented.id, CleanupSuggestionKind::Rename, None)
                .and_then(|operation| text_value(&operation.new)),
            Some("Halo")
        );
        assert_eq!(
            operation(&plans, junk.id, CleanupSuggestionKind::Rename, None)
                .and_then(|operation| text_value(&operation.new)),
            Some("Song Title")
        );
        assert!(operation(&plans, loud.id, CleanupSuggestionKind::Rename, None).is_none());
        let all_rules = analyze_cleanup(
            std::slice::from_ref(&loud),
            std::slice::from_ref(&loud),
            ALL_CLEANUP_RULES,
            None,
        );
        assert_eq!(
            operation(
                &all_rules,
                TrackId::new(3)?,
                CleanupSuggestionKind::Rename,
                None
            )
            .and_then(|operation| text_value(&operation.new)),
            Some("My Loud Song")
        );
        Ok(())
    }

    #[test]
    fn folder_clues_verdicts_and_collisions_are_conservative() -> Result<(), Box<dyn Error>> {
        let mut first = track(1, "Andrey Vinogradov/01 - Pastorale.mp3")?;
        let second = track(2, "Andrey Vinogradov/02 - Oberek.mp3")?;
        let offline = analyze_cleanup(
            &[first.clone(), second.clone()],
            &[first.clone(), second.clone()],
            DEFAULT_CLEANUP_RULES,
            None,
        );
        assert_eq!(
            pending_cleanup_lookups(&offline, None),
            ["Andrey Vinogradov"]
        );
        let verdicts = BTreeMap::from([(cleanup_loose_key("Andrey Vinogradov"), (100, 30))]);
        let verified = analyze_cleanup(
            &[first.clone(), second.clone()],
            &[first.clone(), second.clone()],
            DEFAULT_CLEANUP_RULES,
            Some(&verdicts),
        );
        let artist = operation(
            &verified,
            first.id,
            CleanupSuggestionKind::Tag,
            Some(CleanupTagField::Artist),
        )
        .ok_or("missing verified artist")?;
        assert_eq!(text_value(&artist.new), Some("Andrey Vinogradov"));
        assert!(artist.verified);

        first = track(3, "Misc/01 - Bar.mp3")?;
        let existing = track(4, "Misc/Bar.mp3")?;
        let collision = analyze_cleanup(
            &[first.clone()],
            &[first.clone(), existing],
            DEFAULT_CLEANUP_RULES,
            None,
        );
        assert!(operation(&collision, first.id, CleanupSuggestionKind::Rename, None).is_none());
        assert!(collision[0].notes[0].contains("already exists"));
        Ok(())
    }

    #[test]
    fn folder_renames_tidy_canonicalize_rebuild_and_avoid_collisions() -> Result<(), Box<dyn Error>>
    {
        let tidy_tracks = vec![
            track(1, "Skyrim_OST_(2011)/01 - Dragonborn.mp3")?,
            track(2, "Skyrim_OST_(2011)/02 - Awake.mp3")?,
        ];
        let tidy = analyze_cleanup_folders(&tidy_tracks, &tidy_tracks, DEFAULT_CLEANUP_RULES);
        assert_eq!(tidy[0].new, "Skyrim OST (2011)");

        let disc_tracks = vec![
            track(3, "Big Album/CD1/01 - One.mp3")?,
            track(4, "Big Album/CD2/01 - Two.mp3")?,
        ];
        let discs = analyze_cleanup_folders(&disc_tracks, &disc_tracks, DEFAULT_CLEANUP_RULES);
        assert!(discs.iter().any(|folder| folder.new == "Disc 1"));
        assert!(discs.iter().any(|folder| folder.new == "Disc 2"));

        let mut rebuilt_a = track(5, "1/a.mp3")?;
        rebuilt_a.metadata.artist = "Pendulum".to_owned();
        rebuilt_a.metadata.album = "Immersion".to_owned();
        rebuilt_a.metadata.year = Some(2010);
        let mut rebuilt_b = track(6, "1/b.mp3")?;
        rebuilt_b.metadata = rebuilt_a.metadata.clone();
        rebuilt_b.metadata.title = "b".to_owned();
        let rebuilt = analyze_cleanup_folders(
            &[rebuilt_a.clone(), rebuilt_b.clone()],
            &[rebuilt_a, rebuilt_b],
            DEFAULT_CLEANUP_RULES,
        );
        assert_eq!(rebuilt[0].new, "Pendulum - Immersion (2010)");
        assert_eq!(rebuilt[0].confidence, CleanupConfidence::Low);

        let collisions = vec![track(7, "Album_X/a.mp3")?, track(8, "Album X/b.mp3")?];
        assert!(
            analyze_cleanup_folders(&collisions, &collisions, DEFAULT_CLEANUP_RULES)
                .iter()
                .all(|folder| folder.path != "Album_X")
        );
        Ok(())
    }

    #[test]
    fn combined_artist_album_number_layout_produces_one_coherent_plan() -> Result<(), Box<dyn Error>>
    {
        let tracks = vec![
            track(1, "Stuff/Artist - Album - 01 - Song One.mp3")?,
            track(2, "Stuff/Artist - Album - 02 - Song Two.mp3")?,
            track(3, "Stuff/Artist - Album - 03 - Song Three.mp3")?,
        ];
        let plans = analyze_cleanup(&tracks, &tracks, DEFAULT_CLEANUP_RULES, None);
        let track_id = TrackId::new(2)?;
        assert_eq!(
            operation(&plans, track_id, CleanupSuggestionKind::Rename, None)
                .and_then(|operation| text_value(&operation.new)),
            Some("Song Two")
        );
        assert_eq!(
            operation(
                &plans,
                track_id,
                CleanupSuggestionKind::Tag,
                Some(CleanupTagField::Artist)
            )
            .and_then(|operation| text_value(&operation.new)),
            Some("Artist")
        );
        assert_eq!(
            operation(
                &plans,
                track_id,
                CleanupSuggestionKind::Tag,
                Some(CleanupTagField::Album)
            )
            .and_then(|operation| text_value(&operation.new)),
            Some("Album")
        );
        assert_eq!(
            operation(
                &plans,
                track_id,
                CleanupSuggestionKind::Tag,
                Some(CleanupTagField::TrackNumber)
            )
            .and_then(|operation| number_value(&operation.new)),
            Some(2)
        );
        assert_eq!(
            operation(
                &plans,
                track_id,
                CleanupSuggestionKind::Tag,
                Some(CleanupTagField::Title)
            )
            .and_then(|operation| text_value(&operation.new)),
            Some("Song Two")
        );
        Ok(())
    }

    #[test]
    fn disc_year_and_dominant_sibling_clues_fill_missing_tags() -> Result<(), Box<dyn Error>> {
        let discs = vec![
            track(1, "Big Album/CD1/01 - One.mp3")?,
            track(2, "Big Album/CD1/02 - Two.mp3")?,
            track(3, "Big Album/CD2/01 - Three.mp3")?,
        ];
        let disc_plans = analyze_cleanup(&discs, &discs, DEFAULT_CLEANUP_RULES, None);
        assert_eq!(
            operation(
                &disc_plans,
                TrackId::new(1)?,
                CleanupSuggestionKind::Tag,
                Some(CleanupTagField::DiscNumber)
            )
            .and_then(|operation| number_value(&operation.new)),
            Some(1)
        );
        assert_eq!(
            operation(
                &disc_plans,
                TrackId::new(1)?,
                CleanupSuggestionKind::Tag,
                Some(CleanupTagField::Album)
            )
            .and_then(|operation| text_value(&operation.new)),
            Some("Big Album")
        );

        let dated = vec![
            track(4, "Skyrim OST (2011)/01 - Dragonborn.mp3")?,
            track(5, "Skyrim OST (2011)/02 - Awake.mp3")?,
        ];
        let dated_plans = analyze_cleanup(&dated, &dated, DEFAULT_CLEANUP_RULES, None);
        assert_eq!(
            operation(
                &dated_plans,
                TrackId::new(4)?,
                CleanupSuggestionKind::Tag,
                Some(CleanupTagField::Year)
            )
            .and_then(|operation| number_value(&operation.new)),
            Some(2011)
        );
        assert_eq!(
            operation(
                &dated_plans,
                TrackId::new(4)?,
                CleanupSuggestionKind::Tag,
                Some(CleanupTagField::Album)
            )
            .and_then(|operation| text_value(&operation.new)),
            Some("Skyrim OST")
        );

        let mut alpha = track(6, "Mixtape/Alpha.mp3")?;
        alpha.metadata.title = "Alpha".to_owned();
        alpha.metadata.artist = "The Band".to_owned();
        alpha.metadata.album = "Live Set".to_owned();
        let mut beta = track(7, "Mixtape/Beta.mp3")?;
        beta.metadata.title = "Beta".to_owned();
        beta.metadata.artist = "The Band".to_owned();
        beta.metadata.album = "Live Set".to_owned();
        let mut gamma = track(8, "Mixtape/Gamma.mp3")?;
        gamma.metadata.title = "Gamma".to_owned();
        gamma.metadata.album.clear();
        let siblings = vec![alpha, beta, gamma];
        let sibling_plans = analyze_cleanup(&siblings, &siblings, DEFAULT_CLEANUP_RULES, None);
        assert_eq!(
            operation(
                &sibling_plans,
                TrackId::new(8)?,
                CleanupSuggestionKind::Tag,
                Some(CleanupTagField::Artist)
            )
            .and_then(|operation| text_value(&operation.new)),
            Some("The Band")
        );
        assert_eq!(
            operation(
                &sibling_plans,
                TrackId::new(8)?,
                CleanupSuggestionKind::Tag,
                Some(CleanupTagField::Album)
            )
            .and_then(|operation| text_value(&operation.new)),
            Some("Live Set")
        );
        Ok(())
    }

    #[test]
    fn rule_gating_and_cached_album_verdicts_do_not_leak_other_guesses()
    -> Result<(), Box<dyn Error>> {
        let junk = track(1, "Misc/01 - Foo (Official Audio).mp3")?;
        let junk_only = CleanupRuleSet::from_iter([CleanupRule::StripJunk]);
        let plans = analyze_cleanup(
            std::slice::from_ref(&junk),
            std::slice::from_ref(&junk),
            junk_only,
            None,
        );
        let plan = plans.first().ok_or("missing junk cleanup plan")?;
        assert_eq!(plan.operations.len(), 1);
        assert_eq!(plan.operations[0].kind, CleanupSuggestionKind::Rename);
        assert_eq!(text_value(&plan.operations[0].new), Some("01 - Foo"));

        let album = track(2, "Misc/Abbey Road - Come Together.mp3")?;
        let verdicts = BTreeMap::from([(cleanup_loose_key("Abbey Road"), (10, 100))]);
        let verified = analyze_cleanup(
            std::slice::from_ref(&album),
            std::slice::from_ref(&album),
            DEFAULT_CLEANUP_RULES,
            Some(&verdicts),
        );
        let album_operation = operation(
            &verified,
            album.id,
            CleanupSuggestionKind::Tag,
            Some(CleanupTagField::Album),
        )
        .ok_or("missing verified album operation")?;
        assert_eq!(text_value(&album_operation.new), Some("Abbey Road"));
        assert_eq!(album_operation.confidence, CleanupConfidence::High);
        assert!(album_operation.verified);
        assert!(
            operation(
                &verified,
                album.id,
                CleanupSuggestionKind::Tag,
                Some(CleanupTagField::Artist)
            )
            .is_none()
        );
        Ok(())
    }
}
