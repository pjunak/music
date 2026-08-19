from app.assistant.providers.execution import StructuredModelResult
from app.assistant.providers.usage import ProviderUsageAccumulator


def test_usage_accumulator_reports_tokens_and_missing_usage_separately() -> None:
    usage = ProviderUsageAccumulator()

    returned = usage.record(
        StructuredModelResult(
            True,
            None,
            {},
            provider_model_id="planner-v2",
            input_tokens=120,
            output_tokens=30,
        )
    )
    usage.record(
        StructuredModelResult(
            False,
            "timeout",
            provider_model_id="planner-v2",
        )
    )

    assert returned.provider_model_id == "planner-v2"
    assert usage.summary().model_dump(mode="json") == {
        "schema_version": "assistant-provider-usage/v1",
        "attempted_requests": 2,
        "input_tokens": 120,
        "output_tokens": 30,
        "input_tokens_reported_requests": 1,
        "output_tokens_reported_requests": 1,
        "provider_model_ids": ["planner-v2"],
        "provider_model_ids_truncated": False,
    }
    assert usage.checkpoint() == {
        "schema_version": "assistant-provider-usage-checkpoint/v1",
        "usage": usage.summary().model_dump(mode="json"),
    }


def test_usage_accumulator_bounds_provider_controlled_model_ids() -> None:
    usage = ProviderUsageAccumulator()

    for index in range(10):
        usage.record(
            StructuredModelResult(
                True,
                None,
                {},
                provider_model_id=f"model-{index}",
            )
        )
    usage.record(
        StructuredModelResult(
            True,
            None,
            {},
            provider_model_id="x" * 257,
        )
    )

    summary = usage.summary()
    assert summary.attempted_requests == 11
    assert summary.provider_model_ids == [f"model-{index}" for index in range(8)]
    assert summary.provider_model_ids_truncated is True
