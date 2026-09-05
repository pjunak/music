use std::collections::{BTreeMap, BTreeSet};

use music_application::assistant::{
    AnalysisReviewDecision, AnalysisReviewFailure, AnalysisReviewFailureCode, AnalysisReviewTarget,
    MAX_TAGS_PER_TRACK,
};

use super::review_failure;

pub(super) struct PlannedReview {
    pub target: AnalysisReviewTarget,
    pub insert_manual_tag: bool,
}

pub(super) struct ReviewPlan {
    pub operations: Vec<PlannedReview>,
    pub failures: Vec<AnalysisReviewFailure>,
}

/// Plan only targets already checked against current evidence in the transaction.
/// Reject all new tags for an overflowing track instead of choosing an arbitrary
/// subset by request order. Existing tags may still receive an explicit decision.
pub(super) fn plan_decisions(
    targets: Vec<AnalysisReviewTarget>,
    decision: AnalysisReviewDecision,
    mut manual: BTreeMap<i64, BTreeSet<String>>,
) -> ReviewPlan {
    let mut additions = BTreeMap::<i64, BTreeSet<String>>::new();
    if decision == AnalysisReviewDecision::Accepted {
        for target in &targets {
            if !manual
                .entry(target.track_id.get())
                .or_default()
                .contains(&target.tag)
            {
                additions
                    .entry(target.track_id.get())
                    .or_default()
                    .insert(target.tag.clone());
            }
        }
    }
    let overflow = additions
        .iter()
        .filter(|(track_id, tags)| {
            manual.get(track_id).map_or(0, BTreeSet::len) + tags.len() > MAX_TAGS_PER_TRACK
        })
        .map(|(track_id, _)| *track_id)
        .collect::<BTreeSet<_>>();
    let mut plan = ReviewPlan {
        operations: Vec::new(),
        failures: Vec::new(),
    };
    for target in targets {
        let current_tags = manual.entry(target.track_id.get()).or_default();
        if decision == AnalysisReviewDecision::Accepted
            && overflow.contains(&target.track_id.get())
            && !current_tags.contains(&target.tag)
        {
            plan.failures.push(review_failure(
                &target,
                AnalysisReviewFailureCode::TagLimit,
                &format!("selected suggestions would exceed the {MAX_TAGS_PER_TRACK}-tag limit"),
            ));
            continue;
        }
        let insert_manual_tag =
            decision == AnalysisReviewDecision::Accepted && current_tags.insert(target.tag.clone());
        plan.operations.push(PlannedReview {
            target,
            insert_manual_tag,
        });
    }
    plan
}

#[cfg(test)]
mod tests {
    use super::*;
    use music_domain::TrackId;

    #[test]
    fn duplicate_sources_share_capacity_and_overflow_never_selects_an_arbitrary_subset()
    -> Result<(), Box<dyn std::error::Error>> {
        let track_id = TrackId::new(1)?;
        let target = |tag: &str, analyzer: &str| AnalysisReviewTarget {
            track_id,
            tag: tag.to_owned(),
            analyzer_id: analyzer.to_owned(),
            source_signature: "current".to_owned(),
        };
        let manual = BTreeMap::from([(
            1,
            (0..MAX_TAGS_PER_TRACK - 1)
                .map(|index| format!("existing-{index}"))
                .collect::<BTreeSet<_>>(),
        )]);
        let shared = plan_decisions(
            vec![target("calm", "local"), target("calm", "catalog")],
            AnalysisReviewDecision::Accepted,
            manual.clone(),
        );
        assert!(shared.failures.is_empty());
        assert_eq!(shared.operations.len(), 2);
        assert_eq!(
            shared
                .operations
                .iter()
                .filter(|item| item.insert_manual_tag)
                .count(),
            1
        );
        for tags in [["calm", "dark"], ["dark", "calm"]] {
            let overflow = plan_decisions(
                vec![
                    target(tags[0], "local"),
                    target("existing-0", "local"),
                    target(tags[1], "catalog"),
                ],
                AnalysisReviewDecision::Accepted,
                manual.clone(),
            );
            assert_eq!(overflow.operations.len(), 1);
            assert_eq!(overflow.operations[0].target.tag, "existing-0");
            assert!(!overflow.operations[0].insert_manual_tag);
            assert_eq!(overflow.failures.len(), 2);
            assert!(
                overflow
                    .failures
                    .iter()
                    .all(|failure| failure.code == AnalysisReviewFailureCode::TagLimit)
            );
        }
        Ok(())
    }

    #[test]
    fn rejection_and_reopening_never_insert_tags_even_at_capacity()
    -> Result<(), Box<dyn std::error::Error>> {
        for decision in [
            AnalysisReviewDecision::Rejected,
            AnalysisReviewDecision::Pending,
        ] {
            let plan = plan_decisions(
                vec![AnalysisReviewTarget {
                    track_id: TrackId::new(1)?,
                    tag: "new suggestion".to_owned(),
                    analyzer_id: "local".to_owned(),
                    source_signature: "current".to_owned(),
                }],
                decision,
                BTreeMap::from([(
                    1,
                    (0..MAX_TAGS_PER_TRACK)
                        .map(|index| index.to_string())
                        .collect(),
                )]),
            );
            assert!(plan.failures.is_empty());
            assert_eq!(plan.operations.len(), 1);
            assert!(!plan.operations[0].insert_manual_tag);
        }
        Ok(())
    }
}
