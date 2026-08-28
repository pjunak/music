use std::cmp::Ordering;
use std::collections::{BTreeMap, BTreeSet};

use music_domain::{IndexedTrack, TrackId};
use serde::{Deserialize, Serialize};
use unicode_general_category::{GeneralCategory, get_general_category};
use unicode_normalization::UnicodeNormalization;

use super::tags::{
    AssistantTrackEvidence, AudioSignalProfile, Confidence, current_audio_analysis,
    current_metadata_analysis, view_for_track,
};

pub const LOCAL_PLAYLIST_ENGINE_ID: &str = "local-planner/v2";

#[derive(Debug, Clone, Copy, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum EnergyCurve {
    #[default]
    Steady,
    Rising,
    Falling,
    Arc,
}

impl EnergyCurve {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Steady => "steady",
            Self::Rising => "rising",
            Self::Falling => "falling",
            Self::Arc => "arc",
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct PlaylistSuggestionRequest {
    pub prompt: String,
    pub target_minutes: u16,
    pub candidate_limit: u16,
    pub min_bpm: Option<u32>,
    pub max_bpm: Option<u32>,
    pub include_unknown_bpm: bool,
    pub exclude_track_ids: Vec<TrackId>,
    pub energy_curve: EnergyCurve,
}

impl PlaylistSuggestionRequest {
    pub fn validate(&self) -> Result<(), String> {
        let prompt_length = self.prompt.trim().chars().count();
        if !(2..=500).contains(&prompt_length) {
            return Err("prompt must contain between 2 and 500 characters".to_owned());
        }
        if !(5..=600).contains(&self.target_minutes) {
            return Err("target_minutes must be between 5 and 600".to_owned());
        }
        if !(5..=100).contains(&self.candidate_limit) {
            return Err("candidate_limit must be between 5 and 100".to_owned());
        }
        if self.exclude_track_ids.len() > 5_000 {
            return Err("exclude_track_ids cannot exceed 5000 tracks".to_owned());
        }
        if self
            .min_bpm
            .is_some_and(|value| !(1..=999).contains(&value))
            || self
                .max_bpm
                .is_some_and(|value| !(1..=999).contains(&value))
        {
            return Err("BPM bounds must be between 1 and 999".to_owned());
        }
        if matches!((self.min_bpm, self.max_bpm), (Some(minimum), Some(maximum)) if minimum > maximum)
        {
            return Err("min_bpm cannot be greater than max_bpm".to_owned());
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct PlaylistIntent {
    pub matched_moods: Vec<String>,
    pub search_terms: Vec<String>,
    pub energy: f64,
    pub brightness: f64,
    pub tension: f64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct PlaylistAudioSignal {
    pub analyzer_id: String,
    pub energy: f64,
    pub brightness: f64,
    pub tension: f64,
    pub tempo_bpm: Option<f64>,
    pub confidence: Confidence,
}

#[derive(Debug, Clone, PartialEq)]
pub struct PlaylistPlan {
    pub energy_curve: EnergyCurve,
    pub selected_tracks: usize,
    pub selected_duration_s: f64,
    pub audio_profile_tracks: usize,
}

#[derive(Debug, Clone, PartialEq)]
pub struct PlaylistCandidate {
    pub track_id: TrackId,
    pub path: String,
    pub title: String,
    pub display_title: String,
    pub artist: String,
    pub album: String,
    pub origin: String,
    pub genre: String,
    pub manual_tags: Vec<String>,
    pub analysis_tags: Vec<String>,
    pub length_s: f64,
    pub bpm: Option<u32>,
    pub match_score: f64,
    pub confidence: Confidence,
    pub reasons: Vec<String>,
    pub default_selected: bool,
    pub sequence_position: Option<usize>,
    pub planning_energy: f64,
    pub audio_signal: Option<PlaylistAudioSignal>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct PlaylistSuggestion {
    pub engine: String,
    pub library_tracks: usize,
    pub eligible_tracks: usize,
    pub intent: PlaylistIntent,
    pub plan: PlaylistPlan,
    pub candidates: Vec<PlaylistCandidate>,
}

#[derive(Debug, Clone)]
struct MoodProfile {
    name: &'static str,
    aliases: &'static [&'static str],
    energy: f64,
    brightness: f64,
    tension: f64,
}

const MOODS: &[MoodProfile] = &[
    MoodProfile {
        name: "calm",
        aliases: &[
            "ambient", "calm", "gentle", "peaceful", "quiet", "relaxed", "rest",
        ],
        energy: 0.20,
        brightness: 0.60,
        tension: 0.15,
    },
    MoodProfile {
        name: "tense",
        aliases: &[
            "investigation",
            "mystery",
            "ominous",
            "suspense",
            "tense",
            "tension",
        ],
        energy: 0.50,
        brightness: 0.30,
        tension: 0.82,
    },
    MoodProfile {
        name: "combat",
        aliases: &[
            "action", "battle", "boss", "combat", "epic", "fight", "intense",
        ],
        energy: 0.90,
        brightness: 0.48,
        tension: 0.76,
    },
    MoodProfile {
        name: "dark",
        aliases: &["dark", "dread", "haunting", "horror", "scary", "sinister"],
        energy: 0.45,
        brightness: 0.12,
        tension: 0.90,
    },
    MoodProfile {
        name: "bright",
        aliases: &[
            "bright",
            "happy",
            "hopeful",
            "joyful",
            "triumphant",
            "uplifting",
        ],
        energy: 0.62,
        brightness: 0.88,
        tension: 0.18,
    },
    MoodProfile {
        name: "melancholy",
        aliases: &["grief", "melancholy", "sad", "somber", "sorrow", "tragic"],
        energy: 0.28,
        brightness: 0.14,
        tension: 0.35,
    },
    MoodProfile {
        name: "tavern",
        aliases: &["acoustic", "folk", "inn", "medieval", "tavern"],
        energy: 0.44,
        brightness: 0.68,
        tension: 0.18,
    },
    MoodProfile {
        name: "festive",
        aliases: &[
            "celebration",
            "dance",
            "dancing",
            "feast",
            "festive",
            "festival",
        ],
        energy: 0.72,
        brightness: 0.82,
        tension: 0.12,
    },
    MoodProfile {
        name: "exploration",
        aliases: &[
            "adventure",
            "exploration",
            "journey",
            "travel",
            "wilderness",
        ],
        energy: 0.46,
        brightness: 0.58,
        tension: 0.32,
    },
];

const STOP_WORDS: &[&str] = &[
    "a", "an", "and", "for", "from", "in", "into", "music", "of", "on", "playlist", "song",
    "songs", "the", "to", "track", "tracks", "with",
];

const GENRE_ENERGY: &[(&str, f64)] = &[
    ("ambient", 0.15),
    ("classical", 0.34),
    ("acoustic", 0.30),
    ("folk", 0.42),
    ("jazz", 0.45),
    ("soundtrack", 0.55),
    ("cinematic", 0.58),
    ("electronic", 0.68),
    ("rock", 0.72),
    ("dance", 0.82),
    ("metal", 0.90),
];

#[derive(Debug, Clone)]
struct MetadataProfile {
    energy: f64,
    brightness: f64,
    tension: f64,
    moods: Vec<String>,
    confidence: Confidence,
}

#[derive(Debug, Clone)]
struct RankedTrack<'a> {
    track: &'a IndexedTrack,
    score: f64,
    confidence: Confidence,
    reasons: Vec<String>,
    manual_tags: Vec<String>,
    analysis_tags: Vec<String>,
    planning_energy: f64,
    signal: Option<AudioSignalProfile>,
}

pub fn interpret_prompt(prompt: &str) -> PlaylistIntent {
    let prompt_tokens = tokens(prompt);
    let matched = MOODS
        .iter()
        .filter(|profile| {
            profile
                .aliases
                .iter()
                .any(|alias| prompt_tokens.contains(*alias))
        })
        .collect::<Vec<_>>();
    let all_aliases = MOODS
        .iter()
        .flat_map(|profile| profile.aliases)
        .copied()
        .collect::<BTreeSet<_>>();
    let stop_words = STOP_WORDS.iter().copied().collect::<BTreeSet<_>>();
    let mut semantic_terms = prompt_tokens
        .iter()
        .filter(|token| {
            !stop_words.contains(token.as_str()) && !all_aliases.contains(token.as_str())
        })
        .cloned()
        .collect::<Vec<_>>();
    if semantic_terms.is_empty() && matched.is_empty() {
        semantic_terms = prompt_tokens
            .iter()
            .filter(|token| !stop_words.contains(token.as_str()))
            .cloned()
            .collect();
    }
    PlaylistIntent {
        matched_moods: matched
            .iter()
            .map(|profile| profile.name.to_owned())
            .collect(),
        search_terms: semantic_terms,
        energy: mean(
            &matched
                .iter()
                .map(|profile| profile.energy)
                .collect::<Vec<_>>(),
            0.5,
        ),
        brightness: mean(
            &matched
                .iter()
                .map(|profile| profile.brightness)
                .collect::<Vec<_>>(),
            0.5,
        ),
        tension: mean(
            &matched
                .iter()
                .map(|profile| profile.tension)
                .collect::<Vec<_>>(),
            0.5,
        ),
    }
}

pub fn suggest_local_playlist(
    source: &[AssistantTrackEvidence],
    request: &PlaylistSuggestionRequest,
) -> Result<PlaylistSuggestion, String> {
    request.validate()?;
    let prompt = request.prompt.trim();
    let intent = interpret_prompt(prompt);
    let intent_tokens = intent.search_terms.iter().cloned().collect::<BTreeSet<_>>();
    let stop_words = STOP_WORDS.iter().copied().collect::<BTreeSet<_>>();
    let match_terms = tokens(prompt)
        .into_iter()
        .filter(|token| !stop_words.contains(token.as_str()))
        .collect::<BTreeSet<_>>();
    let excluded = request
        .exclude_track_ids
        .iter()
        .copied()
        .collect::<BTreeSet<_>>();
    let mut ranked = Vec::new();
    let mut eligible_tracks = 0usize;
    for evidence in source {
        let signal = current_audio_analysis(evidence);
        if !eligible(&evidence.track, signal.as_ref(), request, &excluded) {
            continue;
        }
        eligible_tracks += 1;
        let view = view_for_track(evidence, None);
        let profile = current_profile(evidence, &view.analysis_tags)
            .unwrap_or_else(|| metadata_profile(&evidence.track));
        ranked.push(rank_track(
            &evidence.track,
            &intent,
            &intent_tokens,
            &match_terms,
            &profile,
            &view.manual_tags,
            signal,
        ));
    }
    ranked.sort_by(|left, right| {
        right
            .score
            .total_cmp(&left.score)
            .then_with(|| left.track.id.cmp(&right.track.id))
    });
    let diversified = diversify(ranked, usize::from(request.candidate_limit));
    let target_seconds = f64::from(request.target_minutes) * 60.0;
    let (default_pool, selected_seconds) = default_pool(&diversified, target_seconds);
    let selected_ids = default_pool
        .iter()
        .map(|item| item.track.id)
        .collect::<BTreeSet<_>>();
    let planned_pool = sequence_default_pool(&default_pool, request.energy_curve);
    let mut planned = planned_pool.clone();
    planned.extend(
        diversified
            .iter()
            .filter(|item| !selected_ids.contains(&item.track.id))
            .cloned(),
    );
    let sequence_positions = planned_pool
        .iter()
        .enumerate()
        .map(|(index, item)| (item.track.id, index + 1))
        .collect::<BTreeMap<_, _>>();
    let candidates = planned
        .iter()
        .map(|item| PlaylistCandidate {
            track_id: item.track.id,
            path: item.track.path.as_str().to_owned(),
            title: item.track.metadata.title.clone(),
            display_title: item.track.display_title.clone(),
            artist: item.track.metadata.artist.clone(),
            album: item.track.metadata.album.clone(),
            origin: item.track.origin.clone(),
            genre: item.track.metadata.genre.clone(),
            manual_tags: item.manual_tags.clone(),
            analysis_tags: item.analysis_tags.clone(),
            length_s: item.track.duration.as_secs_f64().max(0.0),
            bpm: item.track.metadata.bpm,
            match_score: round_to(item.score, 4),
            confidence: item.confidence,
            reasons: item.reasons.clone(),
            default_selected: selected_ids.contains(&item.track.id),
            sequence_position: sequence_positions.get(&item.track.id).copied(),
            planning_energy: round_to(item.planning_energy, 4),
            audio_signal: item.signal.as_ref().map(|signal| PlaylistAudioSignal {
                analyzer_id: signal.analyzer_id.clone(),
                energy: signal.energy,
                brightness: signal.brightness,
                tension: signal.tension,
                tempo_bpm: signal.tempo_bpm,
                confidence: signal.confidence,
            }),
        })
        .collect();
    Ok(PlaylistSuggestion {
        engine: LOCAL_PLAYLIST_ENGINE_ID.to_owned(),
        library_tracks: source.len(),
        eligible_tracks,
        intent,
        plan: PlaylistPlan {
            energy_curve: request.energy_curve,
            selected_tracks: default_pool.len(),
            selected_duration_s: round_to(selected_seconds, 3),
            audio_profile_tracks: planned.iter().filter(|item| item.signal.is_some()).count(),
        },
        candidates,
    })
}

fn current_profile(
    evidence: &AssistantTrackEvidence,
    visible_tags: &[String],
) -> Option<MetadataProfile> {
    let (analysis, confidence) = current_metadata_analysis(evidence)?;
    Some(MetadataProfile {
        energy: analysis.energy,
        brightness: analysis.brightness,
        tension: analysis.tension,
        moods: visible_tags.to_vec(),
        confidence,
    })
}

fn metadata_profile(track: &IndexedTrack) -> MetadataProfile {
    let fields = track_field_tokens(track, &[]);
    let mood_tokens = fields["title"]
        .union(&fields["genre"])
        .cloned()
        .collect::<BTreeSet<_>>()
        .union(&fields["album"])
        .cloned()
        .collect::<BTreeSet<_>>();
    let track_moods = MOODS
        .iter()
        .filter(|profile| {
            profile
                .aliases
                .iter()
                .any(|alias| mood_tokens.contains(*alias))
        })
        .collect::<Vec<_>>();
    let mut energy_values = track_moods
        .iter()
        .map(|profile| profile.energy)
        .collect::<Vec<_>>();
    if let Some(bpm) = track.metadata.bpm {
        energy_values.push(clamp((f64::from(bpm) - 55.0) / 125.0));
    }
    for (genre, prior) in GENRE_ENERGY {
        if fields["genre"].contains(*genre) {
            energy_values.push(*prior);
        }
    }
    let evidence_count = usize::from(!track_moods.is_empty())
        + usize::from(track.metadata.bpm.is_some())
        + usize::from(!track.metadata.genre.is_empty());
    MetadataProfile {
        energy: mean(&energy_values, 0.5),
        brightness: mean(
            &track_moods
                .iter()
                .map(|profile| profile.brightness)
                .collect::<Vec<_>>(),
            0.5,
        ),
        tension: mean(
            &track_moods
                .iter()
                .map(|profile| profile.tension)
                .collect::<Vec<_>>(),
            0.5,
        ),
        moods: track_moods
            .iter()
            .map(|profile| profile.name.to_owned())
            .collect(),
        confidence: if evidence_count >= 3 {
            Confidence::High
        } else if evidence_count > 0 {
            Confidence::Medium
        } else {
            Confidence::Low
        },
    }
}

fn rank_track<'a>(
    track: &'a IndexedTrack,
    intent: &PlaylistIntent,
    intent_tokens: &BTreeSet<String>,
    match_terms: &BTreeSet<String>,
    profile: &MetadataProfile,
    manual_tags: &[String],
    signal: Option<AudioSignalProfile>,
) -> RankedTrack<'a> {
    let fields = track_field_tokens(track, manual_tags);
    let (energy, brightness, tension) = combined_axes(profile, signal.as_ref());
    let (semantic_score, matched_terms) = semantic_match(intent_tokens, &fields);
    let manual_tokens = &fields["manual_tags"];
    let manual_matches = match_terms
        .intersection(manual_tokens)
        .cloned()
        .collect::<Vec<_>>();
    let manual_moods = MOODS
        .iter()
        .filter(|mood| {
            mood.aliases
                .iter()
                .any(|alias| manual_tokens.contains(*alias))
        })
        .map(|mood| mood.name)
        .collect::<BTreeSet<_>>();
    let manual_mood_matches = intent
        .matched_moods
        .iter()
        .filter(|mood| manual_moods.contains(mood.as_str()))
        .cloned()
        .collect::<Vec<_>>();
    let mut weighted = Vec::<(f64, f64)>::new();
    let manual_score = if manual_matches.is_empty() && manual_mood_matches.is_empty() {
        None
    } else {
        let exact = if match_terms.is_empty() {
            0.0
        } else {
            manual_matches.len() as f64 / match_terms.len() as f64
        };
        let mood = if intent.matched_moods.is_empty() {
            0.0
        } else {
            manual_mood_matches.len() as f64 / intent.matched_moods.len() as f64
        };
        let value = exact.max(mood);
        weighted.push((value, 0.75));
        Some(value)
    };
    if !intent.matched_moods.is_empty() {
        let mood_score = mean(
            &[
                1.0 - (intent.energy - energy).abs(),
                1.0 - (intent.brightness - brightness).abs(),
                1.0 - (intent.tension - tension).abs(),
            ],
            0.5,
        );
        weighted.push((mood_score, if weighted.is_empty() { 0.68 } else { 0.55 }));
    }
    if !intent_tokens.is_empty() {
        weighted.push((semantic_score, if weighted.is_empty() { 1.0 } else { 0.3 }));
    }
    if weighted.is_empty() {
        weighted.push((0.5, 1.0));
    }
    let mut evidence_count = matched_terms.len()
        + profile.moods.len()
        + manual_matches.len()
        + manual_mood_matches.len()
        + usize::from(track.metadata.bpm.is_some() && !intent.matched_moods.is_empty())
        + usize::from(!track.metadata.genre.is_empty());
    if let Some(signal) = &signal {
        evidence_count += if signal.confidence == Confidence::High {
            2
        } else {
            1
        };
    }
    let raw_score = weighted
        .iter()
        .map(|(value, weight)| value * weight)
        .sum::<f64>()
        / weighted.iter().map(|(_, weight)| weight).sum::<f64>();
    let reliability = (evidence_count as f64 / 3.0).min(1.0);
    let mut score = clamp(raw_score * (0.88 + 0.12 * reliability));
    if let Some(manual_score) = manual_score {
        score = score.max(0.65 + 0.35 * manual_score);
    }
    let mut reasons = Vec::new();
    if !manual_matches.is_empty() || !manual_mood_matches.is_empty() {
        let matched = manual_tags
            .iter()
            .filter(|tag| {
                let tag_tokens = tokens(tag);
                !tag_tokens.is_disjoint(match_terms)
                    || MOODS.iter().any(|mood| {
                        manual_mood_matches.iter().any(|name| name == mood.name)
                            && mood.aliases.iter().any(|alias| tag_tokens.contains(*alias))
                    })
            })
            .take(3)
            .cloned()
            .collect::<Vec<_>>();
        reasons.push(format!("Your tags: {}", matched.join(", ")));
    }
    if !matched_terms.is_empty() {
        reasons.push(format!(
            "Metadata matches: {}",
            matched_terms
                .iter()
                .take(3)
                .cloned()
                .collect::<Vec<_>>()
                .join(", ")
        ));
    }
    if !profile.moods.is_empty() {
        reasons.push(format!(
            "Mood metadata: {}",
            profile
                .moods
                .iter()
                .take(2)
                .cloned()
                .collect::<Vec<_>>()
                .join(", ")
        ));
    }
    let effective_bpm = track
        .metadata
        .bpm
        .map(f64::from)
        .or_else(|| signal.as_ref().and_then(|item| item.tempo_bpm));
    if let Some(bpm) = effective_bpm
        && !intent.matched_moods.is_empty()
    {
        let pace = if intent.energy < 0.4 {
            "calmer"
        } else {
            "higher-energy"
        };
        if (intent.energy - energy).abs() <= 0.25 {
            let source = if track.metadata.bpm.is_none() {
                "Measured tempo"
            } else {
                "Tempo"
            };
            reasons.push(format!(
                "{source}: {bpm:.0} BPM supports the requested {pace} pace"
            ));
        } else {
            let source = if track.metadata.bpm.is_none() {
                "Measured tempo"
            } else {
                "Tempo evidence"
            };
            reasons.push(format!("{source}: {bpm:.0} BPM"));
        }
    }
    if let Some(signal) = &signal
        && reasons.len() < 4
    {
        if (intent.energy - signal.energy).abs() <= 0.25 {
            reasons.push("Measured audio energy supports the requested flow".to_owned());
        } else {
            reasons.push(format!(
                "Measured audio energy: {:.0}%",
                signal.energy * 100.0
            ));
        }
    }
    if !track.metadata.genre.is_empty() && reasons.len() < 3 {
        reasons.push(format!("Genre metadata: {}", track.metadata.genre));
    }
    if reasons.is_empty() {
        reasons.push("Limited mood metadata; this is a low-confidence local match".to_owned());
    }
    reasons.truncate(4);
    RankedTrack {
        track,
        score,
        confidence: if evidence_count >= 3 {
            Confidence::High
        } else if evidence_count > 0 {
            Confidence::Medium
        } else {
            Confidence::Low
        },
        reasons,
        manual_tags: manual_tags.to_vec(),
        analysis_tags: profile.moods.clone(),
        planning_energy: energy,
        signal,
    }
}

fn combined_axes(
    profile: &MetadataProfile,
    signal: Option<&AudioSignalProfile>,
) -> (f64, f64, f64) {
    let Some(signal) = signal else {
        return (profile.energy, profile.brightness, profile.tension);
    };
    (
        blend_axis(
            profile.energy,
            profile.confidence,
            signal.energy,
            signal.confidence,
            1.0,
        ),
        blend_axis(
            profile.brightness,
            profile.confidence,
            signal.brightness,
            signal.confidence,
            0.85,
        ),
        blend_axis(
            profile.tension,
            profile.confidence,
            signal.tension,
            signal.confidence,
            0.6,
        ),
    )
}

fn blend_axis(
    metadata: f64,
    metadata_confidence: Confidence,
    signal: f64,
    signal_confidence: Confidence,
    multiplier: f64,
) -> f64 {
    let metadata_weight = metadata_confidence.weight();
    let signal_weight = signal_confidence.weight() * multiplier;
    clamp((metadata * metadata_weight + signal * signal_weight) / (metadata_weight + signal_weight))
}

fn semantic_match(
    terms: &BTreeSet<String>,
    fields: &BTreeMap<&str, BTreeSet<String>>,
) -> (f64, Vec<String>) {
    if terms.is_empty() {
        return (0.0, Vec::new());
    }
    const WEIGHTS: &[(&str, f64)] = &[
        ("title", 1.4),
        ("genre", 1.4),
        ("origin", 1.2),
        ("album", 0.9),
        ("artist", 0.6),
        ("path", 0.5),
        ("manual_tags", 2.0),
    ];
    let mut matched = Vec::new();
    let mut total = 0.0;
    for term in terms {
        let best = WEIGHTS
            .iter()
            .filter(|(field, _)| fields[*field].contains(term))
            .map(|(_, weight)| *weight)
            .fold(0.0_f64, f64::max);
        if best > 0.0 {
            matched.push(term.clone());
            total += best;
        }
    }
    (clamp(total / (terms.len() as f64 * 1.4)), matched)
}

fn track_field_tokens(
    track: &IndexedTrack,
    manual_tags: &[String],
) -> BTreeMap<&'static str, BTreeSet<String>> {
    let title = if track.display_title.trim().is_empty() {
        &track.metadata.title
    } else {
        &track.display_title
    };
    BTreeMap::from([
        ("title", tokens(title)),
        ("genre", tokens(&track.metadata.genre)),
        ("origin", tokens(&track.origin)),
        ("album", tokens(&track.metadata.album)),
        ("artist", tokens(&track.metadata.artist)),
        ("path", tokens(track.path.as_str())),
        (
            "manual_tags",
            manual_tags.iter().flat_map(|tag| tokens(tag)).collect(),
        ),
    ])
}

fn eligible(
    track: &IndexedTrack,
    signal: Option<&AudioSignalProfile>,
    request: &PlaylistSuggestionRequest,
    excluded: &BTreeSet<TrackId>,
) -> bool {
    if excluded.contains(&track.id) {
        return false;
    }
    let bpm = track
        .metadata
        .bpm
        .map(f64::from)
        .or_else(|| signal.and_then(|item| item.tempo_bpm));
    let Some(bpm) = bpm else {
        return request.include_unknown_bpm;
    };
    request
        .min_bpm
        .is_none_or(|minimum| bpm >= f64::from(minimum))
        && request
            .max_bpm
            .is_none_or(|maximum| bpm <= f64::from(maximum))
}

fn diversify<'a>(mut remaining: Vec<RankedTrack<'a>>, limit: usize) -> Vec<RankedTrack<'a>> {
    let mut selected = Vec::new();
    let mut artist_counts = BTreeMap::<String, usize>::new();
    let mut album_counts = BTreeMap::<String, usize>::new();
    let mut origin_counts = BTreeMap::<String, usize>::new();
    while !remaining.is_empty() && selected.len() < limit {
        let winner = remaining
            .iter()
            .enumerate()
            .max_by(|(_, left), (_, right)| {
                adjusted(left, &artist_counts, &album_counts, &origin_counts)
                    .partial_cmp(&adjusted(
                        right,
                        &artist_counts,
                        &album_counts,
                        &origin_counts,
                    ))
                    .unwrap_or(Ordering::Equal)
            })
            .map(|(index, _)| index)
            .unwrap_or_default();
        let winner = remaining.remove(winner);
        for (value, counts) in [
            (&winner.track.metadata.artist, &mut artist_counts),
            (&winner.track.metadata.album, &mut album_counts),
            (&winner.track.origin, &mut origin_counts),
        ] {
            let key = value.trim().to_lowercase();
            if !key.is_empty() {
                *counts.entry(key).or_default() += 1;
            }
        }
        selected.push(winner);
    }
    selected
}

fn adjusted(
    candidate: &RankedTrack<'_>,
    artist_counts: &BTreeMap<String, usize>,
    album_counts: &BTreeMap<String, usize>,
    origin_counts: &BTreeMap<String, usize>,
) -> (OrderedFloat, OrderedFloat, std::cmp::Reverse<i64>) {
    let artist = candidate.track.metadata.artist.trim().to_lowercase();
    let album = candidate.track.metadata.album.trim().to_lowercase();
    let origin = candidate.track.origin.trim().to_lowercase();
    let penalty = if artist.is_empty() {
        0.0
    } else {
        (0.08 * *artist_counts.get(&artist).unwrap_or(&0) as f64).min(0.18)
    } + if album.is_empty() {
        0.0
    } else {
        (0.05 * *album_counts.get(&album).unwrap_or(&0) as f64).min(0.12)
    } + if origin.is_empty() {
        0.0
    } else {
        (0.04 * *origin_counts.get(&origin).unwrap_or(&0) as f64).min(0.08)
    };
    (
        OrderedFloat(candidate.score - penalty),
        OrderedFloat(candidate.score),
        std::cmp::Reverse(candidate.track.id.get()),
    )
}

#[derive(Debug, Clone, Copy, PartialEq)]
struct OrderedFloat(f64);

impl Eq for OrderedFloat {}

impl Ord for OrderedFloat {
    fn cmp(&self, other: &Self) -> Ordering {
        self.0.total_cmp(&other.0)
    }
}

impl PartialOrd for OrderedFloat {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

fn planning_duration(item: &RankedTrack<'_>) -> f64 {
    let duration = item.track.duration.as_secs_f64();
    if duration > 0.0 { duration } else { 180.0 }
}

fn default_pool<'a>(
    ranked: &[RankedTrack<'a>],
    target_seconds: f64,
) -> (Vec<RankedTrack<'a>>, f64) {
    if ranked.is_empty() {
        return (Vec::new(), 0.0);
    }
    let durations = ranked.iter().map(planning_duration).collect::<Vec<_>>();
    let mut selected = BTreeSet::<usize>::new();
    let mut selected_seconds = 0.0;
    for (index, duration) in durations.iter().enumerate() {
        if (selected_seconds + duration - target_seconds).abs()
            < (selected_seconds - target_seconds).abs()
        {
            selected.insert(index);
            selected_seconds += duration;
        }
    }
    if selected.is_empty() {
        let closest = durations
            .iter()
            .enumerate()
            .min_by(|(left_index, left), (right_index, right)| {
                (*left - target_seconds)
                    .abs()
                    .total_cmp(&(*right - target_seconds).abs())
                    .then_with(|| left_index.cmp(right_index))
            })
            .map(|(index, _)| index)
            .unwrap_or_default();
        selected.insert(closest);
        selected_seconds = durations[closest];
    }
    loop {
        let mut best_error = (selected_seconds - target_seconds).abs();
        let mut best: Option<(BTreeSet<usize>, f64)> = None;
        for (index, duration) in durations.iter().enumerate() {
            let (candidate, seconds) = if selected.contains(&index) {
                if selected.len() == 1 {
                    continue;
                }
                let mut candidate = selected.clone();
                candidate.remove(&index);
                (candidate, selected_seconds - duration)
            } else {
                let mut candidate = selected.clone();
                candidate.insert(index);
                (candidate, selected_seconds + duration)
            };
            let error = (seconds - target_seconds).abs();
            if error < best_error {
                best_error = error;
                best = Some((candidate, seconds));
            }
        }
        for removed in selected.iter().copied() {
            for (added, duration) in durations.iter().enumerate() {
                if selected.contains(&added) {
                    continue;
                }
                let seconds = selected_seconds - durations[removed] + duration;
                let error = (seconds - target_seconds).abs();
                if error < best_error {
                    let mut candidate = selected.clone();
                    candidate.remove(&removed);
                    candidate.insert(added);
                    best_error = error;
                    best = Some((candidate, seconds));
                }
            }
        }
        let Some((next, seconds)) = best else {
            break;
        };
        selected = next;
        selected_seconds = seconds;
    }
    (
        ranked
            .iter()
            .enumerate()
            .filter(|(index, _)| selected.contains(index))
            .map(|(_, item)| item.clone())
            .collect(),
        selected_seconds,
    )
}

fn sequence_default_pool<'a>(
    items: &[RankedTrack<'a>],
    curve: EnergyCurve,
) -> Vec<RankedTrack<'a>> {
    if curve == EnergyCurve::Steady || items.len() < 2 {
        return items.to_vec();
    }
    let rank = items
        .iter()
        .enumerate()
        .map(|(index, item)| (item.track.id, index))
        .collect::<BTreeMap<_, _>>();
    if matches!(curve, EnergyCurve::Rising | EnergyCurve::Falling) {
        let mut ordered = items.to_vec();
        ordered.sort_by(|left, right| {
            let energy = left.planning_energy.total_cmp(&right.planning_energy);
            let energy = if curve == EnergyCurve::Falling {
                energy.reverse()
            } else {
                energy
            };
            energy.then_with(|| rank[&left.track.id].cmp(&rank[&right.track.id]))
        });
        return ordered;
    }
    let mut remaining = items.to_vec();
    let mut ordered = Vec::new();
    for target in arc_targets(items) {
        let winner = remaining
            .iter()
            .enumerate()
            .min_by(|(_, left), (_, right)| {
                (left.planning_energy - target)
                    .abs()
                    .total_cmp(&(right.planning_energy - target).abs())
                    .then_with(|| rank[&left.track.id].cmp(&rank[&right.track.id]))
            })
            .map(|(index, _)| index)
            .unwrap_or_default();
        ordered.push(remaining.remove(winner));
    }
    ordered
}

fn arc_targets(items: &[RankedTrack<'_>]) -> Vec<f64> {
    if items.is_empty() {
        return Vec::new();
    }
    let low = items
        .iter()
        .map(|item| item.planning_energy)
        .fold(f64::INFINITY, f64::min);
    let high = items
        .iter()
        .map(|item| item.planning_energy)
        .fold(f64::NEG_INFINITY, f64::max);
    if items.len() == 1 || high - low < 0.05 {
        return vec![items[0].planning_energy; items.len()];
    }
    let peak = 0.65;
    (0..items.len())
        .map(|index| {
            let position = index as f64 / (items.len() - 1) as f64;
            if position <= peak {
                low + (high - low) * (position / peak)
            } else {
                high - (high - low) * 0.65 * ((position - peak) / (1.0 - peak))
            }
        })
        .collect()
}

fn tokens(value: &str) -> BTreeSet<String> {
    let normalized = value
        .to_lowercase()
        .nfkd()
        .filter(|character| {
            !matches!(
                get_general_category(*character),
                GeneralCategory::NonspacingMark
                    | GeneralCategory::SpacingMark
                    | GeneralCategory::EnclosingMark
            )
        })
        .collect::<String>();
    let mut result = BTreeSet::new();
    let mut current = String::new();
    for character in normalized.chars() {
        if character.is_ascii_lowercase() || character.is_ascii_digit() {
            current.push(character);
        } else if !current.is_empty() {
            result.insert(std::mem::take(&mut current));
        }
    }
    if !current.is_empty() {
        result.insert(current);
    }
    result
}

fn mean(values: &[f64], default: f64) -> f64 {
    if values.is_empty() {
        default
    } else {
        values.iter().sum::<f64>() / values.len() as f64
    }
}

fn clamp(value: f64) -> f64 {
    value.clamp(0.0, 1.0)
}

fn round_to(value: f64, digits: i32) -> f64 {
    let factor = 10_f64.powi(digits);
    (value * factor).round() / factor
}

#[cfg(test)]
mod tests {
    use std::error::Error;
    use std::time::Duration;

    use music_domain::{LibraryPath, TrackMetadata};

    use super::*;

    fn source(
        id: i64,
        title: &str,
        genre: &str,
        bpm: u32,
        duration: u64,
        tags: &[&str],
    ) -> Result<AssistantTrackEvidence, Box<dyn Error>> {
        Ok(AssistantTrackEvidence {
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
                    genre: genre.to_owned(),
                    bpm: Some(bpm),
                },
                duration: Duration::from_secs(duration),
                display_title: title.to_owned(),
                origin: String::new(),
                size_bytes: 1,
                mtime_unix_seconds: 1,
                added_at_unix_seconds: 1,
            },
            manual_tags: tags.iter().map(|tag| (*tag).to_owned()).collect(),
            analyses: Vec::new(),
            reviews: Vec::new(),
        })
    }

    fn request(prompt: &str) -> PlaylistSuggestionRequest {
        PlaylistSuggestionRequest {
            prompt: prompt.to_owned(),
            target_minutes: 5,
            candidate_limit: 40,
            min_bpm: None,
            max_bpm: None,
            include_unknown_bpm: true,
            exclude_track_ids: Vec::new(),
            energy_curve: EnergyCurve::Steady,
        }
    }

    #[test]
    fn combat_and_calm_requests_rank_different_tracks() -> Result<(), Box<dyn Error>> {
        let sources = vec![
            source(1, "Quiet Rest", "ambient", 60, 180, &[])?,
            source(2, "Final Battle", "metal", 160, 180, &[])?,
        ];
        let calm = suggest_local_playlist(&sources, &request("calm music"))?;
        let combat = suggest_local_playlist(&sources, &request("combat music"))?;
        assert_eq!(calm.candidates[0].track_id, TrackId::new(1)?);
        assert_eq!(combat.candidates[0].track_id, TrackId::new(2)?);
        Ok(())
    }

    #[test]
    fn manual_tags_are_authoritative_but_never_auto_applied() -> Result<(), Box<dyn Error>> {
        let sources = vec![
            source(1, "Unlabeled", "", 100, 180, &["stealth"])?,
            source(2, "Generic", "", 100, 180, &[])?,
        ];
        let suggestion = suggest_local_playlist(&sources, &request("stealth playlist"))?;
        assert_eq!(suggestion.candidates[0].track_id, TrackId::new(1)?);
        assert!(suggestion.candidates[0].reasons[0].starts_with("Your tags:"));
        Ok(())
    }

    #[test]
    fn energy_curves_only_reorder_the_default_selection() -> Result<(), Box<dyn Error>> {
        let sources = vec![
            source(1, "Low", "ambient", 60, 100, &[])?,
            source(2, "Middle", "soundtrack", 100, 100, &[])?,
            source(3, "High", "metal", 160, 100, &[])?,
        ];
        let mut rising = request("adventure");
        rising.energy_curve = EnergyCurve::Rising;
        let suggestion = suggest_local_playlist(&sources, &rising)?;
        let selected = suggestion
            .candidates
            .iter()
            .filter(|item| item.default_selected)
            .collect::<Vec<_>>();
        assert!(
            selected
                .windows(2)
                .all(|pair| pair[0].planning_energy <= pair[1].planning_energy)
        );
        Ok(())
    }
}
