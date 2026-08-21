from pydantic import BaseModel, ConfigDict, ValidationError

from app.assistant.schema_diagnostics import safe_validation_diagnostic


class _NestedOutput(BaseModel):
    model_config = ConfigDict(extra="forbid", strict=True)

    confidence: str


class _Output(BaseModel):
    model_config = ConfigDict(extra="forbid", strict=True)

    tracks: list[_NestedOutput]


def test_schema_diagnostic_reports_known_path_without_echoing_output() -> None:
    try:
        _Output.model_validate({"tracks": [{"private-title": "secret value"}]})
    except ValidationError as error:
        diagnostic = safe_validation_diagnostic(error, _Output)
    else:
        raise AssertionError("invalid provider output unexpectedly passed validation")

    assert "tracks.0.confidence: missing" in diagnostic
    assert "private-title" not in diagnostic
    assert "secret value" not in diagnostic
