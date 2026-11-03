"""Small, Namespace-bound MCP operations over the public SDK."""

from __future__ import annotations

import base64
import json
import tempfile
from collections.abc import AsyncIterator, Awaitable
from contextlib import asynccontextmanager
from pathlib import Path
from typing import Annotated, Literal
from urllib.parse import quote

import anyio
from nous_sdk import NousClient
from nous_sdk.contracts import ImportDocument, MediaReference
from nous_sdk.contracts.common import FormationPurpose
from nous_sdk.contracts.context import ChangeWorkContext, WorkContextContent
from nous_sdk.contracts.media import MediaStrategy, PixelRect, TimeSpan
from nous_sdk.contracts.public import ReadView
from nous_sdk.contracts.targets import MediaTarget
from nous_sdk.contracts.text_import import TextImport
from nous_sdk.errors import NousError
from nous_sdk.namespace import NamespaceClient
from nous_sdk.presentation import present, present_query
from pydantic import Field, JsonValue, TypeAdapter

from mcp.server import MCPServer
from mcp.server.mcpserver import Context
from mcp.server.mcpserver.exceptions import ToolError
from mcp.types import CallToolResult, ImageContent, ResourceLink, TextContent, ToolAnnotations
from nous.platform.errors import DomainError
from nous.transports.ingestion import import_result, job_result

_MEDIA_TYPES = {
    "image/jpeg": "jpeg",
    "image/png": "png",
    "image/webp": "webp",
    "image/gif": "gif",
    "video/mp4": "mp4",
    "video/webm": "webm",
    "audio/mpeg": "mp3",
    "audio/wav": "wav",
    "application/pdf": "pdf",
}
_INLINE_IMAGE_BYTES = 5 * 1024 * 1024


async def media_bytes(namespace: NamespaceClient, reference: MediaReference) -> tuple[bytes, str]:
    with tempfile.TemporaryDirectory(prefix="nous-mcp-media-") as directory:
        destination = Path(directory) / "content"
        downloaded = await namespace.materials.download(reference, destination)
        data = await anyio.Path(destination).read_bytes()
    return data, (downloaded.media_type or "application/octet-stream").split(";", 1)[0]


async def media_info(namespace: NamespaceClient, value: ReadView) -> tuple[str, int | None] | None:
    if not isinstance(value.value, dict):
        return None
    if value.kind == "material":
        data = value.value
    elif value.kind == "source_region":
        material_id = value.value.get("material_id")
        if not isinstance(material_id, str):
            return None
        material = await namespace.materials.get(material_id)
        media_type = material.media_type
        if material.media_info and material.media_info.frames and material.media_info.frames > 1:
            media_type = "video/mp4"
        elif (value.value.get("time_span") or value.value.get("pixel_rect")) and media_type.startswith("video/"):
            media_type = "video/mp4"
        elif value.value.get("time_span") and media_type.startswith("audio/"):
            media_type = "audio/wav"
        elif value.value.get("pixel_rect") and media_type.startswith("image/"):
            media_type = "image/png"
        return media_type, material.byte_len
    elif value.kind in {"representation", "derived_region"}:
        data = value.value.get("payload")
        if not isinstance(data, dict) or data.get("kind") != "artifact":
            return None
    else:
        return None
    media_type = data.get("media_type")
    length = data.get("byte_len")
    if not isinstance(media_type, str):
        return None
    return media_type, length if isinstance(length, int) else None


def media_uri(reference: MediaReference, media_type: str) -> str:
    kind = _MEDIA_TYPES.get(media_type, "binary")
    parts = (reference.kind, reference.id, reference.observation_id or "-")
    return "nous://media/" + kind + "/" + "/".join(quote(part, safe="") for part in parts)


def result(value: object) -> CallToolResult:
    data = TypeAdapter(dict[str, JsonValue]).validate_python(present(value))
    text = data.get("text")
    if not isinstance(text, str):
        text = json.dumps(data, ensure_ascii=False, indent=2)
    return CallToolResult(content=[TextContent(type="text", text=text)], structured_content=data)


async def complete(operation: Awaitable[object]) -> CallToolResult:
    try:
        return result(await operation)
    except (NousError, DomainError, ValueError) as error:
        raise ToolError(str(error)) from error


def create_server(
    instance: Path,
    url: str,
    namespace: str,
    credential_file: Path | None = None,
    *,
    read_only: bool = False,
) -> MCPServer:
    @asynccontextmanager
    async def lifespan(server: MCPServer) -> AsyncIterator[NamespaceClient]:
        async with NousClient(
            url, credential_file=credential_file or instance / "credentials" / "admin.token"
        ) as root:
            yield await root.namespace(namespace)

    server = MCPServer(
        "Nous",
        instructions="Search with nous_query, then read returned references with nous_read. Sources and earlier revisions remain available. Work references retain a continuing goal and its reading anchors.",
        lifespan=lifespan,
    )
    read = ToolAnnotations(read_only_hint=True, destructive_hint=False)
    write = ToolAnnotations(read_only_hint=False, destructive_hint=False)

    def bound(ctx: Context[NamespaceClient, object]) -> NamespaceClient:
        return ctx.request_context.lifespan_context

    async def media_resource(
        ctx: Context[NamespaceClient, object], kind: str, identity: str, observation: str
    ) -> bytes:
        reference = MediaReference.model_validate(
            {
                "kind": kind,
                "id": identity,
                "observation_id": None if observation == "-" else observation,
            }
        )
        data, _ = await media_bytes(bound(ctx), reference)
        return data

    for media_type, name in (*_MEDIA_TYPES.items(), ("application/octet-stream", "binary")):
        _ = server.resource(
            "nous://media/" + name + "/{kind}/{identity}/{observation}",
            name="media_" + name,
            description="Read an authorized material or exact media region.",
            mime_type=media_type,
        )(media_resource)

    @server.tool(annotations=read)
    async def nous_query(
        ctx: Context[NamespaceClient, object],
        query: str,
        context: str | None = None,
        background: str = "",
        references: list[str] | None = None,
        limit: Annotated[int, Field(ge=1, le=50)] = 8,
        profile: str | None = None,
        media: list[str] | None = None,
        trace: bool = False,
    ) -> CallToolResult:
        """Find relevant text using natural language or NousQL. media accepts material or region references from ingest, read or a previous query. Set trace to inspect the retrieval channels."""
        try:
            found = await bound(ctx).query(
                query, context=context, background=background, references=tuple(references or ()),
                limit=limit, profile=profile, media=tuple(media or ())
            )
            if trace:
                return result(
                    {
                        **present_query(found).model_dump(mode="json"),
                        "trace": found.plan,
                        "execution": found.execution.model_dump(mode="json")
                        if found.execution else None,
                    }
                )
            return result(found)
        except (NousError, DomainError, ValueError) as error:
            raise ToolError(str(error)) from error

    @server.tool(annotations=read if read_only else write)
    async def nous_read(
        ctx: Context[NamespaceClient, object],
        ref: str,
        sources: bool = False,
        cursor: str | None = None,
        offset: Annotated[int, Field(ge=0)] = 0,
        max_chars: Annotated[int, Field(ge=1, le=64000)] = 12000,
        time_span: TimeSpan | None = None,
        pixel_rect: PixelRect | None = None,
    ) -> CallToolResult:
        """Read a result, object, original or region. Select a time_span or pixel_rect on an original to create a reusable region. Images include visible content when small; media links can be read as MCP resources. Set sources to inspect provenance."""
        try:
            namespace = bound(ctx)
            selected_ref = ref
            if time_span is not None or pixel_rect is not None:
                if read_only:
                    raise ValueError("Creating a media region requires write access")
                selected = await namespace.materials.select(
                    ref, time_span=time_span, pixel_rect=pixel_rect
                )
                selected_ref = (
                    selected.addresses.exact if selected.addresses else 'source-region:' + selected.id
                )
            value = await namespace.read(
                selected_ref, sources=sources, cursor=cursor, offset=offset, max_chars=max_chars
            )
            response = result(value)
            info = await media_info(namespace, value)
            if info is None:
                return response
            target = (await namespace.resolve_reference(selected_ref)).reference
            if not isinstance(target, MediaTarget):
                return response
            media_type, size = info
            data = None
            if media_type.startswith("image/") and size is not None and size <= _INLINE_IMAGE_BYTES:
                data, media_type = await media_bytes(namespace, target.reference)
                size = len(data)
            uri = media_uri(target.reference, media_type)
            response.content.append(
                ResourceLink(
                    name=value.title or "media",
                    uri=uri,
                    mime_type=media_type,
                    size=size if data is not None or value.kind not in {"source_region", "derived_region"} else None,
                    description="Read this resource to obtain the authorized media bytes.",
                )
            )
            if data is not None and media_type.startswith("image/") and len(data) <= _INLINE_IMAGE_BYTES:
                response.content.append(
                    ImageContent(data=base64.b64encode(data).decode("ascii"), mime_type=media_type)
                )
            assert response.structured_content is not None
            response.structured_content["resource"] = uri
            response.structured_content["media_type"] = media_type
            return response
        except (NousError, DomainError, ValueError, OSError) as error:
            raise ToolError(str(error)) from error

    @server.tool(annotations=read)
    async def nous_history(
        ctx: Context[NamespaceClient, object], ref: str, limit: Annotated[int, Field(ge=1, le=100)] = 20
    ) -> CallToolResult:
        """Read retained revisions of a cognitive object."""
        values = await bound(ctx).history(ref, limit=limit)
        return result({"items": [present(value) for value in values]})

    @server.tool(annotations=read)
    async def nous_status(ctx: Context[NamespaceClient, object], ref: str | None = None) -> CallToolResult:
        """Inspect a job or recent work. A completed import returns its source and cognition references."""
        scope = bound(ctx)
        if ref:
            async def read_status() -> object:
                return await job_result(scope, await scope.jobs.get(ref))

            return await complete(read_status())
        return result(
            {
                "namespace": scope.identity.slug,
                "state": scope.identity.state,
                "jobs": await scope.jobs.list(limit=10),
            }
        )

    if not read_only:

        @server.tool(annotations=write)
        async def nous_remember(
            ctx: Context[NamespaceClient, object],
            content: str,
            source: str | None = None,
            tags: list[str] | None = None,
            pin: bool = False,
        ) -> CallToolResult:
            """Save explicit content as a memory with its original source, optional tags and retention pin."""
            return await complete(
                bound(ctx).remember(content, source=source, tags=tuple(tags or ()), pin=pin)
            )

        @server.tool(annotations=write)
        async def nous_correct(
            ctx: Context[NamespaceClient, object], ref: str, content: str, reason: str = "", source: str | None = None
        ) -> CallToolResult:
            """Correct the selected memory, retaining its earlier revision and provenance."""
            return await complete(bound(ctx).correct(ref, content, reason=reason, source=source))

        @server.tool(annotations=write)
        async def nous_ingest(
            ctx: Context[NamespaceClient, object],
            content: str | None = None,
            path: str | None = None,
            source: str | None = None,
            formation_purpose: FormationPurpose | None = None,
            media_strategy: MediaStrategy = "direct_structured",
            wait: bool = True,
        ) -> CallToolResult:
            """Import either text or a file path accessible to this MCP server. Set formation_purpose for memory, graph or both; omit it to save the original without a model. When wait is false, use nous_status with the returned job reference to get the result."""
            try:
                if (content is None) == (path is None):
                    raise ValueError("Provide either content or one accessible file path")
                namespace = bound(ctx)
                completion = "formed" if formation_purpose is not None and wait else "accepted"
                if path is not None:
                    expanded = await anyio.Path(path).expanduser()
                    selected = Path(await expanded.resolve())
                    files = [{"path": str(selected), "source": source}] if source else [selected]
                    submission = await namespace.client.import_files(
                        namespace.namespace_id,
                        files,
                        transfer_root=selected.parent,
                        formation_purpose=formation_purpose,
                        media_strategy=media_strategy,
                        completion=completion,
                    )
                else:
                    assert content is not None
                    job = await namespace.ingest(
                        TextImport(
                            inputs=(ImportDocument(text=content, source=source),),
                            formation_purpose=formation_purpose,
                            completion=completion,
                        )
                    )
                    submission = job.submission
                return result(await import_result(namespace, submission, wait=wait))
            except (NousError, DomainError, ValueError, OSError) as error:
                raise ToolError(str(error)) from error

        @server.tool(annotations=write)
        async def nous_work_open(
            ctx: Context[NamespaceClient, object], goal: str, background: str = "", anchors: list[str] | None = None
        ) -> CallToolResult:
            """Start continuing work with a goal, known background and exact reading references."""
            from nous_sdk.contracts.targets import CognitiveTarget, MediaTarget

            scope = bound(ctx)
            targets = []
            for ref in anchors or ():
                value = await scope.resolve_reference(ref)
                if isinstance(value.reference, (CognitiveTarget, MediaTarget)):
                    targets.append(value.reference)
            return await complete(
                scope.runtime.create_work(
                    ChangeWorkContext(action='create', content=WorkContextContent(purpose=goal, context_text=background, anchors=tuple(targets))),
                )
            )

        @server.tool(annotations=write)
        async def nous_work_update(
            ctx: Context[NamespaceClient, object],
            ref: str,
            background: str | None = None,
            questions: list[str] | None = None,
            action: Literal["edit", "pause", "resume", "end"] = "edit",
        ) -> CallToolResult:
            """Update work progress and unresolved questions, or pause, resume or finish the work."""
            scope = bound(ctx)
            current = await scope.runtime.work(ref)
            updates = {}
            if background is not None:
                updates["context_text"] = background
            if questions is not None:
                updates["unresolved_questions"] = tuple(questions)
            return await complete(
                scope.runtime.change_work(
                    ChangeWorkContext(work_context_id=current.id, expected_revision=current.revision, action=action, content=current.content.model_copy(update=updates) if action == 'edit' else None),
                )
            )

    return server


def run(
    instance: Path,
    url: str,
    namespace_id: str,
    credential_file: Path | None = None,
    *,
    read_only: bool = False,
) -> None:
    create_server(instance, url, namespace_id, credential_file, read_only=read_only).run(
        transport="stdio"
    )
