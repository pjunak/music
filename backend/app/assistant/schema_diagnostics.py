"""Safe, bounded diagnostics for rejected provider output schemas."""

from __future__ import annotations

import re
from typing import Any

from pydantic import BaseModel, ValidationError

_SAFE_ERROR_TYPE = re.compile(r"^[a-z0-9_]{1,64}$")


def _schema_field_names(model: type[BaseModel]) -> frozenset[str]:
    names: set[str] = set()

    def visit(value: Any) -> None:
        if isinstance(value, dict):
            properties = value.get("properties")
            if isinstance(properties, dict):
                names.update(str(name) for name in properties)
            for nested in value.values():
                visit(nested)
        elif isinstance(value, list):
            for nested in value:
                visit(nested)

    visit(model.model_json_schema())
    return frozenset(names)


def safe_validation_diagnostic(
    error: ValidationError,
    model: type[BaseModel],
) -> str:
    """Describe the first schema issue without echoing model-generated content."""

    issues = error.errors(include_url=False, include_context=False, include_input=False)
    if not issues:
        return "response: validation_error"

    first = issues[0]
    allowed_fields = _schema_field_names(model)
    location: list[str] = []
    for part in first.get("loc", ()):
        if isinstance(part, int) and part >= 0:
            location.append(str(part))
        elif isinstance(part, str) and part in allowed_fields:
            location.append(part)
        else:
            location.append("<unexpected-field>")
    path = ".".join(location) or "response"
    raw_error_type = first.get("type")
    error_type = (
        raw_error_type
        if isinstance(raw_error_type, str) and _SAFE_ERROR_TYPE.fullmatch(raw_error_type)
        else "validation_error"
    )
    count_suffix = f"; {len(issues)} issues" if len(issues) > 1 else ""
    return f"{path}: {error_type}{count_suffix}"
