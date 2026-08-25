from __future__ import annotations

from datetime import datetime
from typing import Literal

from pydantic import (
    BaseModel,
    ConfigDict,
    Field,
    SecretStr,
    field_validator,
    model_validator,
)


class StrictProviderModel(BaseModel):
    model_config = ConfigDict(extra="forbid")


class ProviderUsageSummary(StrictProviderModel):
    schema_version: Literal["assistant-provider-usage/v1"]
    attempted_requests: int = Field(ge=0)
    input_tokens: int = Field(ge=0)
    output_tokens: int = Field(ge=0)
    input_tokens_reported_requests: int = Field(ge=0)
    output_tokens_reported_requests: int = Field(ge=0)
    provider_model_ids: list[str] = Field(max_length=8)
    provider_model_ids_truncated: bool

    @field_validator("provider_model_ids")
    @classmethod
    def validate_model_ids(cls, value: list[str]) -> list[str]:
        if any(not model_id or len(model_id) > 256 for model_id in value):
            raise ValueError("provider model IDs must contain 1-256 characters")
        if len(set(value)) != len(value):
            raise ValueError("provider model IDs must be unique")
        return value

    @model_validator(mode="after")
    def validate_reported_requests(self) -> ProviderUsageSummary:
        if self.input_tokens_reported_requests > self.attempted_requests:
            raise ValueError("reported input-token requests exceed attempted requests")
        if self.output_tokens_reported_requests > self.attempted_requests:
            raise ValueError("reported output-token requests exceed attempted requests")
        return self


class ProviderCapabilityOut(StrictProviderModel):
    id: str
    label: str
    description: str


class ProviderAdapterOut(StrictProviderModel):
    id: str
    label: str
    description: str
    capability_ids: list[str]


class ModelRoleDefinitionOut(StrictProviderModel):
    id: str
    label: str
    description: str
    required_capability_ids: list[str]
    configuration_available: bool


class ProviderFrameworkStatusOut(StrictProviderModel):
    credential_storage_ready: bool
    credential_storage_error: str | None
    credential_storage_source: Literal["environment", "file"] | None
    credential_storage_key_id: str | None
    credential_storage_key_file_path: str | None
    credential_storage_host_directory_hint: str | None
    credential_storage_can_initialize: bool
    credential_storage_initialization_error: str | None
    capabilities: list[ProviderCapabilityOut]
    adapters: list[ProviderAdapterOut]
    roles: list[ModelRoleDefinitionOut]


class ProviderCredentialStorageReset(StrictProviderModel):
    current_password: SecretStr = Field(min_length=1, max_length=256)


class ProviderCredentialStorageResetOut(StrictProviderModel):
    deleted_credentials: int = Field(ge=0)
    master_key_removed: bool
    master_key_removal_error: str | None
    status: ProviderFrameworkStatusOut


class ProviderConnectionCreate(StrictProviderModel):
    name: str = Field(min_length=1, max_length=128)
    adapter_id: str = Field(min_length=1, max_length=64)
    base_url: str = Field(min_length=1, max_length=2048)
    api_key: SecretStr = Field(min_length=1, max_length=4096)
    allow_private_network: bool = False

    @field_validator("name", "adapter_id", "base_url", mode="before")
    @classmethod
    def strip_text(cls, value: object) -> object:
        return value.strip() if isinstance(value, str) else value


class ProviderConnectionUpdate(StrictProviderModel):
    name: str | None = Field(default=None, min_length=1, max_length=128)
    adapter_id: str | None = Field(default=None, min_length=1, max_length=64)
    base_url: str | None = Field(default=None, min_length=1, max_length=2048)
    api_key: SecretStr | None = Field(default=None, min_length=1, max_length=4096)
    allow_private_network: bool | None = None

    @field_validator("name", "adapter_id", "base_url", mode="before")
    @classmethod
    def strip_text(cls, value: object) -> object:
        return value.strip() if isinstance(value, str) else value


class ProviderConnectionOut(StrictProviderModel):
    id: str
    name: str
    adapter_id: str
    base_url: str
    credential_saved: bool
    key_hint: str | None
    allow_private_network: bool
    verification_status: Literal["never", "verified", "failed"]
    verification_error_code: str | None
    verified_models: list[str]
    verified_capability_ids: list[str]
    last_verified_at: datetime | None
    created_at: datetime
    updated_at: datetime


class ProviderVerificationOut(StrictProviderModel):
    connection: ProviderConnectionOut
    verified: bool
    error_code: str | None
    models: list[str]


class ModelRoleUpdate(StrictProviderModel):
    connection_id: str = Field(min_length=1, max_length=32)
    model_id: str = Field(min_length=1, max_length=256)
    enabled: bool = False
    timeout_seconds: int = Field(default=30, ge=5, le=300)
    max_output_tokens: int = Field(default=2000, ge=128, le=65536)
    thinking_mode: Literal["provider_default", "enabled", "disabled"] = (
        "provider_default"
    )

    @field_validator("connection_id", "model_id", mode="before")
    @classmethod
    def strip_text(cls, value: object) -> object:
        return value.strip() if isinstance(value, str) else value


class ModelRoleOut(StrictProviderModel):
    role_id: str
    label: str
    description: str
    required_capability_ids: list[str]
    configuration_available: bool
    connection_id: str | None
    connection_name: str | None
    model_id: str
    enabled: bool
    effective_enabled: bool
    timeout_seconds: int
    max_output_tokens: int
    thinking_mode: Literal["provider_default", "enabled", "disabled"]
    verification_status: Literal["never", "verified", "failed"] | None
    conformance_status: Literal["never", "passed", "failed"]
    conformance_error_code: str | None
    last_conformance_at: datetime | None
    updated_at: datetime | None


class ModelConformanceOut(StrictProviderModel):
    role: ModelRoleOut
    passed: bool
    error_code: str | None
    contract_version: Literal["assistant-provider-conformance/v3"]
    provider_model_id: str | None
    finish_reason: str | None
    input_tokens: int | None
    output_tokens: int | None
    duration_ms: int


class ModelQualityEvaluationOut(StrictProviderModel):
    evaluation_id: str
    role_id: str
    label: str
    description: str
    status: Literal["never", "passed", "failed", "stale"]
    suite_id: str
    passed_cases: int
    total_cases: int
    last_job_id: str | None
    last_evaluated_at: datetime | None


class ModelQualityJobResult(StrictProviderModel):
    schema_version: Literal["assistant-model-quality-result/v1"]
    execution_scope: Literal["full_suite", "diagnostic_retest"] = "full_suite"
    role_id: str = Field(min_length=1, max_length=64)
    evaluation_id: str = Field(min_length=1, max_length=128)
    role_fingerprint: str = Field(pattern=r"^[a-f0-9]{64}$")
    evaluation: dict[str, object]
    usage: ProviderUsageSummary
