"""Provider response formats reuse the same installed SDK schema preparation as execution."""

from __future__ import annotations

from nous_sdk.contracts import DomainError
from openai.lib._parsing import type_to_response_format_param
from pydantic import JsonValue, TypeAdapter
from pydantic_ai import NativeOutput, ToolOutput
from pydantic_ai._json_schema import JsonSchemaTransformerWithOutputOverride  # pyright: ignore[reportPrivateImportUsage]
from pydantic_ai._output import OutputSchema  # pyright: ignore[reportPrivateImportUsage]
from pydantic_ai.models import _customize_output_object  # pyright: ignore[reportPrivateUsage]
from pydantic_ai.providers.openai import OpenAIProvider
from typing_extensions import TypeForm


def output_contract[T](
    output_type: TypeForm[T], mode: str = "native"
) -> TypeForm[T] | NativeOutput[T] | ToolOutput[T]:
    if output_type is str:
        return output_type
    if mode == "tool":
        return ToolOutput(output_type)
    if mode == "native":
        return NativeOutput(output_type)
    raise DomainError(
        "model_output_capability", "This task requires native structured output or tools"
    )


def response_format(
    output_type: TypeForm[object], model_name: str, *, backend: str, mode: str = "native"
) -> dict[str, JsonValue] | None:
    if output_type is str or mode == "tool":
        return None
    if backend == "openai_sdk":
        if not isinstance(output_type, type):
            raise DomainError(
                "model_output_capability",
                "Native video structured output requires an object contract",
            )
        return TypeAdapter(dict[str, JsonValue]).validate_python(
            type_to_response_format_param(output_type)
        )
    if backend != "pydantic_ai":
        raise ValueError("Unknown generation schema backend")
    output = OutputSchema[object].build(NativeOutput(output_type)).object_def
    if output is None:
        raise ValueError("Native structured generation requires an output object")
    profile = OpenAIProvider.model_profile(model_name) or {}
    transformer = profile.get("json_schema_transformer")
    if transformer is not None:
        output_transformer = (
            transformer.output_object_transformer
            if issubclass(transformer, JsonSchemaTransformerWithOutputOverride)
            else transformer
        )
        output = _customize_output_object(output_transformer, output)
    # This envelope is OpenAIChatModel._map_json_schema's mapping. Schema mutation
    # above is entirely delegated to PydanticAI's actual output/profile preparation.
    schema: dict[str, object] = {
        "name": output.name or "final_result",
        "schema": output.json_schema,
    }
    if output.description:
        schema["description"] = output.description
    if profile.get("openai_supports_strict_tool_definition", True):
        schema["strict"] = output.strict
    return TypeAdapter(dict[str, JsonValue]).validate_python(
        {"type": "json_schema", "json_schema": schema}
    )


def response_formats(
    output_type: TypeForm[object], model_name: str
) -> dict[str, dict[str, JsonValue] | None]:
    return {
        backend: response_format(output_type, model_name, backend=backend)
        for backend in ("pydantic_ai", "openai_sdk")
    }
