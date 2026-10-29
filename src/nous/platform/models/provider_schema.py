"""Provider response formats reuse the same installed SDK schema preparation as execution."""

from __future__ import annotations

from openai.lib._parsing import type_to_response_format_param
from pydantic import BaseModel, JsonValue, TypeAdapter
from pydantic_ai import NativeOutput
from pydantic_ai._json_schema import JsonSchemaTransformerWithOutputOverride  # pyright: ignore[reportPrivateImportUsage]
from pydantic_ai._output import OutputSchema  # pyright: ignore[reportPrivateImportUsage]
from pydantic_ai.models import _customize_output_object  # pyright: ignore[reportPrivateUsage]
from pydantic_ai.providers.openai import OpenAIProvider


def native_output[T: BaseModel | str](output_type: type[T]) -> type[str] | NativeOutput[T]:
    return str if output_type is str else NativeOutput(output_type)


def response_format(
    output_type: type[BaseModel] | type[str], model_name: str, *, backend: str
) -> dict[str, JsonValue] | None:
    if output_type is str:
        return None
    if backend == "openai_sdk":
        return TypeAdapter(dict[str, JsonValue]).validate_python(
            type_to_response_format_param(output_type)
        )
    if backend != "pydantic_ai":
        raise ValueError("Unknown generation schema backend")
    output = OutputSchema[BaseModel | str].build(NativeOutput(output_type)).object_def
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
    output_type: type[BaseModel] | type[str], model_name: str
) -> dict[str, dict[str, JsonValue] | None]:
    return {
        backend: response_format(output_type, model_name, backend=backend)
        for backend in ("pydantic_ai", "openai_sdk")
    }
