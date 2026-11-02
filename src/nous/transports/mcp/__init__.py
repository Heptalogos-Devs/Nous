"""Small, Namespace-bound MCP operations over the public SDK."""

from __future__ import annotations

import json
from collections.abc import AsyncIterator, Awaitable
from contextlib import asynccontextmanager
from pathlib import Path
from typing import Annotated, Literal
from uuid import uuid4

from nous_sdk import NousClient
from nous_sdk.contracts.imports import FormationPurpose
from nous_sdk.contracts import DomainError
from nous_sdk.contracts.context import ChangeWorkContext, WorkContextContent
from nous_sdk.errors import NousError
from nous_sdk.namespace import NamespaceClient
from nous_sdk.presentation import present
from pydantic import Field, JsonValue, TypeAdapter

from mcp.server import MCPServer
from mcp.server.mcpserver import Context
from mcp.server.mcpserver.exceptions import ToolError
from mcp.types import CallToolResult, TextContent, ToolAnnotations


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

    @server.tool(annotations=read)
    async def nous_query(
        ctx: Context[NamespaceClient, object],
        query: str,
        context: str | None = None,
        limit: Annotated[int, Field(ge=1, le=50)] = 8,
        profile: str | None = None,
    ) -> CallToolResult:
        """Find relevant text using natural language or NousQL. Optional context is an existing work/session reference. Returns reusable result references."""
        return await complete(
            bound(ctx).query(query, context=context, limit=limit, profile=profile)
        )

    @server.tool(annotations=read)
    async def nous_read(
        ctx: Context[NamespaceClient, object],
        ref: str,
        sources: bool = False,
        cursor: str | None = None,
        offset: Annotated[int, Field(ge=0)] = 0,
        max_chars: Annotated[int, Field(ge=1, le=64000)] = 12000,
    ) -> CallToolResult:
        """Read a result reference, object, original material, work, job or maintenance run. Set sources to inspect its provenance; use next_cursor for the next page."""
        return await complete(
            bound(ctx).read(ref, sources=sources, cursor=cursor, offset=offset, max_chars=max_chars)
        )

    @server.tool(annotations=read)
    async def nous_history(
        ctx: Context[NamespaceClient, object], ref: str, limit: Annotated[int, Field(ge=1, le=100)] = 20
    ) -> CallToolResult:
        """Read retained revisions of a cognitive object."""
        values = await bound(ctx).history(ref, limit=limit)
        return result({"items": [present(value) for value in values]})

    @server.tool(annotations=read)
    async def nous_status(ctx: Context[NamespaceClient, object], ref: str | None = None) -> CallToolResult:
        """Inspect a job reference or the current Namespace and recent background work."""
        scope = bound(ctx)
        if ref:
            return await complete(scope.jobs.get(ref))
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
            content: str,
            source: str | None = None,
            formation_purpose: FormationPurpose | None = None,
        ) -> CallToolResult:
            """Capture a document and optionally form memory notes, graph links, or both."""
            job = await bound(ctx).ingest(
                content, source=source, formation_purpose=formation_purpose
            )
            return result({"ref": job.ref, "status": "queued"})

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
