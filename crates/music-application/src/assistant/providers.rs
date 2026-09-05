use std::collections::{BTreeMap, BTreeSet};
use std::fmt::{self, Debug, Display, Formatter};
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use rand::TryRngCore;
use rand::rngs::OsRng;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use uuid::Uuid;
use zeroize::Zeroizing;

use super::{AssistantDependencyError, AssistantFuture};

pub const OPENAI_COMPATIBLE_ADAPTER: &str = "openai-compatible/v1";
pub const OPENAI_COMPATIBLE_JSON_SCHEMA_ADAPTER: &str = "openai-compatible-json-schema/v1";
pub const OPENAI_RESPONSES_ADAPTER: &str = "openai-responses/v1";
pub const GOOGLE_GEMINI_OPENAI_ADAPTER: &str = "google-gemini-openai/v1";
pub const GOOGLE_GEMINI_OPENAI_JSON_SCHEMA_ADAPTER: &str = "google-gemini-openai-json-schema/v1";
pub const STRUCTURED_TEXT_CAPABILITY: &str = "structured-text/v1";
pub const STRICT_JSON_SCHEMA_CAPABILITY: &str = "strict-json-schema/v1";
pub const AUDIO_INPUT_CAPABILITY: &str = "audio-input/v1";
pub const PROVIDER_CONFORMANCE_CONTRACT: &str = "assistant-provider-conformance/v3";
const PROVIDER_CONFORMANCE_CHALLENGE_CONTRACT: &str = "assistant-provider-conformance-challenge/v4";
pub const STRUCTURED_HARNESS_CONTRACT: &str = "assistant-structured-harness/v3";
const CONFORMANCE_CHALLENGE_BYTES: usize = 24;

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub struct ProviderCapabilityDefinition {
    pub id: &'static str,
    pub label: &'static str,
    pub description: &'static str,
}

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub struct ProviderAdapterDefinition {
    pub id: &'static str,
    pub label: &'static str,
    pub description: &'static str,
    pub capability_ids: &'static [&'static str],
}

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub struct ModelRoleDefinition {
    pub id: &'static str,
    pub label: &'static str,
    pub description: &'static str,
    pub required_capability_ids: &'static [&'static str],
    pub configuration_available: bool,
    pub runtime_contract: &'static str,
}

pub const PROVIDER_CAPABILITIES: &[ProviderCapabilityDefinition] = &[
    ProviderCapabilityDefinition {
        id: STRUCTURED_TEXT_CAPABILITY,
        label: "Structured text",
        description: "Sends text instructions and receives a validated machine-readable result.",
    },
    ProviderCapabilityDefinition {
        id: STRICT_JSON_SCHEMA_CAPABILITY,
        label: "Strict JSON Schema",
        description: "Constrains model responses with the task's exact JSON Schema at the API.",
    },
    ProviderCapabilityDefinition {
        id: AUDIO_INPUT_CAPABILITY,
        label: "Audio input",
        description: "Accepts bounded audio content through a dedicated provider adapter.",
    },
];

pub const PROVIDER_ADAPTERS: &[ProviderAdapterDefinition] = &[
    ProviderAdapterDefinition {
        id: OPENAI_RESPONSES_ADAPTER,
        label: "OpenAI API (Responses)",
        description: "OpenAI's native Responses API with reasoning controls and strict JSON Schema output.",
        capability_ids: &[STRUCTURED_TEXT_CAPABILITY, STRICT_JSON_SCHEMA_CAPABILITY],
    },
    ProviderAdapterDefinition {
        id: OPENAI_COMPATIBLE_ADAPTER,
        label: "Other OpenAI-compatible API",
        description: "Maximum third-party compatibility using JSON-object response mode plus strict local validation.",
        capability_ids: &[STRUCTURED_TEXT_CAPABILITY],
    },
    ProviderAdapterDefinition {
        id: OPENAI_COMPATIBLE_JSON_SCHEMA_ADAPTER,
        label: "OpenAI-compatible strict JSON Schema",
        description: "For compatible services that support response_format type json_schema. Use the standard adapter when the provider supports only json_object.",
        capability_ids: &[STRUCTURED_TEXT_CAPABILITY, STRICT_JSON_SCHEMA_CAPABILITY],
    },
    ProviderAdapterDefinition {
        id: GOOGLE_GEMINI_OPENAI_ADAPTER,
        label: "Google Gemini API",
        description: "Gemini's OpenAI-compatible API with canonical model IDs, provider-specific thinking controls, and native JSON Schema output.",
        capability_ids: &[STRUCTURED_TEXT_CAPABILITY, STRICT_JSON_SCHEMA_CAPABILITY],
    },
    ProviderAdapterDefinition {
        id: GOOGLE_GEMINI_OPENAI_JSON_SCHEMA_ADAPTER,
        label: "Google Gemini API with strict JSON Schema",
        description: "Gemini's OpenAI-compatible API with canonical model IDs, provider-specific thinking controls, and native JSON Schema output.",
        capability_ids: &[STRUCTURED_TEXT_CAPABILITY, STRICT_JSON_SCHEMA_CAPABILITY],
    },
];

pub const MODEL_ROLES: &[ModelRoleDefinition] = &[
    ModelRoleDefinition {
        id: "music_tagger",
        label: "Mood tagging",
        description: "Suggest reviewable setting, period, scene, and mood database tags from approved track evidence.",
        required_capability_ids: &[STRUCTURED_TEXT_CAPABILITY],
        configuration_available: true,
        runtime_contract: "assistant-music-tagger-input/v19+output/v3+local-context/v2",
    },
    ModelRoleDefinition {
        id: "playlist_planner",
        label: "Playlist planning",
        description: "Interpret playlist requests and improve a reviewable local draft.",
        required_capability_ids: &[STRUCTURED_TEXT_CAPABILITY],
        configuration_available: true,
        runtime_contract: "assistant-playlist-planner-input/v3+output/v1+closed-ids/v1",
    },
    ModelRoleDefinition {
        id: "tag_cleanup",
        label: "Mood-tag cleanup",
        description: "Suggests review-only consistent names and merges from the mood-tag catalog.",
        required_capability_ids: &[STRUCTURED_TEXT_CAPABILITY],
        configuration_available: true,
        runtime_contract: "assistant-model-tag-cleanup-input/v3+output/v2+incidental-text-bounds/v1",
    },
    ModelRoleDefinition {
        id: "library_cleanup",
        label: "Library cleanup",
        description: "Reserved for a future model pass over the existing review-first cleanup.",
        required_capability_ids: &[STRUCTURED_TEXT_CAPABILITY],
        configuration_available: false,
        runtime_contract: "reserved-library-cleanup/v1",
    },
    ModelRoleDefinition {
        id: "eq_assistant",
        label: "EQ assistance",
        description: "Creates bounded graphic-EQ drafts for explicit Authoring review.",
        required_capability_ids: &[STRUCTURED_TEXT_CAPABILITY],
        configuration_available: true,
        runtime_contract: "assistant-eq-draft-input/v2+output/v1+incidental-text-bounds/v1",
    },
    ModelRoleDefinition {
        id: "audio_analyzer",
        label: "Specialized audio analysis",
        description: "Reserved for a future audio-capable adapter with separate consent.",
        required_capability_ids: &[AUDIO_INPUT_CAPABILITY],
        configuration_available: false,
        runtime_contract: "reserved-audio-analyzer/v1",
    },
];

#[must_use]
pub fn provider_adapter(adapter_id: &str) -> Option<&'static ProviderAdapterDefinition> {
    PROVIDER_ADAPTERS
        .iter()
        .find(|definition| definition.id == adapter_id)
}

#[must_use]
pub fn model_role(role_id: &str) -> Option<&'static ModelRoleDefinition> {
    MODEL_ROLES
        .iter()
        .find(|definition| definition.id == role_id)
}

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum ProviderVerificationStatus {
    Never,
    Verified,
    Failed,
}

impl ProviderVerificationStatus {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Never => "never",
            Self::Verified => "verified",
            Self::Failed => "failed",
        }
    }

    #[must_use]
    pub fn parse(value: &str) -> Self {
        match value {
            "verified" => Self::Verified,
            "failed" => Self::Failed,
            _ => Self::Never,
        }
    }
}

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum ModelConformanceStatus {
    Never,
    Passed,
    Failed,
}

impl ModelConformanceStatus {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Never => "never",
            Self::Passed => "passed",
            Self::Failed => "failed",
        }
    }

    #[must_use]
    pub fn parse(value: &str) -> Self {
        match value {
            "passed" => Self::Passed,
            "failed" => Self::Failed,
            _ => Self::Never,
        }
    }
}

#[derive(Debug, Clone, Copy, Default, Eq, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ThinkingMode {
    #[default]
    ProviderDefault,
    Enabled,
    Disabled,
}

impl ThinkingMode {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::ProviderDefault => "provider_default",
            Self::Enabled => "enabled",
            Self::Disabled => "disabled",
        }
    }

    #[must_use]
    pub fn parse(value: &str) -> Self {
        match value {
            "enabled" => Self::Enabled,
            "disabled" => Self::Disabled,
            _ => Self::ProviderDefault,
        }
    }
}

#[derive(Clone, Eq, PartialEq)]
pub struct ProviderConnectionRecord {
    pub id: String,
    pub name: String,
    pub adapter_id: String,
    pub base_url: String,
    pub encrypted_api_key: String,
    pub api_key_nonce: String,
    pub api_key_hint: String,
    pub allow_private_network: bool,
    pub verification_status: String,
    pub verification_error_code: Option<String>,
    pub verified_models: Vec<String>,
    pub verified_capability_ids: Vec<String>,
    pub last_verified_at_unix_seconds: Option<i64>,
    pub created_at_unix_seconds: i64,
    pub updated_at_unix_seconds: i64,
}

impl Debug for ProviderConnectionRecord {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ProviderConnectionRecord")
            .field("id", &self.id)
            .field("name", &self.name)
            .field("adapter_id", &self.adapter_id)
            .field("base_url", &self.base_url)
            .field("encrypted_api_key", &"[REDACTED]")
            .field("api_key_nonce", &"[REDACTED]")
            .field("api_key_hint", &self.api_key_hint)
            .field("allow_private_network", &self.allow_private_network)
            .field("verification_status", &self.verification_status)
            .finish_non_exhaustive()
    }
}

impl ProviderConnectionRecord {
    #[must_use]
    pub fn credential_saved(&self) -> bool {
        !self.encrypted_api_key.is_empty() && !self.api_key_nonce.is_empty()
    }

    #[must_use]
    pub fn fingerprint(&self) -> String {
        let capabilities = serde_json::to_string(&self.verified_capability_ids)
            .unwrap_or_else(|_| "[]".to_owned());
        let value = [
            self.id.as_str(),
            self.adapter_id.as_str(),
            self.base_url.as_str(),
            self.encrypted_api_key.as_str(),
            self.api_key_nonce.as_str(),
            capabilities.as_str(),
            if self.allow_private_network { "1" } else { "0" },
        ]
        .join("\0");
        format!("{:x}", Sha256::digest(value.as_bytes()))
    }
}

#[derive(Debug, Clone, Eq, PartialEq)]
pub struct ProviderConnectionView {
    pub id: String,
    pub name: String,
    pub adapter_id: String,
    pub base_url: String,
    pub credential_saved: bool,
    pub key_hint: Option<String>,
    pub allow_private_network: bool,
    pub verification_status: ProviderVerificationStatus,
    pub verification_error_code: Option<String>,
    pub verified_models: Vec<String>,
    pub verified_capability_ids: Vec<String>,
    pub last_verified_at_unix_seconds: Option<i64>,
    pub created_at_unix_seconds: i64,
    pub updated_at_unix_seconds: i64,
}

impl From<&ProviderConnectionRecord> for ProviderConnectionView {
    fn from(value: &ProviderConnectionRecord) -> Self {
        let saved = value.credential_saved();
        Self {
            id: value.id.clone(),
            name: value.name.clone(),
            adapter_id: value.adapter_id.clone(),
            base_url: value.base_url.clone(),
            credential_saved: saved,
            key_hint: saved.then(|| value.api_key_hint.clone()),
            allow_private_network: value.allow_private_network,
            verification_status: ProviderVerificationStatus::parse(&value.verification_status),
            verification_error_code: value.verification_error_code.clone(),
            verified_models: bounded_unique_strings(&value.verified_models, 200),
            verified_capability_ids: verified_capabilities(value),
            last_verified_at_unix_seconds: value.last_verified_at_unix_seconds,
            created_at_unix_seconds: value.created_at_unix_seconds,
            updated_at_unix_seconds: value.updated_at_unix_seconds,
        }
    }
}

#[derive(Debug, Clone, Eq, PartialEq)]
pub struct ModelRoleRecord {
    pub role_id: String,
    pub connection_id: String,
    pub model_id: String,
    pub enabled: bool,
    pub timeout_seconds: u16,
    pub max_output_tokens: u32,
    pub thinking_mode: String,
    pub conformance_status: String,
    pub conformance_error_code: Option<String>,
    pub conformance_fingerprint: Option<String>,
    pub last_conformance_at_unix_seconds: Option<i64>,
    pub updated_at_unix_seconds: i64,
}

impl ModelRoleRecord {
    #[must_use]
    pub fn configuration_fingerprint(&self) -> String {
        let timeout_seconds = self.timeout_seconds.to_string();
        let max_output_tokens = self.max_output_tokens.to_string();
        let value = [
            self.role_id.as_str(),
            self.connection_id.as_str(),
            self.model_id.as_str(),
            if self.enabled { "1" } else { "0" },
            timeout_seconds.as_str(),
            max_output_tokens.as_str(),
            ThinkingMode::parse(&self.thinking_mode).as_str(),
        ]
        .join("\0");
        format!("{:x}", Sha256::digest(value.as_bytes()))
    }
}

#[derive(Debug, Clone, Eq, PartialEq)]
pub struct ModelRoleView {
    pub role_id: String,
    pub label: String,
    pub description: String,
    pub required_capability_ids: Vec<String>,
    pub configuration_available: bool,
    pub connection_id: Option<String>,
    pub connection_name: Option<String>,
    pub model_id: String,
    pub enabled: bool,
    pub effective_enabled: bool,
    pub timeout_seconds: u16,
    pub max_output_tokens: u32,
    pub thinking_mode: ThinkingMode,
    pub verification_status: Option<ProviderVerificationStatus>,
    pub conformance_status: ModelConformanceStatus,
    pub conformance_error_code: Option<String>,
    pub last_conformance_at_unix_seconds: Option<i64>,
    pub updated_at_unix_seconds: Option<i64>,
}

#[derive(Debug)]
pub struct ProviderConnectionCreate {
    pub name: String,
    pub adapter_id: String,
    pub base_url: String,
    pub api_key: ProviderSecret,
    pub allow_private_network: bool,
}

#[derive(Debug, Default)]
pub struct ProviderConnectionPatch {
    pub name: Option<String>,
    pub adapter_id: Option<String>,
    pub base_url: Option<String>,
    pub api_key: Option<ProviderSecret>,
    pub allow_private_network: Option<bool>,
}

#[derive(Debug, Clone, Eq, PartialEq)]
pub struct ModelRoleUpdate {
    pub connection_id: String,
    pub model_id: String,
    pub enabled: bool,
    pub timeout_seconds: u16,
    pub max_output_tokens: u32,
    pub thinking_mode: ThinkingMode,
}

#[derive(Clone, Eq, PartialEq)]
pub struct EncryptedProviderCredential {
    pub ciphertext: String,
    pub nonce: String,
    pub hint: String,
}

impl Debug for EncryptedProviderCredential {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("EncryptedProviderCredential")
            .field("ciphertext", &"[REDACTED]")
            .field("nonce", &"[REDACTED]")
            .field("hint", &self.hint)
            .finish()
    }
}

pub struct ProviderSecret {
    value: Zeroizing<String>,
    _lifetime_guard: Option<Arc<dyn Debug + Send + Sync>>,
}

impl ProviderSecret {
    #[must_use]
    pub fn new(value: impl Into<String>) -> Self {
        Self {
            value: Zeroizing::new(value.into()),
            _lifetime_guard: None,
        }
    }

    #[must_use]
    pub fn with_lifetime_guard(mut self, guard: Arc<dyn Debug + Send + Sync>) -> Self {
        self._lifetime_guard = Some(guard);
        self
    }

    #[must_use]
    pub fn expose_secret(&self) -> &str {
        self.value.as_str()
    }
}

impl Debug for ProviderSecret {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        formatter.write_str("ProviderSecret([REDACTED])")
    }
}

#[derive(Debug, Clone, Eq, PartialEq)]
pub struct ProviderCredentialError {
    pub code: String,
}

impl Display for ProviderCredentialError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.code)
    }
}

impl std::error::Error for ProviderCredentialError {}

pub trait ProviderCredentialCipher: Debug + Send + Sync {
    fn encrypt(
        &self,
        connection_id: &str,
        api_key: &str,
    ) -> Result<EncryptedProviderCredential, ProviderCredentialError>;
    fn decrypt(
        &self,
        connection_id: &str,
        ciphertext: &str,
        nonce: &str,
    ) -> Result<ProviderSecret, ProviderCredentialError>;
}

pub type ProviderCredentialFuture<'a> = Pin<
    Box<
        dyn Future<Output = Result<Arc<dyn ProviderCredentialCipher>, ProviderCredentialError>>
            + Send
            + 'a,
    >,
>;

pub trait ProviderCredentialSource: Debug + Send + Sync {
    fn current_cipher(&self) -> ProviderCredentialFuture<'_>;
}

#[derive(Debug, Clone, Eq, PartialEq)]
pub struct ProviderPolicyError {
    pub code: String,
    pub message: String,
}

impl Display for ProviderPolicyError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for ProviderPolicyError {}

pub trait ProviderConnectionPolicy: Debug + Send + Sync {
    fn normalize_base_url(
        &self,
        adapter_id: &str,
        raw: &str,
        allow_private_network: bool,
    ) -> Result<String, ProviderPolicyError>;
}

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum ProviderMutationOutcome {
    Applied,
    NotFound,
    DuplicateName,
    ConnectionInUse,
    ConnectionModelJobActive,
    RoleModelJobActive,
    Changed,
}

#[derive(Debug, Clone, Eq, PartialEq)]
pub enum ProviderConnectionPreparation {
    Ready(Box<ProviderConnectionRecord>),
    NotFound,
    ModelJobActive,
}

#[derive(Debug)]
pub struct ProviderVerificationTarget {
    pub connection_id: String,
    pub adapter_id: String,
    pub base_url: String,
    pub api_key: ProviderSecret,
    pub allow_private_network: bool,
    pub fingerprint: String,
}

#[derive(Debug, Clone, Eq, PartialEq)]
pub struct ProviderVerificationResult {
    pub verified: bool,
    pub error_code: Option<String>,
    pub models: Vec<String>,
    pub capability_ids: Vec<String>,
}

#[derive(Debug, Clone, Eq, PartialEq)]
pub struct ProviderVerificationWrite {
    pub connection_id: String,
    pub expected_fingerprint: String,
    pub verified: bool,
    pub error_code: Option<String>,
    pub models: Vec<String>,
    pub capability_ids: Vec<String>,
}

#[derive(Debug, Clone, Eq, PartialEq)]
pub enum ProviderVerificationWriteOutcome {
    Applied(Box<ProviderConnectionRecord>),
    NotFound,
    ModelJobActive,
    Changed,
}

#[derive(Debug, Clone, Eq, PartialEq)]
pub struct ProviderVerificationView {
    pub connection: ProviderConnectionView,
    pub verified: bool,
    pub error_code: Option<String>,
    pub models: Vec<String>,
}

#[derive(Debug)]
pub struct ProviderExecutionTarget {
    pub adapter_id: String,
    pub base_url: String,
    pub api_key: ProviderSecret,
    pub allow_private_network: bool,
    pub model_id: String,
    pub timeout_seconds: u16,
    pub max_output_tokens: u32,
    pub thinking_mode: ThinkingMode,
}

#[derive(Debug)]
pub struct ResolvedRoleExecution {
    pub role_id: String,
    pub execution: ProviderExecutionTarget,
    pub fingerprint: String,
    pub role_configuration_fingerprint: String,
    pub connection_fingerprint: String,
    pub connection_name: String,
}

/// Current local identity for reviewing stored proposals; grants no provider access.
#[derive(Debug, Clone, Eq, PartialEq)]
pub struct ModelRoleReviewIdentity {
    pub runtime_fingerprint: String,
    pub configuration_fingerprint: String,
    pub connection_fingerprint: String,
}

#[derive(Debug, Clone, Eq, PartialEq, Serialize)]
pub struct StructuredModelRequest {
    pub system_prompt: String,
    pub user_prompt: String,
    pub max_output_tokens: u32,
    pub output_schema_name: Option<String>,
    pub output_schema: Option<Value>,
}

#[derive(Debug, Clone, Eq, PartialEq)]
pub struct StructuredModelResult {
    pub outcome: ProviderAttemptOutcome,
    pub succeeded: bool,
    pub error_code: Option<String>,
    pub payload: Option<Value>,
    pub provider_model_id: Option<String>,
    pub finish_reason: Option<String>,
    pub input_tokens: Option<u64>,
    pub output_tokens: Option<u64>,
}

/// Transport facts, independent of schema validation and provider billing.
#[derive(Debug, Clone, Copy, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderAttemptOutcome {
    PreflightRejected,
    NotSent,
    ResponseReceived,
    Uncertain,
}

#[derive(Debug, Clone, Eq, PartialEq)]
pub struct ProviderConformanceResult {
    pub passed: bool,
    pub error_code: Option<String>,
    pub provider_model_id: Option<String>,
    pub finish_reason: Option<String>,
    pub input_tokens: Option<u64>,
    pub output_tokens: Option<u64>,
}

#[derive(Debug)]
pub struct ProviderConformanceTarget {
    pub role_id: String,
    pub execution: ProviderExecutionTarget,
    pub challenge: String,
    pub runtime_fingerprint: String,
    pub role_configuration_fingerprint: String,
    pub connection_fingerprint: String,
}

impl ProviderConformanceTarget {
    #[must_use]
    pub fn request(&self) -> StructuredModelRequest {
        StructuredModelRequest {
            system_prompt: "This is a connection conformance test. Return only one JSON object with exactly these keys: contract, challenge, checks, accepted. Copy the supplied contract and challenge exactly, copy checks in the supplied order without duplicates, and set accepted to true. Do not use a Markdown code fence or add any other text.".to_owned(),
            user_prompt: json!({
                "contract": PROVIDER_CONFORMANCE_CHALLENGE_CONTRACT,
                "challenge": self.challenge,
                "checks": ["schema", "identity"],
            })
            .to_string(),
            max_output_tokens: 256,
            output_schema_name: Some("assistant-provider-conformance".to_owned()),
            output_schema: Some(json!({
                "type": "object",
                "additionalProperties": false,
                "required": ["contract", "challenge", "checks", "accepted"],
                "properties": {
                    "contract": {
                        "type": "string",
                        "const": PROVIDER_CONFORMANCE_CHALLENGE_CONTRACT,
                    },
                    "challenge": {"type": "string", "const": self.challenge},
                    "checks": {
                        "type": "array",
                        "minItems": 2,
                        "maxItems": 2,
                        "uniqueItems": true,
                        "items": {"type": "string", "enum": ["schema", "identity"]},
                    },
                    "accepted": {"type": "boolean", "const": true},
                },
            })),
        }
    }

    #[must_use]
    pub fn evaluate(&self, result: StructuredModelResult) -> ProviderConformanceResult {
        let passed = result.succeeded
            && result.payload.as_ref()
                == Some(&json!({
                    "contract": PROVIDER_CONFORMANCE_CHALLENGE_CONTRACT,
                    "challenge": self.challenge,
                    "checks": ["schema", "identity"],
                    "accepted": true,
                }));
        ProviderConformanceResult {
            passed,
            error_code: if passed {
                None
            } else if result.succeeded {
                Some("conformance_mismatch".to_owned())
            } else {
                result.error_code
            },
            provider_model_id: result.provider_model_id,
            finish_reason: result.finish_reason,
            input_tokens: result.input_tokens,
            output_tokens: result.output_tokens,
        }
    }
}

#[derive(Debug, Clone, Eq, PartialEq)]
pub struct ProviderRoleRuntimeRecord {
    pub role: ModelRoleRecord,
    pub connection: ProviderConnectionRecord,
}

#[derive(Debug, Clone, Eq, PartialEq)]
pub enum ProviderRolePreparation {
    Ready(Box<ProviderRoleRuntimeRecord>),
    NotConfigured,
    ConnectionNotFound,
    ModelJobActive,
}

#[derive(Debug, Clone, Eq, PartialEq)]
pub struct ProviderConformanceWrite {
    pub role_id: String,
    pub expected_role_configuration_fingerprint: String,
    pub expected_connection_fingerprint: String,
    pub runtime_fingerprint: String,
    pub passed: bool,
    pub error_code: Option<String>,
}

#[derive(Debug, Clone, Eq, PartialEq)]
pub enum ProviderConformanceWriteOutcome {
    Applied(Box<ProviderRoleRuntimeRecord>),
    RoleChanged,
    ConnectionChanged,
    ModelJobActive,
}

#[derive(Debug, Clone, Eq, PartialEq)]
pub struct ProviderConformanceView {
    pub role: ModelRoleView,
    pub passed: bool,
    pub error_code: Option<String>,
    pub provider_model_id: Option<String>,
    pub finish_reason: Option<String>,
    pub input_tokens: Option<u64>,
    pub output_tokens: Option<u64>,
}

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum ProviderCredentialResetOutcome {
    Applied { deleted_credentials: u64 },
    ModelJobActive,
}

pub trait ProviderRepository: Debug + Send + Sync {
    fn saved_provider_credentials_exist(&self) -> AssistantFuture<'_, bool>;
    fn reset_provider_credentials(&self) -> AssistantFuture<'_, ProviderCredentialResetOutcome>;
    fn provider_connections(&self) -> AssistantFuture<'_, Vec<ProviderConnectionRecord>>;
    fn provider_connection<'a>(
        &'a self,
        connection_id: &'a str,
    ) -> AssistantFuture<'a, Option<ProviderConnectionRecord>>;
    fn prepare_provider_connection<'a>(
        &'a self,
        connection_id: &'a str,
    ) -> AssistantFuture<'a, ProviderConnectionPreparation>;
    fn finish_provider_verification<'a>(
        &'a self,
        verification: &'a ProviderVerificationWrite,
    ) -> AssistantFuture<'a, ProviderVerificationWriteOutcome>;
    fn create_provider_connection<'a>(
        &'a self,
        connection: &'a ProviderConnectionRecord,
    ) -> AssistantFuture<'a, ProviderMutationOutcome>;
    fn replace_provider_connection<'a>(
        &'a self,
        expected_fingerprint: &'a str,
        connection: &'a ProviderConnectionRecord,
        reset_dependents: bool,
    ) -> AssistantFuture<'a, ProviderMutationOutcome>;
    fn delete_provider_connection<'a>(
        &'a self,
        connection_id: &'a str,
    ) -> AssistantFuture<'a, ProviderMutationOutcome>;
    fn clear_provider_credential<'a>(
        &'a self,
        connection_id: &'a str,
    ) -> AssistantFuture<'a, ProviderMutationOutcome>;
    fn model_roles(&self) -> AssistantFuture<'_, Vec<ModelRoleRecord>>;
    fn prepare_model_role<'a>(
        &'a self,
        role_id: &'a str,
    ) -> AssistantFuture<'a, ProviderRolePreparation>;
    fn finish_role_conformance<'a>(
        &'a self,
        conformance: &'a ProviderConformanceWrite,
    ) -> AssistantFuture<'a, ProviderConformanceWriteOutcome>;
    fn save_model_role<'a>(
        &'a self,
        expected_connection_fingerprint: &'a str,
        role: &'a ModelRoleRecord,
        reset_evaluations: bool,
    ) -> AssistantFuture<'a, ProviderMutationOutcome>;
    fn delete_model_role<'a>(
        &'a self,
        role_id: &'a str,
    ) -> AssistantFuture<'a, ProviderMutationOutcome>;
}

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum ProviderServiceErrorKind {
    Invalid,
    NotFound,
    Conflict,
    Unavailable,
    Dependency,
}

#[derive(Debug)]
pub struct ProviderServiceError {
    kind: ProviderServiceErrorKind,
    code: &'static str,
    message: &'static str,
    source: Option<AssistantDependencyError>,
}

impl ProviderServiceError {
    #[must_use]
    pub const fn kind(&self) -> ProviderServiceErrorKind {
        self.kind
    }

    #[must_use]
    pub const fn code(&self) -> &'static str {
        self.code
    }

    #[must_use]
    pub const fn message(&self) -> &'static str {
        self.message
    }

    pub(super) fn public(
        kind: ProviderServiceErrorKind,
        code: &'static str,
        message: &'static str,
    ) -> Self {
        Self {
            kind,
            code,
            message,
            source: None,
        }
    }

    fn dependency(source: AssistantDependencyError) -> Self {
        Self {
            kind: ProviderServiceErrorKind::Dependency,
            code: "provider_storage_failed",
            message: "Provider storage failed.",
            source: Some(source),
        }
    }
}

impl Display for ProviderServiceError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.message)
    }
}

impl std::error::Error for ProviderServiceError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        self.source
            .as_deref()
            .map(|source| source as &(dyn std::error::Error + 'static))
    }
}

#[derive(Debug)]
pub struct ProviderService {
    repository: Arc<dyn ProviderRepository>,
    credentials: Arc<dyn ProviderCredentialSource>,
    policy: Arc<dyn ProviderConnectionPolicy>,
    executable_contract_digest: String,
    role_contract_digests: BTreeMap<String, String>,
}

impl ProviderService {
    #[must_use]
    pub fn new(
        repository: Arc<dyn ProviderRepository>,
        credentials: Arc<dyn ProviderCredentialSource>,
        policy: Arc<dyn ProviderConnectionPolicy>,
        executable_contract_digest: String,
    ) -> Self {
        Self {
            repository,
            credentials,
            policy,
            executable_contract_digest,
            role_contract_digests: BTreeMap::new(),
        }
    }

    /// Composition may supply reviewed role closures. Unlisted roles retain the
    /// complete executable digest rather than silently omitting code coverage.
    #[must_use]
    pub fn with_role_contract_digests(mut self, digests: BTreeMap<String, String>) -> Self {
        self.role_contract_digests = digests;
        self
    }

    pub async fn list_connections(
        &self,
    ) -> Result<Vec<ProviderConnectionView>, ProviderServiceError> {
        let mut connections = self
            .repository
            .provider_connections()
            .await
            .map_err(ProviderServiceError::dependency)?;
        connections.sort_by(|left, right| {
            left.name
                .to_lowercase()
                .cmp(&right.name.to_lowercase())
                .then_with(|| left.id.cmp(&right.id))
        });
        Ok(connections
            .iter()
            .map(ProviderConnectionView::from)
            .collect())
    }

    pub async fn create_connection(
        &self,
        mut request: ProviderConnectionCreate,
    ) -> Result<ProviderConnectionView, ProviderServiceError> {
        normalize_required(&mut request.name, 128)?;
        normalize_required(&mut request.adapter_id, 64)?;
        normalize_required(&mut request.base_url, 2_048)?;
        validate_api_key(request.api_key.expose_secret())?;
        require_adapter(&request.adapter_id)?;
        let base_url = self
            .policy
            .normalize_base_url(
                &request.adapter_id,
                &request.base_url,
                request.allow_private_network,
            )
            .map_err(map_policy_error)?;
        let id = Uuid::new_v4().simple().to_string();
        let cipher = self
            .credentials
            .current_cipher()
            .await
            .map_err(map_credential_error)?;
        let encrypted = cipher
            .encrypt(&id, request.api_key.expose_secret())
            .map_err(map_credential_error)?;
        let record = ProviderConnectionRecord {
            id: id.clone(),
            name: request.name,
            adapter_id: request.adapter_id,
            base_url,
            encrypted_api_key: encrypted.ciphertext,
            api_key_nonce: encrypted.nonce,
            api_key_hint: encrypted.hint,
            allow_private_network: request.allow_private_network,
            verification_status: "never".to_owned(),
            verification_error_code: None,
            verified_models: Vec::new(),
            verified_capability_ids: Vec::new(),
            last_verified_at_unix_seconds: None,
            created_at_unix_seconds: 0,
            updated_at_unix_seconds: 0,
        };
        let result = self
            .repository
            .create_provider_connection(&record)
            .await
            .map_err(ProviderServiceError::dependency)?;
        drop(cipher);
        match result {
            ProviderMutationOutcome::Applied => self.connection_view(&id).await,
            ProviderMutationOutcome::DuplicateName => Err(duplicate_name()),
            _ => Err(unexpected_mutation()),
        }
    }

    pub async fn update_connection(
        &self,
        connection_id: &str,
        mut request: ProviderConnectionPatch,
    ) -> Result<ProviderConnectionView, ProviderServiceError> {
        validate_identifier(connection_id, 32)?;
        if let Some(value) = request.name.as_mut() {
            normalize_required(value, 128)?;
        }
        if let Some(value) = request.adapter_id.as_mut() {
            normalize_required(value, 64)?;
        }
        if let Some(value) = request.base_url.as_mut() {
            normalize_required(value, 2_048)?;
        }
        if let Some(value) = request.api_key.as_ref() {
            validate_api_key(value.expose_secret())?;
        }
        let current = self.connection_record(connection_id).await?;
        if request.api_key.is_some() && current.credential_saved() {
            return Err(ProviderServiceError::public(
                ProviderServiceErrorKind::Conflict,
                "credential_already_saved",
                "Delete the saved API key before adding a different one.",
            ));
        }
        let name = request.name.unwrap_or_else(|| current.name.clone());
        let adapter_id = request
            .adapter_id
            .unwrap_or_else(|| current.adapter_id.clone());
        let allow_private_network = request
            .allow_private_network
            .unwrap_or(current.allow_private_network);
        require_adapter(&adapter_id)?;
        let raw_url = request.base_url.unwrap_or_else(|| current.base_url.clone());
        let base_url = self
            .policy
            .normalize_base_url(&adapter_id, &raw_url, allow_private_network)
            .map_err(map_policy_error)?;
        let mut replacement = current.clone();
        let mut credential_lease: Option<Arc<dyn ProviderCredentialCipher>> = None;
        replacement.name = name;
        replacement.adapter_id = adapter_id;
        replacement.base_url = base_url;
        replacement.allow_private_network = allow_private_network;
        if let Some(api_key) = request.api_key {
            let cipher = self
                .credentials
                .current_cipher()
                .await
                .map_err(map_credential_error)?;
            let encrypted = cipher
                .encrypt(connection_id, api_key.expose_secret())
                .map_err(map_credential_error)?;
            replacement.encrypted_api_key = encrypted.ciphertext;
            replacement.api_key_nonce = encrypted.nonce;
            replacement.api_key_hint = encrypted.hint;
            credential_lease = Some(cipher);
        }
        let reset = replacement.adapter_id != current.adapter_id
            || replacement.base_url != current.base_url
            || replacement.allow_private_network != current.allow_private_network
            || replacement.encrypted_api_key != current.encrypted_api_key
            || replacement.api_key_nonce != current.api_key_nonce;
        if reset {
            reset_connection_verification(&mut replacement);
        }
        let result = self
            .repository
            .replace_provider_connection(&current.fingerprint(), &replacement, reset)
            .await
            .map_err(ProviderServiceError::dependency)?;
        drop(credential_lease);
        match result {
            ProviderMutationOutcome::Applied => self.connection_view(connection_id).await,
            ProviderMutationOutcome::NotFound => Err(connection_not_found()),
            ProviderMutationOutcome::DuplicateName => Err(duplicate_name()),
            ProviderMutationOutcome::ConnectionModelJobActive => Err(connection_job_active()),
            ProviderMutationOutcome::Changed => Err(ProviderServiceError::public(
                ProviderServiceErrorKind::Conflict,
                "connection_changed",
                "The connection changed while it was being updated. Refresh and try again.",
            )),
            _ => Err(unexpected_mutation()),
        }
    }

    pub async fn delete_connection(&self, connection_id: &str) -> Result<(), ProviderServiceError> {
        validate_identifier(connection_id, 32)?;
        match self
            .repository
            .delete_provider_connection(connection_id)
            .await
            .map_err(ProviderServiceError::dependency)?
        {
            ProviderMutationOutcome::Applied => Ok(()),
            ProviderMutationOutcome::NotFound => Err(connection_not_found()),
            ProviderMutationOutcome::ConnectionInUse => Err(ProviderServiceError::public(
                ProviderServiceErrorKind::Conflict,
                "connection_in_use",
                "Remove this connection from its model roles before deleting it.",
            )),
            ProviderMutationOutcome::ConnectionModelJobActive => Err(connection_job_active()),
            _ => Err(unexpected_mutation()),
        }
    }

    pub async fn delete_connection_credential(
        &self,
        connection_id: &str,
    ) -> Result<ProviderConnectionView, ProviderServiceError> {
        validate_identifier(connection_id, 32)?;
        match self
            .repository
            .clear_provider_credential(connection_id)
            .await
            .map_err(ProviderServiceError::dependency)?
        {
            ProviderMutationOutcome::Applied => self.connection_view(connection_id).await,
            ProviderMutationOutcome::NotFound => Err(connection_not_found()),
            ProviderMutationOutcome::ConnectionModelJobActive => Err(connection_job_active()),
            _ => Err(unexpected_mutation()),
        }
    }

    pub async fn prepare_connection_verification(
        &self,
        connection_id: &str,
    ) -> Result<ProviderVerificationTarget, ProviderServiceError> {
        validate_identifier(connection_id, 32)?;
        let connection = match self
            .repository
            .prepare_provider_connection(connection_id)
            .await
            .map_err(ProviderServiceError::dependency)?
        {
            ProviderConnectionPreparation::Ready(connection) => *connection,
            ProviderConnectionPreparation::NotFound => return Err(connection_not_found()),
            ProviderConnectionPreparation::ModelJobActive => {
                return Err(connection_job_active());
            }
        };
        let api_key = self.decrypt_credential(&connection).await?;
        Ok(ProviderVerificationTarget {
            connection_id: connection.id.clone(),
            adapter_id: connection.adapter_id.clone(),
            base_url: connection.base_url.clone(),
            api_key,
            allow_private_network: connection.allow_private_network,
            fingerprint: connection.fingerprint(),
        })
    }

    pub async fn finish_connection_verification(
        &self,
        target: &ProviderVerificationTarget,
        result: ProviderVerificationResult,
    ) -> Result<ProviderVerificationView, ProviderServiceError> {
        let adapter = require_adapter(&target.adapter_id)?;
        let error_code = normalize_verification_error(result.error_code.as_deref());
        let verified = result.verified && error_code.is_none();
        let models = if verified {
            bounded_unique_strings(&result.models, 200)
        } else {
            Vec::new()
        };
        let known = PROVIDER_CAPABILITIES
            .iter()
            .map(|definition| definition.id)
            .collect::<BTreeSet<_>>();
        let supported = adapter
            .capability_ids
            .iter()
            .copied()
            .collect::<BTreeSet<_>>();
        let capability_ids = if verified {
            bounded_unique_strings(&result.capability_ids, PROVIDER_CAPABILITIES.len())
                .into_iter()
                .filter(|value| {
                    known.contains(value.as_str()) && supported.contains(value.as_str())
                })
                .collect()
        } else {
            Vec::new()
        };
        let verification = ProviderVerificationWrite {
            connection_id: target.connection_id.clone(),
            expected_fingerprint: target.fingerprint.clone(),
            verified,
            error_code: if verified {
                None
            } else {
                Some(error_code.unwrap_or_else(|| "verification_failed".to_owned()))
            },
            models,
            capability_ids,
        };
        let connection = match self
            .repository
            .finish_provider_verification(&verification)
            .await
            .map_err(ProviderServiceError::dependency)?
        {
            ProviderVerificationWriteOutcome::Applied(connection) => *connection,
            ProviderVerificationWriteOutcome::NotFound => return Err(connection_not_found()),
            ProviderVerificationWriteOutcome::ModelJobActive => {
                return Err(connection_job_active());
            }
            ProviderVerificationWriteOutcome::Changed => {
                return Err(ProviderServiceError::public(
                    ProviderServiceErrorKind::Conflict,
                    "connection_changed",
                    "The connection changed while verification was running. Verify it again.",
                ));
            }
        };
        let connection = ProviderConnectionView::from(&connection);
        Ok(ProviderVerificationView {
            verified: connection.verification_status == ProviderVerificationStatus::Verified,
            error_code: connection.verification_error_code.clone(),
            models: connection.verified_models.clone(),
            connection,
        })
    }

    pub async fn list_model_roles(&self) -> Result<Vec<ModelRoleView>, ProviderServiceError> {
        let connections = self
            .repository
            .provider_connections()
            .await
            .map_err(ProviderServiceError::dependency)?
            .into_iter()
            .map(|connection| (connection.id.clone(), connection))
            .collect::<BTreeMap<_, _>>();
        let roles = self
            .repository
            .model_roles()
            .await
            .map_err(ProviderServiceError::dependency)?
            .into_iter()
            .filter(|role| model_role(&role.role_id).is_some())
            .map(|role| (role.role_id.clone(), role))
            .collect::<BTreeMap<_, _>>();
        let cipher = self.credentials.current_cipher().await.ok();
        Ok(MODEL_ROLES
            .iter()
            .map(|definition| {
                let role = roles.get(definition.id);
                let connection = role.and_then(|role| connections.get(&role.connection_id));
                let credential_available = connection.is_some_and(|connection| {
                    connection.credential_saved()
                        && cipher.as_ref().is_some_and(|cipher| {
                            cipher
                                .decrypt(
                                    &connection.id,
                                    &connection.encrypted_api_key,
                                    &connection.api_key_nonce,
                                )
                                .is_ok()
                        })
                });
                self.role_view(definition, role, connection, credential_available)
            })
            .collect())
    }

    pub async fn prepare_role_conformance(
        &self,
        role_id: &str,
    ) -> Result<ProviderConformanceTarget, ProviderServiceError> {
        let definition = configurable_role(role_id)?;
        let runtime = match self
            .repository
            .prepare_model_role(role_id)
            .await
            .map_err(ProviderServiceError::dependency)?
        {
            ProviderRolePreparation::Ready(runtime) => *runtime,
            ProviderRolePreparation::NotConfigured => return Err(role_not_configured()),
            ProviderRolePreparation::ConnectionNotFound => {
                return Err(connection_not_found());
            }
            ProviderRolePreparation::ModelJobActive => return Err(role_job_active()),
        };
        if ProviderVerificationStatus::parse(&runtime.connection.verification_status)
            != ProviderVerificationStatus::Verified
        {
            return Err(ProviderServiceError::public(
                ProviderServiceErrorKind::Conflict,
                "connection_not_verified",
                "Verify this provider connection before testing the model.",
            ));
        }
        if !capabilities_satisfy(
            &verified_capabilities(&runtime.connection),
            definition.required_capability_ids,
        ) {
            return Err(incompatible_connection());
        }
        let api_key = self.decrypt_credential(&runtime.connection).await?;
        let runtime_fingerprint = self.role_runtime_fingerprint(&runtime.role, &runtime.connection);
        Ok(ProviderConformanceTarget {
            role_id: role_id.to_owned(),
            execution: ProviderExecutionTarget {
                adapter_id: runtime.connection.adapter_id.clone(),
                base_url: runtime.connection.base_url.clone(),
                api_key,
                allow_private_network: runtime.connection.allow_private_network,
                model_id: runtime.role.model_id.clone(),
                timeout_seconds: runtime.role.timeout_seconds,
                max_output_tokens: runtime.role.max_output_tokens,
                thinking_mode: ThinkingMode::parse(&runtime.role.thinking_mode),
            },
            challenge: random_conformance_challenge()?,
            runtime_fingerprint,
            role_configuration_fingerprint: runtime.role.configuration_fingerprint(),
            connection_fingerprint: runtime.connection.fingerprint(),
        })
    }

    pub async fn finish_role_conformance(
        &self,
        target: &ProviderConformanceTarget,
        result: ProviderConformanceResult,
    ) -> Result<ProviderConformanceView, ProviderServiceError> {
        let error_code = normalize_verification_error(result.error_code.as_deref());
        let passed = result.passed && error_code.is_none();
        let write = ProviderConformanceWrite {
            role_id: target.role_id.clone(),
            expected_role_configuration_fingerprint: target.role_configuration_fingerprint.clone(),
            expected_connection_fingerprint: target.connection_fingerprint.clone(),
            runtime_fingerprint: target.runtime_fingerprint.clone(),
            passed,
            error_code: if passed {
                None
            } else {
                Some(error_code.unwrap_or_else(|| "conformance_failed".to_owned()))
            },
        };
        let runtime = match self
            .repository
            .finish_role_conformance(&write)
            .await
            .map_err(ProviderServiceError::dependency)?
        {
            ProviderConformanceWriteOutcome::Applied(runtime) => *runtime,
            ProviderConformanceWriteOutcome::ModelJobActive => return Err(role_job_active()),
            ProviderConformanceWriteOutcome::RoleChanged
            | ProviderConformanceWriteOutcome::ConnectionChanged => {
                return Err(ProviderServiceError::public(
                    ProviderServiceErrorKind::Conflict,
                    "role_changed",
                    "The model role changed while its test was running. Test it again.",
                ));
            }
        };
        let definition = configurable_role(&target.role_id)?;
        let role = self.role_view(
            definition,
            Some(&runtime.role),
            Some(&runtime.connection),
            true,
        );
        Ok(ProviderConformanceView {
            passed: role.conformance_status == ModelConformanceStatus::Passed,
            error_code: role.conformance_error_code.clone(),
            provider_model_id: bounded_optional_text(result.provider_model_id, 256),
            finish_reason: bounded_optional_text(result.finish_reason, 64),
            input_tokens: result.input_tokens,
            output_tokens: result.output_tokens,
            role,
        })
    }

    pub async fn update_model_role(
        &self,
        role_id: &str,
        mut request: ModelRoleUpdate,
    ) -> Result<ModelRoleView, ProviderServiceError> {
        let definition = configurable_role(role_id)?;
        normalize_required(&mut request.connection_id, 32)?;
        normalize_required(&mut request.model_id, 256)?;
        if !(5..=300).contains(&request.timeout_seconds)
            || !(128..=65_536).contains(&request.max_output_tokens)
        {
            return Err(validation_error());
        }
        let connection = self.connection_record(&request.connection_id).await?;
        let adapter = require_adapter(&connection.adapter_id)?;
        if !capabilities_satisfy(adapter.capability_ids, definition.required_capability_ids) {
            return Err(incompatible_connection());
        }
        if request.enabled
            && ProviderVerificationStatus::parse(&connection.verification_status)
                != ProviderVerificationStatus::Verified
        {
            return Err(ProviderServiceError::public(
                ProviderServiceErrorKind::Conflict,
                "connection_not_verified",
                "Verify this provider connection before enabling the role.",
            ));
        }
        if request.enabled
            && !capabilities_satisfy(
                &verified_capabilities(&connection),
                definition.required_capability_ids,
            )
        {
            return Err(incompatible_connection());
        }
        if request.enabled {
            self.decrypt_credential(&connection).await?;
        }
        let current = self
            .repository
            .model_roles()
            .await
            .map_err(ProviderServiceError::dependency)?
            .into_iter()
            .find(|role| role.role_id == role_id);
        let runtime_changed = current.as_ref().is_none_or(|current| {
            current.connection_id != request.connection_id
                || current.model_id != request.model_id
                || current.timeout_seconds != request.timeout_seconds
                || current.max_output_tokens != request.max_output_tokens
                || ThinkingMode::parse(&current.thinking_mode) != request.thinking_mode
        });
        if request.enabled {
            let tested = current.as_ref().is_some_and(|current| {
                !runtime_changed
                    && self.current_conformance_status(current, &connection)
                        == ModelConformanceStatus::Passed
            });
            if !tested {
                return Err(ProviderServiceError::public(
                    ProviderServiceErrorKind::Conflict,
                    "model_not_tested",
                    "Save and test this model configuration before enabling it.",
                ));
            }
        }
        let record = ModelRoleRecord {
            role_id: role_id.to_owned(),
            connection_id: request.connection_id,
            model_id: request.model_id,
            enabled: request.enabled,
            timeout_seconds: request.timeout_seconds,
            max_output_tokens: request.max_output_tokens,
            thinking_mode: request.thinking_mode.as_str().to_owned(),
            conformance_status: current
                .as_ref()
                .filter(|_| !runtime_changed)
                .map_or("never", |current| current.conformance_status.as_str())
                .to_owned(),
            conformance_error_code: current
                .as_ref()
                .filter(|_| !runtime_changed)
                .and_then(|current| current.conformance_error_code.clone()),
            conformance_fingerprint: current
                .as_ref()
                .filter(|_| !runtime_changed)
                .and_then(|current| current.conformance_fingerprint.clone()),
            last_conformance_at_unix_seconds: current
                .as_ref()
                .filter(|_| !runtime_changed)
                .and_then(|current| current.last_conformance_at_unix_seconds),
            updated_at_unix_seconds: 0,
        };
        match self
            .repository
            .save_model_role(&connection.fingerprint(), &record, runtime_changed)
            .await
            .map_err(ProviderServiceError::dependency)?
        {
            ProviderMutationOutcome::Applied => self.model_role_view(role_id).await,
            ProviderMutationOutcome::NotFound => Err(connection_not_found()),
            ProviderMutationOutcome::RoleModelJobActive => Err(role_job_active()),
            ProviderMutationOutcome::Changed => Err(ProviderServiceError::public(
                ProviderServiceErrorKind::Conflict,
                "connection_changed",
                "The provider connection changed while the role was being updated. Refresh and try again.",
            )),
            _ => Err(unexpected_mutation()),
        }
    }

    pub async fn delete_model_role(&self, role_id: &str) -> Result<(), ProviderServiceError> {
        if model_role(role_id).is_none() {
            return Err(role_not_found());
        }
        match self
            .repository
            .delete_model_role(role_id)
            .await
            .map_err(ProviderServiceError::dependency)?
        {
            ProviderMutationOutcome::Applied | ProviderMutationOutcome::NotFound => Ok(()),
            ProviderMutationOutcome::RoleModelJobActive => Err(role_job_active()),
            _ => Err(unexpected_mutation()),
        }
    }

    async fn connection_record(
        &self,
        connection_id: &str,
    ) -> Result<ProviderConnectionRecord, ProviderServiceError> {
        self.repository
            .provider_connection(connection_id)
            .await
            .map_err(ProviderServiceError::dependency)?
            .ok_or_else(connection_not_found)
    }

    async fn connection_view(
        &self,
        connection_id: &str,
    ) -> Result<ProviderConnectionView, ProviderServiceError> {
        self.connection_record(connection_id)
            .await
            .map(|record| ProviderConnectionView::from(&record))
    }

    async fn model_role_view(&self, role_id: &str) -> Result<ModelRoleView, ProviderServiceError> {
        self.list_model_roles()
            .await?
            .into_iter()
            .find(|role| role.role_id == role_id)
            .ok_or_else(role_not_found)
    }

    pub async fn current_role_runtime_fingerprint(
        &self,
        role_id: &str,
    ) -> Result<Option<String>, ProviderServiceError> {
        Ok(self
            .current_role_review_identity(role_id)
            .await?
            .map(|identity| identity.runtime_fingerprint))
    }

    pub async fn current_role_review_identity(
        &self,
        role_id: &str,
    ) -> Result<Option<ModelRoleReviewIdentity>, ProviderServiceError> {
        if model_role(role_id).is_none() {
            return Err(role_not_found());
        }
        let role = self
            .repository
            .model_roles()
            .await
            .map_err(ProviderServiceError::dependency)?
            .into_iter()
            .find(|role| role.role_id == role_id);
        let Some(role) = role else {
            return Ok(None);
        };
        let connection = self
            .repository
            .provider_connection(&role.connection_id)
            .await
            .map_err(ProviderServiceError::dependency)?;
        Ok(connection.map(|connection| ModelRoleReviewIdentity {
            runtime_fingerprint: self.role_runtime_fingerprint(&role, &connection),
            configuration_fingerprint: role.configuration_fingerprint(),
            connection_fingerprint: connection.fingerprint(),
        }))
    }

    pub async fn prepare_role_execution(
        &self,
        role_id: &str,
    ) -> Result<ResolvedRoleExecution, ProviderServiceError> {
        let definition = configurable_role(role_id)?;
        let role = self
            .repository
            .model_roles()
            .await
            .map_err(ProviderServiceError::dependency)?
            .into_iter()
            .find(|role| role.role_id == role_id)
            .ok_or_else(role_not_enabled)?;
        if !role.enabled {
            return Err(role_not_enabled());
        }
        let connection = self
            .repository
            .provider_connection(&role.connection_id)
            .await
            .map_err(ProviderServiceError::dependency)?
            .ok_or_else(connection_not_found)?;
        if ProviderVerificationStatus::parse(&connection.verification_status)
            != ProviderVerificationStatus::Verified
        {
            return Err(connection_not_verified());
        }
        if !capabilities_satisfy(
            &verified_capabilities(&connection),
            definition.required_capability_ids,
        ) {
            return Err(incompatible_connection());
        }
        if self.current_conformance_status(&role, &connection) != ModelConformanceStatus::Passed {
            return Err(model_not_tested());
        }
        let api_key = self.decrypt_credential(&connection).await?;
        Ok(ResolvedRoleExecution {
            role_id: role_id.to_owned(),
            execution: ProviderExecutionTarget {
                adapter_id: connection.adapter_id.clone(),
                base_url: connection.base_url.clone(),
                api_key,
                allow_private_network: connection.allow_private_network,
                model_id: role.model_id.clone(),
                timeout_seconds: role.timeout_seconds,
                max_output_tokens: role.max_output_tokens,
                thinking_mode: ThinkingMode::parse(&role.thinking_mode),
            },
            fingerprint: self.role_runtime_fingerprint(&role, &connection),
            role_configuration_fingerprint: role.configuration_fingerprint(),
            connection_fingerprint: connection.fingerprint(),
            connection_name: connection.name,
        })
    }

    #[must_use]
    pub fn role_not_found_error(&self) -> ProviderServiceError {
        role_not_found()
    }

    fn role_view(
        &self,
        definition: &ModelRoleDefinition,
        role: Option<&ModelRoleRecord>,
        connection: Option<&ProviderConnectionRecord>,
        credential_available: bool,
    ) -> ModelRoleView {
        let verification_status = connection
            .map(|connection| ProviderVerificationStatus::parse(&connection.verification_status));
        let conformance_status = match (role, connection) {
            (Some(role), Some(connection)) => self.current_conformance_status(role, connection),
            _ => ModelConformanceStatus::Never,
        };
        let required = definition
            .required_capability_ids
            .iter()
            .map(|value| (*value).to_owned())
            .collect::<Vec<_>>();
        let capabilities_satisfied = connection.is_some_and(|connection| {
            capabilities_satisfy(
                &verified_capabilities(connection),
                definition.required_capability_ids,
            )
        });
        ModelRoleView {
            role_id: definition.id.to_owned(),
            label: definition.label.to_owned(),
            description: definition.description.to_owned(),
            required_capability_ids: required,
            configuration_available: definition.configuration_available,
            connection_id: role.map(|role| role.connection_id.clone()),
            connection_name: connection.map(|connection| connection.name.clone()),
            model_id: role.map_or_else(String::new, |role| role.model_id.clone()),
            enabled: role.is_some_and(|role| role.enabled),
            effective_enabled: role.is_some_and(|role| role.enabled)
                && definition.configuration_available
                && capabilities_satisfied
                && verification_status == Some(ProviderVerificationStatus::Verified)
                && conformance_status == ModelConformanceStatus::Passed
                && credential_available,
            timeout_seconds: role.map_or(30, |role| role.timeout_seconds),
            max_output_tokens: role.map_or(2_000, |role| role.max_output_tokens),
            thinking_mode: role.map_or(ThinkingMode::ProviderDefault, |role| {
                ThinkingMode::parse(&role.thinking_mode)
            }),
            verification_status,
            conformance_status,
            conformance_error_code: role
                .filter(|_| conformance_status == ModelConformanceStatus::Failed)
                .and_then(|role| role.conformance_error_code.clone()),
            last_conformance_at_unix_seconds: role
                .filter(|_| conformance_status != ModelConformanceStatus::Never)
                .and_then(|role| role.last_conformance_at_unix_seconds),
            updated_at_unix_seconds: role.map(|role| role.updated_at_unix_seconds),
        }
    }

    fn current_conformance_status(
        &self,
        role: &ModelRoleRecord,
        connection: &ProviderConnectionRecord,
    ) -> ModelConformanceStatus {
        if role.conformance_fingerprint.as_deref()
            != Some(self.role_runtime_fingerprint(role, connection).as_str())
        {
            return ModelConformanceStatus::Never;
        }
        ModelConformanceStatus::parse(&role.conformance_status)
    }

    #[must_use]
    pub fn role_runtime_fingerprint(
        &self,
        role: &ModelRoleRecord,
        connection: &ProviderConnectionRecord,
    ) -> String {
        let runtime_contract = model_role(&role.role_id)
            .map_or("unknown-role/v1", |definition| definition.runtime_contract);
        let connection_fingerprint = connection.fingerprint();
        let timeout_seconds = role.timeout_seconds.to_string();
        let max_output_tokens = role.max_output_tokens.to_string();
        let value = [
            PROVIDER_CONFORMANCE_CHALLENGE_CONTRACT,
            STRUCTURED_HARNESS_CONTRACT,
            connection_fingerprint.as_str(),
            role.role_id.as_str(),
            runtime_contract,
            self.role_contract_digests
                .get(&role.role_id)
                .map_or(self.executable_contract_digest.as_str(), String::as_str),
            role.model_id.as_str(),
            timeout_seconds.as_str(),
            max_output_tokens.as_str(),
            ThinkingMode::parse(&role.thinking_mode).as_str(),
        ]
        .join("\0");
        format!("{:x}", Sha256::digest(value.as_bytes()))
    }

    async fn decrypt_credential(
        &self,
        connection: &ProviderConnectionRecord,
    ) -> Result<ProviderSecret, ProviderServiceError> {
        if !connection.credential_saved() {
            return Err(ProviderServiceError::public(
                ProviderServiceErrorKind::Conflict,
                "credential_missing",
                "Save an API key for this provider connection before using it.",
            ));
        }
        self.credentials
            .current_cipher()
            .await
            .map_err(map_credential_error)?
            .decrypt(
                &connection.id,
                &connection.encrypted_api_key,
                &connection.api_key_nonce,
            )
            .map_err(map_credential_error)
    }
}

fn verified_capabilities(connection: &ProviderConnectionRecord) -> Vec<String> {
    if ProviderVerificationStatus::parse(&connection.verification_status)
        != ProviderVerificationStatus::Verified
    {
        return Vec::new();
    }
    let known = PROVIDER_CAPABILITIES
        .iter()
        .map(|definition| definition.id)
        .collect::<BTreeSet<_>>();
    bounded_unique_strings(
        &connection.verified_capability_ids,
        PROVIDER_CAPABILITIES.len(),
    )
    .into_iter()
    .filter(|value| known.contains(value.as_str()))
    .collect()
}

fn bounded_unique_strings(values: &[String], limit: usize) -> Vec<String> {
    let mut seen = BTreeSet::new();
    values
        .iter()
        .filter(|value| !value.is_empty() && value.len() <= 256)
        .filter(|value| seen.insert((*value).clone()))
        .take(limit)
        .cloned()
        .collect()
}

fn normalize_verification_error(value: Option<&str>) -> Option<String> {
    let value = value?.trim();
    if value.is_empty()
        || value.len() > 64
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'_')
    {
        return Some("verification_failed".to_owned());
    }
    Some(value.to_owned())
}

fn bounded_optional_text(value: Option<String>, maximum: usize) -> Option<String> {
    value.filter(|value| !value.is_empty() && value.len() <= maximum)
}

fn random_conformance_challenge() -> Result<String, ProviderServiceError> {
    let mut bytes = [0_u8; CONFORMANCE_CHALLENGE_BYTES];
    OsRng.try_fill_bytes(&mut bytes).map_err(|_| {
        ProviderServiceError::public(
            ProviderServiceErrorKind::Unavailable,
            "secure_random_unavailable",
            "A secure model-test challenge could not be generated.",
        )
    })?;
    Ok(URL_SAFE_NO_PAD.encode(bytes))
}

fn capabilities_satisfy(available: &[impl AsRef<str>], required: &[&str]) -> bool {
    let available = available.iter().map(AsRef::as_ref).collect::<BTreeSet<_>>();
    required.iter().all(|value| available.contains(value))
}

fn reset_connection_verification(connection: &mut ProviderConnectionRecord) {
    connection.verification_status = "never".to_owned();
    connection.verification_error_code = None;
    connection.verified_models.clear();
    connection.verified_capability_ids.clear();
    connection.last_verified_at_unix_seconds = None;
}

fn normalize_required(value: &mut String, maximum: usize) -> Result<(), ProviderServiceError> {
    *value = value.trim().to_owned();
    if value.is_empty() || value.chars().count() > maximum {
        return Err(validation_error());
    }
    Ok(())
}

fn validate_identifier(value: &str, maximum: usize) -> Result<(), ProviderServiceError> {
    if value.is_empty() || value.chars().count() > maximum {
        return Err(validation_error());
    }
    Ok(())
}

fn validate_api_key(value: &str) -> Result<(), ProviderServiceError> {
    if value.is_empty() || value.chars().count() > 4_096 {
        return Err(validation_error());
    }
    Ok(())
}

fn require_adapter(
    adapter_id: &str,
) -> Result<&'static ProviderAdapterDefinition, ProviderServiceError> {
    provider_adapter(adapter_id).ok_or_else(|| {
        ProviderServiceError::public(
            ProviderServiceErrorKind::Invalid,
            "unsupported_adapter",
            "That provider adapter is not supported.",
        )
    })
}

fn configurable_role(role_id: &str) -> Result<&'static ModelRoleDefinition, ProviderServiceError> {
    let definition = model_role(role_id).ok_or_else(role_not_found)?;
    if !definition.configuration_available {
        return Err(ProviderServiceError::public(
            ProviderServiceErrorKind::Conflict,
            "role_not_available",
            "This model task is planned but is not available yet.",
        ));
    }
    Ok(definition)
}

fn map_policy_error(error: ProviderPolicyError) -> ProviderServiceError {
    ProviderServiceError::public(
        ProviderServiceErrorKind::Invalid,
        if error.code == "unsupported_adapter" {
            "unsupported_adapter"
        } else {
            "invalid_provider_url"
        },
        "The provider URL is invalid for this connection type.",
    )
}

fn map_credential_error(error: ProviderCredentialError) -> ProviderServiceError {
    let code = match error.code.as_str() {
        "master_key_not_configured" => "master_key_not_configured",
        "invalid_master_key" => "invalid_master_key",
        "master_key_file_unreadable" => "master_key_file_unreadable",
        "master_key_file_unsafe" => "master_key_file_unsafe",
        "master_key_file_permissions" => "master_key_file_permissions",
        _ => "credential_unreadable",
    };
    ProviderServiceError::public(
        ProviderServiceErrorKind::Unavailable,
        code,
        "Provider credential storage is unavailable.",
    )
}

fn validation_error() -> ProviderServiceError {
    ProviderServiceError::public(
        ProviderServiceErrorKind::Invalid,
        "validation_error",
        "The provider request is invalid.",
    )
}

fn duplicate_name() -> ProviderServiceError {
    ProviderServiceError::public(
        ProviderServiceErrorKind::Conflict,
        "duplicate_connection_name",
        "A provider connection with that name already exists.",
    )
}

fn connection_not_found() -> ProviderServiceError {
    ProviderServiceError::public(
        ProviderServiceErrorKind::NotFound,
        "connection_not_found",
        "Provider connection not found.",
    )
}

fn role_not_found() -> ProviderServiceError {
    ProviderServiceError::public(
        ProviderServiceErrorKind::NotFound,
        "role_not_found",
        "Model role not found.",
    )
}

fn role_not_configured() -> ProviderServiceError {
    ProviderServiceError::public(
        ProviderServiceErrorKind::Conflict,
        "role_not_configured",
        "Save a model configuration before testing it.",
    )
}

fn role_not_enabled() -> ProviderServiceError {
    ProviderServiceError::public(
        ProviderServiceErrorKind::Conflict,
        "role_not_enabled",
        "This model role is not enabled.",
    )
}

fn connection_not_verified() -> ProviderServiceError {
    ProviderServiceError::public(
        ProviderServiceErrorKind::Conflict,
        "connection_not_verified",
        "Verify this provider connection before using the role.",
    )
}

fn model_not_tested() -> ProviderServiceError {
    ProviderServiceError::public(
        ProviderServiceErrorKind::Conflict,
        "model_not_tested",
        "Test this model configuration before using the role.",
    )
}

fn incompatible_connection() -> ProviderServiceError {
    ProviderServiceError::public(
        ProviderServiceErrorKind::Conflict,
        "incompatible_connection",
        "This connection does not support the capabilities required by this task.",
    )
}

fn connection_job_active() -> ProviderServiceError {
    ProviderServiceError::public(
        ProviderServiceErrorKind::Conflict,
        "connection_model_job_active",
        "Wait for or cancel active model work before changing this connection.",
    )
}

fn role_job_active() -> ProviderServiceError {
    ProviderServiceError::public(
        ProviderServiceErrorKind::Conflict,
        "role_model_job_active",
        "Wait for or cancel active model work before changing this role.",
    )
}

fn unexpected_mutation() -> ProviderServiceError {
    ProviderServiceError::public(
        ProviderServiceErrorKind::Dependency,
        "provider_storage_failed",
        "Provider storage failed.",
    )
}

#[cfg(test)]
mod tests {
    use super::{
        MODEL_ROLES, OPENAI_COMPATIBLE_ADAPTER, PROVIDER_ADAPTERS,
        PROVIDER_CONFORMANCE_CHALLENGE_CONTRACT, ProviderConformanceTarget,
        ProviderConnectionRecord, ProviderExecutionTarget, ProviderSecret, StructuredModelResult,
        ThinkingMode,
    };

    #[test]
    fn static_provider_inventory_is_unique_and_role_contracts_are_versioned() {
        let adapters = PROVIDER_ADAPTERS
            .iter()
            .map(|definition| definition.id)
            .collect::<std::collections::BTreeSet<_>>();
        assert_eq!(adapters.len(), PROVIDER_ADAPTERS.len());
        let roles = MODEL_ROLES
            .iter()
            .map(|definition| definition.id)
            .collect::<std::collections::BTreeSet<_>>();
        assert_eq!(roles.len(), MODEL_ROLES.len());
        assert!(
            MODEL_ROLES
                .iter()
                .all(|role| role.runtime_contract.contains("/v"))
        );
    }

    #[test]
    fn connection_debug_never_contains_ciphertext_or_nonce() {
        let record = ProviderConnectionRecord {
            id: "a".repeat(32),
            name: "Fixture".to_owned(),
            adapter_id: "openai-compatible/v1".to_owned(),
            base_url: "https://example.test/v1".to_owned(),
            encrypted_api_key: "secret-ciphertext".to_owned(),
            api_key_nonce: "secret-nonce".to_owned(),
            api_key_hint: "••••cret".to_owned(),
            allow_private_network: false,
            verification_status: "never".to_owned(),
            verification_error_code: None,
            verified_models: Vec::new(),
            verified_capability_ids: Vec::new(),
            last_verified_at_unix_seconds: None,
            created_at_unix_seconds: 0,
            updated_at_unix_seconds: 0,
        };
        let rendered = format!("{record:?}");
        assert!(!rendered.contains("secret-ciphertext"));
        assert!(!rendered.contains("secret-nonce"));
    }

    #[test]
    fn conformance_challenge_is_schema_bound_and_requires_an_exact_echo() {
        let target = ProviderConformanceTarget {
            role_id: "music_tagger".to_owned(),
            execution: ProviderExecutionTarget {
                adapter_id: OPENAI_COMPATIBLE_ADAPTER.to_owned(),
                base_url: "https://example.test/v1".to_owned(),
                api_key: ProviderSecret::new("secret"),
                allow_private_network: false,
                model_id: "fixture-model".to_owned(),
                timeout_seconds: 30,
                max_output_tokens: 2_000,
                thinking_mode: ThinkingMode::ProviderDefault,
            },
            challenge: "one-time-challenge".to_owned(),
            runtime_fingerprint: "a".repeat(64),
            role_configuration_fingerprint: "b".repeat(64),
            connection_fingerprint: "c".repeat(64),
        };
        let request = target.request();
        assert_eq!(request.max_output_tokens, 256);
        assert_eq!(
            request.output_schema.as_ref().and_then(|schema| {
                schema
                    .pointer("/properties/challenge/const")
                    .and_then(serde_json::Value::as_str)
            }),
            Some("one-time-challenge")
        );
        assert_eq!(
            request.output_schema.as_ref().and_then(|schema| {
                schema
                    .pointer("/properties/checks/uniqueItems")
                    .and_then(serde_json::Value::as_bool)
            }),
            Some(true)
        );
        let passed = target.evaluate(StructuredModelResult {
            outcome: crate::assistant::ProviderAttemptOutcome::ResponseReceived,
            succeeded: true,
            error_code: None,
            payload: Some(serde_json::json!({
                "contract": PROVIDER_CONFORMANCE_CHALLENGE_CONTRACT,
                "challenge": "one-time-challenge",
                "checks": ["schema", "identity"],
                "accepted": true,
            })),
            provider_model_id: Some("fixture-model".to_owned()),
            finish_reason: Some("stop".to_owned()),
            input_tokens: Some(12),
            output_tokens: Some(8),
        });
        assert!(passed.passed);

        let mismatch = target.evaluate(StructuredModelResult {
            outcome: crate::assistant::ProviderAttemptOutcome::ResponseReceived,
            succeeded: true,
            error_code: None,
            payload: Some(serde_json::json!({
                "contract": PROVIDER_CONFORMANCE_CHALLENGE_CONTRACT,
                "challenge": "different",
                "checks": ["schema", "identity"],
                "accepted": true,
            })),
            provider_model_id: None,
            finish_reason: None,
            input_tokens: None,
            output_tokens: None,
        });
        assert!(!mismatch.passed);
        assert_eq!(mismatch.error_code.as_deref(), Some("conformance_mismatch"));
    }
}
