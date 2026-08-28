use sha2::{Digest, Sha256};

/// Versioned root for the server-owned model execution contract.
///
/// The digest deliberately includes the executable task definitions, validators,
/// evaluation suites, and the local evidence/vocabulary code on which model
/// inputs depend. A stored conformance or quality result therefore becomes stale
/// whenever one of those artifacts changes.
pub const ASSISTANT_RUNTIME_CONTRACT_VERSION: &str = "assistant-runtime-contract/v1";

const ASSISTANT_RUNTIME_ARTIFACTS: &[(&str, &str)] = &[
    ("assistant/mod.rs", include_str!("mod.rs")),
    (
        "assistant/local_analysis.rs",
        include_str!("local_analysis.rs"),
    ),
    ("assistant/model_eq.rs", include_str!("model_eq.rs")),
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
    ("assistant/planner.rs", include_str!("planner.rs")),
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
    let mut digest = Sha256::new();
    update_artifact(
        &mut digest,
        "contract-version",
        ASSISTANT_RUNTIME_CONTRACT_VERSION,
    );
    for (name, contents) in ASSISTANT_RUNTIME_ARTIFACTS {
        update_artifact(&mut digest, name, contents);
    }
    format!("{:x}", digest.finalize())
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
}
