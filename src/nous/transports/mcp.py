"""Agent-facing tools backed by the official MCP server and application catalogue."""

import base64
import json
from collections.abc import AsyncGenerator
from contextlib import asynccontextmanager
from pathlib import Path
from typing import TYPE_CHECKING

from mcp.server import Server, ServerRequestContext
from mcp.types import (
    CallToolRequestParams,
    CallToolResult,
    ListToolsResult,
    PaginatedRequestParams,
    TextContent,
    Tool,
)
from pydantic import JsonValue, TypeAdapter

from nous.application.inputs import StreamInput
from nous.cognition.material_contracts import MaterialRead
from nous.cognition.retrieval.contracts import RetrievalOutcome
from nous.common.errors import NousError
from nous.content.contracts import ExportOffer
from nous.presentation import select_language
from nous.presentation.evidence import format_material_read, format_retrieval
from nous.presentation.json_selection import select_json, validate_json_pointers

from .asgi import BearerAuth, serve_asgi
from .shared import InlineInput, ServiceAddress, error_value

if TYPE_CHECKING:
    from starlette.types import ASGIApp

    from nous.application.contracts import EventEnvelope
    from nous.application.dispatch import Application
    from nous.content.artifacts import ArtifactStore

_AGENT_OPERATIONS = frozenset(
    {
        "retrieval.query",
        "retrieval.save_evidence",
        "materials.read",
        "materials.follow",
        "steward.arrange",
        "work.create",
        "work.read",
        "work.revise",
        "work.set_active",
        "work.record_use",
        "work.list_uses",
        "feedback.record",
        "feedback.record_query",
        "feedback.read",
        "feedback.list",
        "feedback.resolve",
        "records.create_journal",
        "records.add_note",
        "records.read",
        "records.replace",
        "records.apply",
        "records.list",
        "records.history",
        "records.diff",
        "records.adopt_draft",
        "writing.generate",
        "writing.draft_from_result",
        "writing.read_draft",
        "drafts.revise",
        "content.read_result",
        "content.read_artifact",
        "content.read_selections",
        "models.rank",
        "models.evaluate",
    }
)
DEFAULT_INLINE_BYTES = 512 * 1024


def _object_schema(schema: dict[str, JsonValue]) -> tuple[dict[str, JsonValue], bool]:
    if schema.get("type") == "object" or "properties" in schema:
        return schema, False
    nested = dict(schema)
    definitions = nested.pop("$defs", None)
    wrapped: dict[str, JsonValue] = {
        "type": "object",
        "properties": {"request": nested},
        "required": ["request"],
    }
    if definitions is not None:
        wrapped["$defs"] = definitions
    return wrapped, True


def _result_schema(schema: dict[str, JsonValue]) -> dict[str, JsonValue]:
    nested = dict(schema)
    definitions = nested.pop("$defs", None)
    wrapped: dict[str, JsonValue] = {
        "type": "object",
        "properties": {
            "value": {"anyOf": [nested, {"type": "object", "additionalProperties": True}]}
        },
        "required": ["value"],
    }
    if definitions is not None:
        wrapped["$defs"] = definitions
    return wrapped


def _selectable(schema: dict[str, JsonValue]) -> dict[str, JsonValue]:
    properties = TypeAdapter(dict[str, JsonValue]).validate_python(schema.get("properties", {}))
    if "_select" in properties:
        raise ValueError("_select is reserved for result field selection")
    return {
        **schema,
        "properties": {
            **properties,
            "_select": {"type": "array", "items": {"type": "string"}},
        },
    }


def create_mcp_server(
    app: Application, artifacts: ArtifactStore, *, max_inline_bytes: int = DEFAULT_INLINE_BYTES
) -> Server[object]:
    if max_inline_bytes < 1:
        raise ValueError("Inline content limit must be positive")
    operations = {item["name"]: item for item in app.list() if item["name"] in _AGENT_OPERATIONS}
    schemas: dict[str, dict[str, JsonValue]] = {}
    wrapped: dict[str, bool] = {}
    tools: list[Tool] = []
    for name, item in operations.items():
        description = app.describe(name)
        request_schema = TypeAdapter(dict[str, JsonValue]).validate_python(description["request"])
        result_schema = TypeAdapter(dict[str, JsonValue]).validate_python(description["result"])
        schema, wrapped[name] = _object_schema(request_schema)
        schemas[name] = _selectable(schema)
        tools.append(
            Tool(
                name=name,
                description=item["description"],
                input_schema=schemas[name],
                output_schema=_result_schema(result_schema),
            )
        )
    upload_schema: dict[str, JsonValue] = {
        "type": "object",
        "properties": {
            "data": {"type": "string", "description": "Base64 content bytes."},
            "name": {"type": "string"},
            "source": {"type": ["string", "null"]},
        },
        "required": ["data", "name"],
        "additionalProperties": False,
    }
    download_schema: dict[str, JsonValue] = {
        "type": "object",
        "properties": {"ref": {"type": "string"}},
        "required": ["ref"],
        "additionalProperties": False,
    }
    upload_schema, download_schema = _selectable(upload_schema), _selectable(download_schema)
    schemas.update({"content.upload": upload_schema, "content.download": download_schema})
    tools.extend(
        (
            Tool(
                name="content.upload",
                description="Accept base64 content and return its formal artifact reference.",
                input_schema=upload_schema,
            ),
            Tool(
                name="content.download",
                description="Read bounded artifact bytes as base64.",
                input_schema=download_schema,
            ),
        )
    )

    async def list_tools(
        ctx: ServerRequestContext[object], params: PaginatedRequestParams | None
    ) -> ListToolsResult:
        return ListToolsResult(tools=tools)

    async def call_tool(
        ctx: ServerRequestContext[object], params: CallToolRequestParams
    ) -> CallToolResult:
        try:
            arguments = dict(params.arguments or {})
            pointers = TypeAdapter(tuple[str, ...]).validate_python(arguments.pop("_select", ()))
            validate_json_pointers(pointers)
            display: str | None = None
            if params.name == "content.upload":
                item = InlineInput.model_validate(arguments)
                if len(item.data) > ((max_inline_bytes + 2) // 3) * 4:
                    raise NousError(
                        "input.too_large",
                        "Content exceeds the MCP inline limit; use a streaming transport",
                    )
                data = base64.b64decode(item.data, validate=True)
                if len(data) > max_inline_bytes:
                    raise NousError(
                        "input.too_large",
                        "Content exceeds the MCP inline limit; use a streaming transport",
                    )
                value = await app.call_name(
                    "content.accept",
                    {"key": "content", "name": item.name},
                    inputs={"content": StreamInput((data,), name=item.name, source=item.source)},
                    select=pointers,
                )
            elif params.name == "content.download":
                ref = TypeAdapter(str).validate_python(arguments.get("ref"))
                offer = ExportOffer.model_validate(
                    await app.call_name("content.export_offer", {"ref": ref})
                )
                data = await artifacts.read_bytes(offer.artifact_ref, max_bytes=max_inline_bytes)
                value = {
                    "artifact": offer.artifact_ref.model_dump(mode="json"),
                    "media_type": offer.media_type,
                    "data": base64.b64encode(data).decode("ascii"),
                }
                value = select_json(value, pointers)
                display = f"{offer.suggested_name}: {len(data)} bytes"
            else:
                if params.name not in operations:
                    raise NousError(
                        "operation.unknown",
                        "Unknown agent operation",
                        details={"name": params.name},
                    )
                request = arguments.get("request") if wrapped[params.name] else arguments

                async def observe(event: EventEnvelope) -> None:
                    await ctx.session.report_progress(
                        progress=float(event.sequence), message=event.model_dump_json()
                    )

                value = await app.call_name(params.name, request, observer=observe, select=pointers)
            structured = {"value": select_json(value)}
            if pointers:
                display = json.dumps(structured["value"], ensure_ascii=False, indent=2)
            elif display is None:
                language = select_language(
                    configured=artifacts.content.execution.snapshot.values.localization.language
                )
                if isinstance(value, RetrievalOutcome):
                    display = format_retrieval(value, language)
                elif isinstance(value, MaterialRead):
                    display = format_material_read(value, language)
                else:
                    display = json.dumps(structured["value"], ensure_ascii=False, indent=2)
            return CallToolResult(
                content=[TextContent(text=display)],
                structured_content=structured,
            )
        except Exception as error:
            return CallToolResult(
                content=[TextContent(text=json.dumps(error_value(error), ensure_ascii=False))],
                is_error=True,
            )

    return Server(
        "nous",
        version="0.1.0",
        on_list_tools=list_tools,
        on_call_tool=call_tool,
        get_tool_input_schema=lambda name: schemas.get(name),
    )


def create_mcp_app(
    app: Application,
    artifacts: ArtifactStore,
    *,
    token: str,
    host: str = "127.0.0.1",
    max_inline_bytes: int = DEFAULT_INLINE_BYTES,
) -> ASGIApp:
    server = create_mcp_server(app, artifacts, max_inline_bytes=max_inline_bytes)
    max_request_bytes = ((max_inline_bytes + 2) // 3) * 4 + 65536
    return BearerAuth(
        server.streamable_http_app(host=host, max_request_body_size=max_request_bytes), token
    )


@asynccontextmanager
async def serve_mcp(
    app: Application,
    artifacts: ArtifactStore,
    *,
    token: str,
    host: str = "127.0.0.1",
    port: int = 0,
    max_inline_bytes: int = DEFAULT_INLINE_BYTES,
    tls_certfile: str | Path | None = None,
    tls_keyfile: str | Path | None = None,
) -> AsyncGenerator[ServiceAddress]:
    application = create_mcp_app(
        app, artifacts, token=token, host=host, max_inline_bytes=max_inline_bytes
    )
    async with serve_asgi(
        application, host=host, port=port, tls_certfile=tls_certfile, tls_keyfile=tls_keyfile
    ) as address:
        yield address
