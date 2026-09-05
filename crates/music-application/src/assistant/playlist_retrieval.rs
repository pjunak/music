use std::collections::BTreeSet;

use unicode_normalization::UnicodeNormalization;

use super::{
    AssistantTrackEvidence, PlaylistSuggestion, PlaylistSuggestionRequest, TagVocabularyDocument,
    normalize_manual_tag,
};

/// Supplement a local plan using only operator-authored labels and vocabulary mappings.
/// Keep every default selection and bound recall to a quarter of the pool (at most 20).
pub(super) fn supplement_candidates(
    source: &[AssistantTrackEvidence],
    request: &PlaylistSuggestionRequest,
    vocabulary: &TagVocabularyDocument,
    baseline: &mut PlaylistSuggestion,
) -> Result<(), String> {
    let prompt = phrase_tokens(&request.prompt);
    let labels = vocabulary
        .groups
        .iter()
        .flat_map(|group| &group.tags)
        .filter(|entry| {
            std::iter::once(&entry.name)
                .chain(&entry.aliases)
                .chain(&entry.context_cues)
                .any(|term| contains_phrase(&prompt, &phrase_tokens(term)))
        })
        .flat_map(|entry| std::iter::once(&entry.name).chain(&entry.aliases))
        .filter_map(|term| normalize_manual_tag(term).ok())
        .collect::<BTreeSet<_>>();
    if labels.is_empty() {
        return Ok(());
    }
    let limit = usize::from(request.candidate_limit);
    let selected = baseline
        .candidates
        .iter()
        .filter(|candidate| candidate.default_selected)
        .count();
    let quota = (limit / 4).min(20).min(limit.saturating_sub(selected));
    if quota == 0 {
        return Ok(());
    }
    let existing = baseline
        .candidates
        .iter()
        .map(|candidate| candidate.track_id)
        .collect::<BTreeSet<_>>();
    let matches = source.iter().filter(|track| {
        !existing.contains(&track.track.id)
            && track
                .manual_tags
                .iter()
                .filter_map(|tag| normalize_manual_tag(tag).ok())
                .any(|tag| labels.contains(&tag))
    });
    // The same planner rechecks excluded IDs, known/unknown/measured BPM, and source validity.
    let mut recalled = super::planner::suggest_local_playlist_from(matches, request)?.candidates;
    recalled.truncate(quota);
    let mut ordinary_slots = limit.saturating_sub(selected + recalled.len());
    baseline.candidates.retain(|candidate| {
        if candidate.default_selected {
            return true;
        }
        if ordinary_slots == 0 {
            return false;
        }
        ordinary_slots -= 1;
        true
    });
    for candidate in &mut recalled {
        candidate.default_selected = false;
        candidate.sequence_position = None;
    }
    baseline.candidates.extend(recalled);
    baseline.plan.audio_profile_tracks = baseline
        .candidates
        .iter()
        .filter(|candidate| candidate.audio_signal.is_some())
        .count();
    Ok(())
}

fn phrase_tokens(value: &str) -> Vec<String> {
    value
        .nfkc()
        .flat_map(char::to_lowercase)
        .collect::<String>()
        .split(|character: char| !character.is_alphanumeric())
        .filter(|word| !word.is_empty())
        .map(str::to_owned)
        .collect()
}

fn contains_phrase(text: &[String], phrase: &[String]) -> bool {
    !phrase.is_empty() && text.windows(phrase.len()).any(|window| window == phrase)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recall_phrases_require_complete_ordered_words_and_normalize_unicode() {
        assert!(contains_phrase(
            &phrase_tokens("A ＣＯＶＥＲＴ—approach tonight"),
            &phrase_tokens("covert approach")
        ));
        assert!(!contains_phrase(
            &phrase_tokens("approach covert"),
            &phrase_tokens("covert approach")
        ));
        assert!(!contains_phrase(
            &phrase_tokens("inner courtyard"),
            &phrase_tokens("inn")
        ));
        assert!(!contains_phrase(
            &phrase_tokens("covert"),
            &phrase_tokens("covert approach")
        ));
        assert!(!contains_phrase(
            &phrase_tokens("anything"),
            &phrase_tokens("")
        ));
        assert!(contains_phrase(
            &phrase_tokens("静かな儀式"),
            &phrase_tokens("静かな儀式")
        ));
    }
}
