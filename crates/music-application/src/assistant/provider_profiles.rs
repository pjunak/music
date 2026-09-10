//! Reviewed model settings, independent of HTTP and task prompts.
//! A discovered model is not evidence that it implements these settings.

use super::{
    DEEPSEEK_CHAT_ADAPTER, DEEPSEEK_RESPONSES_ADAPTER, GOOGLE_GEMINI_OPENAI_ADAPTER,
    GOOGLE_GEMINI_OPENAI_JSON_SCHEMA_ADAPTER, OPENAI_RESPONSES_ADAPTER, ThinkingMode,
};

pub const MODEL_PROFILE_CONTRACT: &str = "assistant-model-profiles/v1";
// September 14, 2026, 12:00 Beijing: the provider replaces the Pro alias.
pub const DEEPSEEK_PRO_TRANSITION: u64 = 1_789_358_400;

#[derive(Debug, Clone, Copy)]
pub struct ProviderModelProfile {
    pub id: &'static str,
    pub revision: &'static str,
    pub model_ids: &'static [&'static str],
    pub reasoning_modes: &'static [ThinkingMode],
    pub max_output_tokens: Option<u32>,
    pub documented: bool,
    pub notice: &'static str,
    pub source_url: &'static str,
}

use ThinkingMode::{Disabled, Enabled, High, Low, Max, Medium, ProviderDefault, Xhigh};

const UNKNOWN: ProviderModelProfile = ProviderModelProfile {
    id: "compatible-unverified",
    revision: "v1",
    model_ids: &[],
    reasoning_modes: &[ProviderDefault, Enabled, Disabled],
    max_output_tokens: None,
    documented: false,
    notice: "This model has no reviewed settings profile. Provider default sends no thinking override. Test the exact configuration before use.",
    source_url: "",
};

const OPENAI_UNKNOWN: ProviderModelProfile = ProviderModelProfile {
    id: "openai-unverified",
    reasoning_modes: &[ProviderDefault, Disabled, Low, Medium, High, Xhigh, Max],
    ..UNKNOWN
};

const GEMINI_UNKNOWN: ProviderModelProfile = ProviderModelProfile {
    id: "gemini-unverified",
    reasoning_modes: &[ProviderDefault, Disabled, Low, Medium, High],
    ..UNKNOWN
};

const DEEPSEEK_UNKNOWN: ProviderModelProfile = ProviderModelProfile {
    id: "deepseek-unverified",
    reasoning_modes: &[ProviderDefault, Disabled, Low, High, Max],
    ..UNKNOWN
};

const OPENAI_PROFILES: &[ProviderModelProfile] = &[
    ProviderModelProfile {
        id: "openai-astra",
        revision: "2026-09-10/v1",
        model_ids: &["gpt-6-astra"],
        reasoning_modes: &[ProviderDefault, Low, Medium, High, Xhigh, Max],
        max_output_tokens: Some(128_000),
        documented: true,
        notice: "Astra requires reasoning. Choose an effort level; thinking cannot be turned off.",
        source_url: "https://developers.openai.com/api/docs/models/gpt-6-astra",
    },
    ProviderModelProfile {
        id: "openai-5.6",
        revision: "2026-09-10/v1",
        model_ids: &["gpt-5.6", "gpt-5.6-sol", "gpt-5.6-terra", "gpt-5.6-luna"],
        reasoning_modes: &[ProviderDefault, Disabled, Low, Medium, High, Xhigh, Max],
        max_output_tokens: Some(128_000),
        documented: true,
        notice: "The response allowance includes reasoning and the final answer. Test each selected effort level.",
        source_url: "https://developers.openai.com/api/docs/models",
    },
];

const DEEPSEEK_PROFILES: &[ProviderModelProfile] = &[
    ProviderModelProfile {
        id: "deepseek-flash",
        revision: "v4.1-flash/2026-09-10",
        model_ids: &[
            "deepseek-flash",
            "deepseek-v4-flash",
            "deepseek-v4-flash-vision-exp",
        ],
        reasoning_modes: &[ProviderDefault, Disabled, Low, High, Max],
        max_output_tokens: Some(393_216),
        documented: true,
        notice: "DeepSeek defaults to thinking at high effort. The older Flash aliases now use V4.1 Flash; use deepseek-flash for new configurations.",
        source_url: "https://api-docs.deepseek.com/updates/",
    },
    ProviderModelProfile {
        id: "deepseek-pro",
        revision: "v4-pro-0813/before-2026-09-14",
        model_ids: &["deepseek-v4-pro"],
        reasoning_modes: &[ProviderDefault, Disabled, Low, High, Max],
        max_output_tokens: Some(393_216),
        documented: true,
        notice: "DeepSeek replaces this Pro alias with V4.1 Flash on September 14, 2026 at 04:00 UTC. Its saved conformance and quality evidence will become stale at that boundary.",
        source_url: "https://api-docs.deepseek.com/updates/",
    },
];

const GEMINI_PROFILES: &[ProviderModelProfile] = &[ProviderModelProfile {
    id: "gemini-required-thinking",
    revision: "2026-09-10/v1",
    model_ids: &[
        "gemini-2.5-pro",
        "gemini-3-pro-preview",
        "gemini-3-flash-preview",
        "gemini-3.1-pro-preview",
    ],
    reasoning_modes: &[ProviderDefault, Low, Medium, High],
    max_output_tokens: None,
    documented: true,
    notice: "This Gemini model requires thinking. Effort levels are translated by Google's compatibility API.",
    source_url: "https://ai.google.dev/gemini-api/docs/openai",
}];

#[must_use]
pub fn provider_model_profiles(adapter_id: &str) -> &'static [ProviderModelProfile] {
    match adapter_id {
        OPENAI_RESPONSES_ADAPTER => OPENAI_PROFILES,
        DEEPSEEK_CHAT_ADAPTER | DEEPSEEK_RESPONSES_ADAPTER => DEEPSEEK_PROFILES,
        GOOGLE_GEMINI_OPENAI_ADAPTER | GOOGLE_GEMINI_OPENAI_JSON_SCHEMA_ADAPTER => GEMINI_PROFILES,
        _ => &[],
    }
}

#[must_use]
pub fn default_model_profile(adapter_id: &str) -> ProviderModelProfile {
    match adapter_id {
        OPENAI_RESPONSES_ADAPTER => OPENAI_UNKNOWN,
        DEEPSEEK_CHAT_ADAPTER | DEEPSEEK_RESPONSES_ADAPTER => DEEPSEEK_UNKNOWN,
        GOOGLE_GEMINI_OPENAI_ADAPTER | GOOGLE_GEMINI_OPENAI_JSON_SCHEMA_ADAPTER => GEMINI_UNKNOWN,
        _ => UNKNOWN,
    }
}

#[must_use]
pub fn provider_model_profile(adapter_id: &str, model_id: &str) -> ProviderModelProfile {
    // Resource normalization is scoped to the explicitly selected Google adapter.
    let model_id = if matches!(
        adapter_id,
        GOOGLE_GEMINI_OPENAI_ADAPTER | GOOGLE_GEMINI_OPENAI_JSON_SCHEMA_ADAPTER
    ) {
        model_id.strip_prefix("models/").unwrap_or(model_id)
    } else {
        model_id
    };
    provider_model_profiles(adapter_id)
        .iter()
        .find(|profile| profile.model_ids.contains(&model_id))
        .copied()
        .unwrap_or_else(|| default_model_profile(adapter_id))
}

impl ProviderModelProfile {
    #[must_use]
    pub fn supports_reasoning(self, mode: ThinkingMode) -> bool {
        self.reasoning_modes.contains(&mode)
            || (mode == Enabled && self.reasoning_modes.contains(&High))
    }

    #[must_use]
    pub fn revision_at(self, unix_seconds: u64) -> &'static str {
        if self.id == "deepseek-pro" && unix_seconds >= DEEPSEEK_PRO_TRANSITION {
            "v4.1-flash/from-2026-09-14"
        } else {
            self.revision
        }
    }
}

pub fn validate_model_settings(
    adapter_id: &str,
    model_id: &str,
    mode: ThinkingMode,
    output_tokens: u32,
) -> Result<(), &'static str> {
    let profile = provider_model_profile(adapter_id, model_id);
    if !profile.supports_reasoning(mode) {
        return Err("unsupported_reasoning_mode");
    }
    if profile
        .max_output_tokens
        .is_some_and(|maximum| output_tokens > maximum)
    {
        return Err("unsupported_output_budget");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn model_settings_are_provider_scoped_and_preserve_legacy_on() {
        assert_eq!(
            validate_model_settings(OPENAI_RESPONSES_ADAPTER, "gpt-6-astra", Disabled, 2000),
            Err("unsupported_reasoning_mode")
        );
        assert!(
            validate_model_settings(OPENAI_RESPONSES_ADAPTER, "gpt-6-astra", Enabled, 2000).is_ok()
        );
        assert!(
            validate_model_settings(OPENAI_RESPONSES_ADAPTER, "gpt-5.6-luna", Disabled, 2000)
                .is_ok()
        );
        assert!(
            validate_model_settings(
                super::super::OPENAI_COMPATIBLE_ADAPTER,
                "gpt-6-astra",
                Disabled,
                2000
            )
            .is_ok()
        );
        assert_eq!(
            validate_model_settings(DEEPSEEK_CHAT_ADAPTER, "deepseek-flash", Medium, 2000),
            Err("unsupported_reasoning_mode")
        );
        assert_eq!(
            validate_model_settings(OPENAI_RESPONSES_ADAPTER, "gpt-6-astra", Low, 128_001),
            Err("unsupported_output_budget")
        );
        assert_eq!(
            provider_model_profile(GOOGLE_GEMINI_OPENAI_ADAPTER, "models/gemini-2.5-pro").id,
            "gemini-required-thinking"
        );
        assert!(!provider_model_profile(OPENAI_RESPONSES_ADAPTER, "future-model").documented);
    }

    #[test]
    fn known_remote_alias_change_invalidates_evidence_at_the_announced_time() {
        let pro = provider_model_profile(DEEPSEEK_CHAT_ADAPTER, "deepseek-v4-pro");
        assert_ne!(
            pro.revision_at(DEEPSEEK_PRO_TRANSITION - 1),
            pro.revision_at(DEEPSEEK_PRO_TRANSITION)
        );
        let flash = provider_model_profile(DEEPSEEK_CHAT_ADAPTER, "deepseek-flash");
        assert_eq!(
            flash.revision_at(DEEPSEEK_PRO_TRANSITION - 1),
            flash.revision_at(DEEPSEEK_PRO_TRANSITION)
        );
    }
}
