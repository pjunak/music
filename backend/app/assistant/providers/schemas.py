from __future__ import annotations

from datetime import datetime
from typing import Literal

from pydantic import BaseModel, ConfigDict, Field, SecretStr, field_validator


class StrictProviderModel(BaseModel):
    model_config = ConfigDict(extra="forbid")


class ProviderAdapterOut(StrictProviderModel):
    id: str
    label: str
    description: str


class ModelRoleDefinitionOut(StrictProviderModel):
    id: str
    label: str
    description: str


class ProviderFrameworkStatusOut(StrictProviderModel):
    credential_storage_ready: bool
    credential_storage_error: str | None
    adapters: list[ProviderAdapterOut]
    roles: list[ModelRoleDefinitionOut]


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
    key_hint: str
    allow_private_network: bool
    verification_status: Literal["never", "verified", "failed"]
    verification_error_code: str | None
    verified_models: list[str]
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

    @field_validator("connection_id", "model_id", mode="before")
    @classmethod
    def strip_text(cls, value: object) -> object:
        return value.strip() if isinstance(value, str) else value


class ModelRoleOut(StrictProviderModel):
    role_id: str
    label: str
    description: str
    connection_id: str | None
    connection_name: str | None
    model_id: str
    enabled: bool
    effective_enabled: bool
    timeout_seconds: int
    max_output_tokens: int
    verification_status: Literal["never", "verified", "failed"] | None
    updated_at: datetime | None
