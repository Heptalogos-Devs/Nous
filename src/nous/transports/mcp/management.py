"""MCP management tools delegate to the same public SDK operation owners."""

from collections.abc import Callable
from contextlib import AbstractAsyncContextManager
from typing import Literal

from nous_wave import NousClient
from nous_wave.contracts.model_roles import ModelContractRequest, ModelPreviewRequest
from nous_wave.types import JsonObject
from pydantic import JsonValue

from mcp.server import MCPServer


def register_management(
    server: MCPServer,
    client: Callable[[], AbstractAsyncContextManager[NousClient]],
    namespace_id: str,
    subject_id: str,
) -> None:
    @server.tool()
    async def model_contract(command: ModelContractRequest) -> JsonObject:
        """Read a registered role's real input/output schema and active Prompt without a provider call."""
        async with client() as api:
            return (
                await api.model_contract(
                    {
                        **command.model_dump(mode="json"),
                        "namespace_id": namespace_id,
                        "subject_id": subject_id,
                    }
                )
            ).model_dump(mode="json")

    @server.tool()
    async def model_preview(command: ModelPreviewRequest) -> JsonObject:
        """Build the actual Prompt, schema, parameters and authorized media snapshot without a model call."""
        async with client() as api:
            return (
                await api.model_preview(
                    {
                        **command.model_dump(mode="json"),
                        "namespace_id": namespace_id,
                        "subject_id": subject_id,
                    }
                )
            ).model_dump(mode="json")

    def scoped(action: str, command: JsonObject) -> JsonObject:
        for name, value in (("namespace_id", namespace_id), ("subject_id", subject_id)):
            if command.get(name) not in (None, value):
                raise ValueError(
                    "This MCP connection is bound to its configured namespace and subject"
                )
        return {
            "action": action,
            "request": {**command, "namespace_id": namespace_id, "subject_id": subject_id},
        }

    @server.tool()
    async def manage_sources(action: str, command: JsonObject) -> JsonObject:
        """Source list/preview/trash/restore/purge and operation history in the bound subject.

        Supply the public SourcesRequest action and its request object. Preview
        destructive changes before submitting the command with an operation_id.
        """
        async with client() as api:
            return await api.sources(scoped(action, command))

    @server.tool()
    async def manage_bundles(action: str, command: JsonObject) -> JsonObject:
        """Export/preview/install/list/get/uninstall logical bundles in the bound subject.

        The command is the public BundleOperation request; install accepts the
        exported bundle object. Permissions and provenance are checked by Nous.
        """
        async with client() as api:
            return await api.bundles(scoped(action, command))

    @server.tool()
    async def manage_contexts(action: str, command: JsonObject) -> JsonValue:
        """Use public ContextsRequest operations in the bound subject.

        Includes open/read/change/close_session, residents/change_residents,
        create/read/list/change_work, work_history, prepare/preparation,
        cancel_preparation, admit/admission and report. Use expected_revision
        for concurrent changes; pin/unpin changes work anchors.
        """
        async with client() as api:
            return await api.contexts(scoped(action, command))

    @server.tool()
    async def manage_cognition(action: str, command: JsonObject) -> JsonValue:
        """Read/history/source, adopt/revise, query and evolve cognition in the bound subject.

        Use the public CognitionRequest action and request. Revisions require
        exact evidence and the expected current revision; model formation can
        invoke the configured provider.
        """
        request = scoped(action, command)
        async with client() as api:
            if action in {"get", "history", "source", "set_deleted"}:
                identity = command.get("id")
                if not isinstance(identity, str):
                    raise ValueError("Cognitive read requires id")
                record = await api.cognitive_object(identity)
                if record.namespace_id != namespace_id or record.subject_id != subject_id:
                    raise ValueError("Cognition is unavailable in the bound subject")
            return await api.cognition(request)

    @server.tool()
    async def manage_media(action: str, command: JsonObject) -> JsonValue:
        """Media lineage, regions, representations and derivation in the bound subject.

        Use the public MediaRequest action and request. Original file transfer
        uses the SDK/CLI streaming upload; derive operates on existing originals.
        """
        async with client() as api:
            if action == "inspect":
                material = command.get("material_id")
                if not isinstance(material, str):
                    raise ValueError("Media inspection requires material_id")
                # This scoped source read authorizes the material before returning
                # the SDK's globally addressed immutable metadata.
                await api.source(subject_id, material)
            return await api.media(scoped(action, command))

    @server.tool()
    async def manage_credentials(action: str, command: JsonObject) -> JsonValue:
        """Caller whoami/list/read/history/issue/rotate/revoke using public IdentityRequest.

        Management requires manage permission and cannot enlarge the caller's
        grants. Plaintext is returned only by a first successful issue/rotation;
        replay returns metadata. Store a newly returned token securely.
        """
        async with client() as api:
            return await api.identity({"action": action, "request": command})

    @server.tool()
    async def configuration(
        action: Literal["get", "effective", "set", "reset"], command: JsonObject
    ) -> JsonObject:
        """Read effective configuration or change an authorized scoped override.

        command has key, value, scope and operation_id as applicable. Model
        resources and instance-wide changes require unrestricted management.
        """
        async with client() as api:
            return await api.configuration(
                {
                    **command,
                    "action": action,
                    "namespace_id": namespace_id,
                    "subject_id": subject_id,
                }
            )

    @server.tool()
    async def prompts(
        action: Literal["list", "get", "history", "save", "activate", "preview"],
        command: JsonObject,
    ) -> JsonValue:
        """Read/preview prompt revisions; global save/activate needs instance management.

        command supplies id, template, variables, revision or operation_id.
        Preview renders a template without paying for a model invocation.
        """
        async with client() as api:
            return await api.prompts({**command, "action": action, "namespace_id": namespace_id})

    @server.tool()
    async def list_jobs(
        kind: str | None = None,
        state: str | None = None,
        limit: int = 50,
        cursor: str | None = None,
        scope: str | None = None,
    ) -> JsonObject:
        """Page actual durable jobs, filtering before the limit; reuse next_cursor.

        kind is the precise workflow name, state a DBOS status such as SUCCESS
        or CANCELLED. Results contain metadata, never workflow inputs/outputs.
        """
        async with client() as api:
            return (
                await api.job_page(kind=kind, state=state, limit=limit, cursor=cursor, scope=scope)
            ).model_dump(mode="json")

    @server.tool()
    async def cancel_job(job_id: str) -> JsonObject:
        """Cancel this subject's durable job and its owned work through DBOS."""
        async with client() as api:
            return await api.cancel_job(job_id)

    @server.tool()
    async def subject_diagnostics() -> JsonObject:
        """Read actual inventories, effective model space and readiness in the bound subject."""
        async with client() as api:
            return await api.subject_diagnostics(subject_id)

    @server.tool()
    async def backup_instance() -> JsonObject:
        """Create an online database/content backup; unrestricted management required.

        Returns the server archive filename, commit sequence and object count.
        Use the CLI/Web to download or restore its byte stream to a new instance.
        """
        async with client() as api:
            return await api.backup()
