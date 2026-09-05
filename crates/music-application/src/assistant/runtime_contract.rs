use sha2::{Digest, Sha256};

/// Versioned root for the server-owned model execution contract.
///
/// The digest deliberately includes the executable task definitions, validators,
/// evaluation suites, and the local evidence/vocabulary code on which model
/// inputs depend. A stored conformance or quality result therefore becomes stale
/// whenever one of those artifacts changes.
pub const ASSISTANT_RUNTIME_CONTRACT_VERSION: &str = "assistant-runtime-contract/v2";

const ASSISTANT_RUNTIME_ARTIFACTS: &[(&str, &str)] = &[
    // Derivation and deserialization depend on locked library versions as well
    // as task source. Dependency changes conservatively expire every role.
    ("runtime/Cargo.lock", include_str!("../../../../Cargo.lock")),
    (
        "assistant/runtime_contract.rs",
        include_str!("runtime_contract.rs"),
    ),
    ("assistant/mod.rs", include_str!("mod.rs")),
    (
        "assistant/local_analysis.rs",
        include_str!("local_analysis.rs"),
    ),
    ("assistant/model_eq.rs", include_str!("model_eq.rs")),
    ("assistant/model_jobs.rs", include_str!("model_jobs.rs")),
    (
        "assistant/model_jobs/eq.rs",
        include_str!("model_jobs/eq.rs"),
    ),
    (
        "assistant/model_jobs/playlist.rs",
        include_str!("model_jobs/playlist.rs"),
    ),
    (
        "assistant/model_jobs/tag_cleanup.rs",
        include_str!("model_jobs/tag_cleanup.rs"),
    ),
    (
        "assistant/model_jobs/tagging.rs",
        include_str!("model_jobs/tagging.rs"),
    ),
    (
        "assistant/model_transport.rs",
        include_str!("model_transport.rs"),
    ),
    (
        "assistant/model_playlist.rs",
        include_str!("model_playlist.rs"),
    ),
    (
        "assistant/model_quality.rs",
        include_str!("model_quality.rs"),
    ),
    (
        "assistant/model_tag_cleanup.rs",
        include_str!("model_tag_cleanup.rs"),
    ),
    ("assistant/model_tagger.rs", include_str!("model_tagger.rs")),
    (
        "assistant/tagging_evaluation.rs",
        include_str!("tagging_evaluation.rs"),
    ),
    (
        "assistant/evaluation_suites/tagging-custom-vocabulary-v1.json",
        include_str!("evaluation_suites/tagging-custom-vocabulary-v1.json"),
    ),
    ("assistant/planner.rs", include_str!("planner.rs")),
    (
        "assistant/playlist_retrieval.rs",
        include_str!("playlist_retrieval.rs"),
    ),
    (
        "assistant/playlist_evaluation.rs",
        include_str!("playlist_evaluation.rs"),
    ),
    (
        "assistant/provider_usage.rs",
        include_str!("provider_usage.rs"),
    ),
    ("assistant/providers.rs", include_str!("providers.rs")),
    (
        "assistant/structured_harness.rs",
        include_str!("structured_harness.rs"),
    ),
    ("assistant/tags.rs", include_str!("tags.rs")),
    ("assistant/vocabulary.rs", include_str!("vocabulary.rs")),
    (
        "assistant/default_vocabulary.json",
        include_str!("../default_vocabulary.json"),
    ),
    (
        "assistant/evaluation_suites/eq-assistant-v1.json",
        include_str!("evaluation_suites/eq-assistant-v1.json"),
    ),
    (
        "assistant/evaluation_suites/music-tagging-v1.json",
        include_str!("evaluation_suites/music-tagging-v1.json"),
    ),
    (
        "assistant/evaluation_suites/playlist-local-v1.json",
        include_str!("evaluation_suites/playlist-local-v1.json"),
    ),
    (
        "assistant/evaluation_suites/playlist-model-v1.json",
        include_str!("evaluation_suites/playlist-model-v1.json"),
    ),
    (
        "assistant/evaluation_suites/tag-cleanup-v1.json",
        include_str!("evaluation_suites/tag-cleanup-v1.json"),
    ),
];

#[must_use]
pub fn assistant_runtime_contract_digest() -> String {
    artifact_digest(None, ASSISTANT_RUNTIME_ARTIFACTS)
}

#[must_use]
pub fn assistant_role_runtime_contract_digest(role_id: &str) -> String {
    artifact_digest(Some(role_id), ASSISTANT_RUNTIME_ARTIFACTS)
}

fn artifact_digest(role_id: Option<&str>, artifacts: &[(&str, &str)]) -> String {
    let mut digest = Sha256::new();
    update_artifact(
        &mut digest,
        "contract-version",
        ASSISTANT_RUNTIME_CONTRACT_VERSION,
    );
    for (name, contents) in artifacts {
        if role_id.is_none_or(|role| artifact_affects_role(name, role)) {
            update_artifact(&mut digest, name, contents);
        }
    }
    format!("{:x}", digest.finalize())
}

// Unknown artifacts and roles remain shared. Narrow only explicitly reviewed
// role closures; validator and orchestration code remain part of certification.
fn artifact_affects_role(name: &str, role: &str) -> bool {
    if !matches!(
        role,
        "eq_assistant" | "playlist_planner" | "music_tagger" | "tag_cleanup"
    ) {
        return true;
    }
    match name {
        "assistant/model_eq.rs"
        | "assistant/model_jobs/eq.rs"
        | "assistant/evaluation_suites/eq-assistant-v1.json" => role == "eq_assistant",
        "assistant/model_playlist.rs"
        | "assistant/playlist_retrieval.rs"
        | "assistant/planner.rs"
        | "assistant/playlist_evaluation.rs"
        | "assistant/model_jobs/playlist.rs"
        | "assistant/evaluation_suites/playlist-local-v1.json"
        | "assistant/evaluation_suites/playlist-model-v1.json" => role == "playlist_planner",
        "assistant/model_tagger.rs"
        | "assistant/model_jobs/tagging.rs"
        | "assistant/evaluation_suites/music-tagging-v1.json" => role == "music_tagger",
        "assistant/model_jobs/tag_cleanup.rs"
        | "assistant/evaluation_suites/tag-cleanup-v1.json" => role == "tag_cleanup",
        // This module also owns the default vocabulary snapshot used by tagging.
        "assistant/model_tag_cleanup.rs" => matches!(role, "tag_cleanup" | "music_tagger"),
        "assistant/tagging_evaluation.rs"
        | "assistant/evaluation_suites/tagging-custom-vocabulary-v1.json" => {
            matches!(role, "tag_cleanup" | "music_tagger")
        }
        _ => true,
    }
}

fn update_artifact(digest: &mut Sha256, name: &str, contents: &str) {
    digest.update(u64::try_from(name.len()).unwrap_or(u64::MAX).to_le_bytes());
    digest.update(name.as_bytes());
    digest.update(
        u64::try_from(contents.len())
            .unwrap_or(u64::MAX)
            .to_le_bytes(),
    );
    digest.update(contents.as_bytes());
}

#[cfg(test)]
mod tests {
    use super::{ASSISTANT_RUNTIME_CONTRACT_VERSION, assistant_runtime_contract_digest};

    #[test]
    fn runtime_contract_digest_is_stable_and_content_addressed() {
        let digest = assistant_runtime_contract_digest();
        assert_eq!(digest.len(), 64);
        assert!(digest.bytes().all(|byte| byte.is_ascii_hexdigit()));
        assert_eq!(digest, assistant_runtime_contract_digest());
        assert_ne!(digest, ASSISTANT_RUNTIME_CONTRACT_VERSION);
    }

    #[test]
    fn role_closures_invalidate_relevant_code_and_keep_shared_changes_fail_closed() {
        let baseline = [
            ("assistant/model_eq.rs", "eq-v1"),
            ("assistant/model_tagger.rs", "tag-v1"),
            ("assistant/providers.rs", "policy-v1"),
        ];
        let mut changed = baseline;
        changed[0].1 = "eq-v2";
        assert_ne!(
            super::artifact_digest(Some("eq_assistant"), &baseline),
            super::artifact_digest(Some("eq_assistant"), &changed)
        );
        assert_eq!(
            super::artifact_digest(Some("music_tagger"), &baseline),
            super::artifact_digest(Some("music_tagger"), &changed)
        );
        changed[2].1 = "policy-v2";
        for role in [
            "eq_assistant",
            "music_tagger",
            "playlist_planner",
            "tag_cleanup",
            "future_role",
        ] {
            assert_ne!(
                super::artifact_digest(Some(role), &baseline),
                super::artifact_digest(Some(role), &changed)
            );
            assert!(super::artifact_affects_role(
                "assistant/new_policy.rs",
                role
            ));
            assert!(super::artifact_affects_role("runtime/Cargo.lock", role));
        }
        assert!(super::artifact_affects_role(
            "assistant/model_tag_cleanup.rs",
            "music_tagger"
        ));
    }

    #[test]
    fn every_assistant_runtime_source_and_suite_has_digest_coverage()
    -> Result<(), Box<dyn std::error::Error>> {
        fn visit(
            path: &std::path::Path,
            root: &std::path::Path,
            found: &mut std::collections::BTreeSet<String>,
        ) -> std::io::Result<()> {
            for entry in std::fs::read_dir(path)? {
                let entry = entry?;
                let path = entry.path();
                if path.is_dir() {
                    visit(&path, root, found)?;
                } else if path
                    .extension()
                    .is_some_and(|extension| extension == "rs" || extension == "json")
                {
                    let relative = path
                        .strip_prefix(root)
                        .map_err(std::io::Error::other)?
                        .to_string_lossy()
                        .replace('\\', "/");
                    // Fuzz entry points cannot participate in a live model run.
                    if relative != "fuzzing.rs" {
                        found.insert(format!("assistant/{relative}"));
                    }
                }
            }
            Ok(())
        }
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/assistant");
        let mut found = std::collections::BTreeSet::new();
        visit(&root, &root, &mut found)?;
        let covered = super::ASSISTANT_RUNTIME_ARTIFACTS
            .iter()
            .map(|(name, _)| (*name).to_owned())
            .collect::<std::collections::BTreeSet<_>>();
        assert!(
            found.is_subset(&covered),
            "Uncovered runtime artifacts: {:?}",
            found.difference(&covered).collect::<Vec<_>>()
        );
        Ok(())
    }
}
