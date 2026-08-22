from typing import Literal

import pytest
from pydantic import BaseModel, ConfigDict

from app.assistant.structured_harness import (
    STRUCTURED_HARNESS_CONTRACT,
    StructuredTaskDefinition,
    build_structured_request,
    numbered_rules,
)


class _Input(BaseModel):
    model_config = ConfigDict(extra="forbid", frozen=True)

    value: str


class _Output(BaseModel):
    model_config = ConfigDict(extra="forbid", frozen=True, strict=True)

    schema_version: Literal["test-output/v1"]
    accepted: bool


def test_harness_generates_one_schema_for_prompt_and_provider() -> None:
    request = build_structured_request(
        StructuredTaskDefinition(
            task_id="test-task",
            role="A test role.",
            objective="Return a validated decision.",
            rules=numbered_rules("Use only the supplied value."),
            untrusted_data=("value",),
        ),
        _Input(value="untrusted"),
        _Output,
        output_example={"schema_version": "test-output/v1", "accepted": False},
        max_output_tokens=128,
    )

    assert request.output_schema_name == "test-task-response"
    assert request.output_schema is not None
    assert request.output_schema["additionalProperties"] is False
    assert f"HARNESS CONTRACT: {STRUCTURED_HARNESS_CONTRACT}" in request.system_prompt
    assert '"additionalProperties":false' in request.system_prompt
    assert '"schema_version":"test-output/v1"' in request.system_prompt
    assert request.user_prompt == '{"value":"untrusted"}'


def test_task_id_reserves_space_for_provider_schema_suffix() -> None:
    with pytest.raises(ValueError, match="1-55"):
        StructuredTaskDefinition(
            task_id="x" * 56,
            role="A test role.",
            objective="Return a result.",
            rules=("Use the input.",),
            untrusted_data=("value",),
        )


def test_harness_uses_task_specific_closed_schema_in_prompt_and_request() -> None:
    def close_schema(schema: dict[str, object]) -> dict[str, object]:
        properties = schema["properties"]
        assert isinstance(properties, dict)
        accepted = properties["accepted"]
        assert isinstance(accepted, dict)
        accepted["const"] = True
        return schema

    request = build_structured_request(
        StructuredTaskDefinition(
            task_id="closed-test-task",
            role="A test role.",
            objective="Return a validated decision.",
            rules=("Use the closed schema.",),
            untrusted_data=("value",),
        ),
        _Input(value="untrusted"),
        _Output,
        output_example={"schema_version": "test-output/v1", "accepted": True},
        max_output_tokens=128,
        schema_transform=close_schema,
    )

    assert request.output_schema is not None
    properties = request.output_schema["properties"]
    assert properties["accepted"]["const"] is True
    assert '"const":true' in request.system_prompt
