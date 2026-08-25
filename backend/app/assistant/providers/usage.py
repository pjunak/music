"""Bounded provider-usage accounting shared by durable Assistant jobs."""

from dataclasses import dataclass, field
from typing import TypeGuard

from app.assistant.providers.execution import StructuredModelResult
from app.assistant.providers.schemas import ProviderUsageSummary

_MAX_PROVIDER_MODEL_IDS = 8


def _valid_token_count(value: int | None) -> TypeGuard[int]:
    return isinstance(value, int) and not isinstance(value, bool) and value >= 0


@dataclass
class ProviderUsageAccumulator:
    """Record attempted model calls without guessing cost or missing token values."""

    attempted_requests: int = 0
    input_tokens: int = 0
    output_tokens: int = 0
    input_tokens_reported_requests: int = 0
    output_tokens_reported_requests: int = 0
    _provider_model_ids: list[str] = field(default_factory=list)
    _provider_model_ids_truncated: bool = False

    def record(self, result: StructuredModelResult) -> StructuredModelResult:
        self.attempted_requests += 1
        if _valid_token_count(result.input_tokens):
            self.input_tokens += result.input_tokens
            self.input_tokens_reported_requests += 1
        if _valid_token_count(result.output_tokens):
            self.output_tokens += result.output_tokens
            self.output_tokens_reported_requests += 1

        model_id = result.provider_model_id
        if (
            isinstance(model_id, str)
            and 0 < len(model_id) <= 256
            and model_id not in self._provider_model_ids
        ):
            if len(self._provider_model_ids) < _MAX_PROVIDER_MODEL_IDS:
                self._provider_model_ids.append(model_id)
            else:
                self._provider_model_ids_truncated = True
        return result

    def summary(self) -> ProviderUsageSummary:
        return ProviderUsageSummary(
            schema_version="assistant-provider-usage/v1",
            attempted_requests=self.attempted_requests,
            input_tokens=self.input_tokens,
            output_tokens=self.output_tokens,
            input_tokens_reported_requests=self.input_tokens_reported_requests,
            output_tokens_reported_requests=self.output_tokens_reported_requests,
            provider_model_ids=list(self._provider_model_ids),
            provider_model_ids_truncated=self._provider_model_ids_truncated,
        )

    def checkpoint(self) -> dict[str, object]:
        return {
            "schema_version": "assistant-provider-usage-checkpoint/v1",
            "usage": self.summary().model_dump(mode="json"),
        }
