"""MCP management tools delegate to the same public SDK operation owners."""

import base64
from collections.abc import Callable
from contextlib import AbstractAsyncContextManager
from typing import Literal

from nous_sdk import NousClient
from nous_sdk.contracts import CognitiveRef, SourceCommand
from nous_sdk.contracts.corrections import ApplyCorrection, PlanCorrection, RollbackCorrection
from nous_sdk.contracts.feedback import (
    EnqueueFeedback,
    FeedbackQuery,
    RetryFeedback,
    RollbackFeedback,
)
from nous_sdk.contracts.identity import (
    BindSubject,
    ChangeNamespace,
    IdentityDirectoryQuery,
    IdentityPurpose,
    PurgeNamespace,
)
from nous_sdk.contracts.management import SourceSelection
from nous_sdk.contracts.model_roles import ModelContractRequest, ModelPreviewRequest
from nous_sdk.contracts.namespace_archives import RestoreNamespace
from nous_sdk.contracts.namespace_backups import RestoreNamespaceBackup
from nous_sdk.contracts.records import ReadRecords, RecordPurpose, RecordsQuery
from nous_sdk.contracts.resources import ChangeResource, ReadResource, RegisterResource
from nous_sdk.contracts.retention import DecayRetention, RetentionChange, RetentionChanges
from nous_sdk.contracts.tuning import ApplyRetrievalProfile, CreateTuning, RollbackRetrievalProfile
from nous_sdk.contracts.vector_projection import ProjectVectors
from nous_sdk.contracts.vector_spaces import (
    CheckVectorSpaces,
    DeleteVectorSpace,
    RebuildVectors,
    RecoverVectors,
    RetryVectorProjection,
)
from nous_sdk.types import JsonObject
from pydantic import JsonValue

from mcp.server import MCPServer


def register_management(
    server: MCPServer,
    client: Callable[[], AbstractAsyncContextManager[NousClient]],
    namespace_id: str,
    
) -> None:
    @server.tool()
    async def decay_retention(command: DecayRetention) -> JsonObject:
        """Evaluate a cursor-paged Memory/Association selection with the common decay rules."""
        async with client() as api:
            return (await api.decay_retention(namespace_id,  command)).model_dump(mode="json")

    @server.tool()
    async def change_retention(command: RetentionChange) -> JsonObject:
        """Apply the shared retention rules to an exact Memory or Association revision."""
        async with client() as api:
            return (await api.change_retention(namespace_id,  command)).model_dump(mode="json")

    @server.tool()
    async def change_retentions(command: RetentionChanges) -> JsonObject:
        """Change a frozen lifecycle selection atomically."""
        async with client() as api:
            return {"records": [record.model_dump(mode="json") for record in await api.change_retentions(namespace_id,  command)]}

    @server.tool()
    async def retention_snapshot(reference: CognitiveRef) -> JsonObject:
        """Read the actual retention state and authoritative object revision."""
        async with client() as api:
            return (await api.retention_snapshot(namespace_id,  reference)).model_dump(mode="json")

    @server.tool()
    async def recover_vectors(command: RecoverVectors) -> JsonObject:
        """Resume original failed projections and backfill current missing vectors, preserving unknown outcomes."""
        async with client() as api:
            return (await api.recover_vectors(namespace_id,  command)).model_dump(
                mode="json"
            )

    @server.tool()
    async def vector_spaces(
        namespace_scope: bool = False, include_media: bool = False
    ) -> JsonObject:
        """Read configured fingerprints and stored vector spaces in the authorized scope."""
        async with client() as api:
            return (
                await api.vector_spaces(
                    namespace_id,
                    
                    include_media=include_media,
                )
            ).model_dump(mode="json")

    @server.tool()
    async def delete_vector_space(command: DeleteVectorSpace) -> JsonObject:
        """Delete derived vector storage in the selected Namespace."""
        async with client() as api:
            return (await api.delete_vector_space(namespace_id, command)).model_dump(mode="json")

    @server.tool()
    async def rebuild_vectors(command: RebuildVectors) -> JsonObject:
        """Rebuild current model-space projections through the durable embedding queue."""
        async with client() as api:
            return (await api.rebuild_vectors(namespace_id,  command)).model_dump(
                mode="json"
            )

    @server.tool()
    async def vector_projection_progress(request_id: str) -> JsonObject:
        """Read live DBOS state and prepared/adopted vector counts."""
        async with client() as api:
            return (
                await api.vector_projection_progress(namespace_id,  request_id)
            ).model_dump(mode="json")

    @server.tool()
    async def retry_vector_projection(command: RetryVectorProjection) -> JsonObject:
        """Retry the saved projection for its original targets."""
        async with client() as api:
            return (
                await api.retry_vector_projection(namespace_id,  command)
            ).model_dump(mode="json")

    @server.tool()
    async def check_vector_spaces(command: CheckVectorSpaces) -> JsonObject:
        """Check storage/configuration; provider probing requires actual input and probe=true."""
        async with client() as api:
            return (await api.check_vector_spaces(namespace_id,  command)).model_dump(
                mode="json"
            )

    @server.tool()
    async def preview_source_selection(
        selection: SourceSelection, action: Literal["trash", "restore", "purge"] = "trash"
    ) -> JsonObject:
        """Preview source names, observations and cognitive selections under current permissions."""
        async with client() as api:
            return (
                await api.preview_sources(
                    namespace_id,  selection=selection, action=action
                )
            ).model_dump(mode="json")

    @server.tool()
    async def change_source_selection(
        action: Literal["trash", "restore", "purge"], command: SourceCommand
    ) -> JsonObject:
        """Change an authorized range through canonical lifecycle and operation records."""
        async with client() as api:
            if action == "trash":
                result = await api.trash_sources(namespace_id,  command)
            elif action == "restore":
                result = await api.restore_sources(namespace_id,  command)
            else:
                result = await api.purge_sources(namespace_id,  command)
            return result.model_dump(mode="json")

    @server.tool()
    async def retry_feedback(command: RetryFeedback) -> JsonObject:
        """Retry a failed feedback task from its saved request and evidence."""
        async with client() as api:
            return (await api.retry_feedback(namespace_id,  command)).model_dump(
                mode="json"
            )

    @server.tool()
    async def create_retrieval_tuning(command: CreateTuning) -> JsonObject:
        """Run bounded parameter trials using supplied queries and relevance judgments."""
        async with client() as api:
            return (await api.create_tuning(namespace_id,  command)).model_dump(
                mode="json"
            )

    @server.tool()
    async def retrieval_tuning_tasks(limit: int = 50, cursor: str | None = None) -> JsonObject:
        """Page this caller's tuning tasks and their actual durable job states."""
        async with client() as api:
            return (
                await api.tuning_tasks(namespace_id,  limit=limit, cursor=cursor)
            ).model_dump(mode="json")

    @server.tool()
    async def retrieval_tuning_task(task_id: str) -> JsonObject:
        """Read the selected profile, development results and independent holdout results."""
        async with client() as api:
            return (await api.tuning_task(namespace_id,  task_id)).model_dump(
                mode="json"
            )

    @server.tool()
    async def retrieval_tuning_rounds(
        task_id: str, limit: int = 100, after: int | None = None
    ) -> JsonObject:
        """Read each measured candidate and its individual query rankings."""
        async with client() as api:
            return (
                await api.tuning_rounds(namespace_id,  task_id, limit=limit, after=after)
            ).model_dump(mode="json")

    @server.tool()
    async def cancel_retrieval_tuning(task_id: str) -> JsonObject:
        """Cancel the durable tuning job and retain completed measurements."""
        async with client() as api:
            return (await api.cancel_tuning(namespace_id,  task_id)).model_dump(
                mode="json"
            )

    @server.tool()
    async def retrieval_profile() -> JsonObject:
        """Read the effective retrieval profile and its configuration revision."""
        async with client() as api:
            return await api.retrieval_profile(namespace_id)

    @server.tool()
    async def apply_retrieval_profile(command: ApplyRetrievalProfile) -> JsonObject:
        """Publish a chosen profile or a completed tuning winner as a new Subject configuration."""
        async with client() as api:
            return await api.apply_retrieval_profile(namespace_id,  command)

    @server.tool()
    async def rollback_retrieval_profile(command: RollbackRetrievalProfile) -> JsonObject:
        """Reset the Namespace profile override to its inherited configuration."""
        async with client() as api:
            return await api.rollback_retrieval_profile(namespace_id,  command)

    @server.tool()
    async def enqueue_feedback(command: EnqueueFeedback) -> JsonObject:
        """Schedule feedback evaluation from subsequent real contacts in this session."""
        async with client() as api:
            return (await api.enqueue_feedback(namespace_id,  command)).model_dump(
                mode="json"
            )

    @server.tool()
    async def feedback_task(task_id: str) -> JsonObject:
        """Read a saved feedback decision and its correction result."""
        async with client() as api:
            return (await api.feedback_task(namespace_id,  task_id)).model_dump(
                mode="json"
            )

    @server.tool()
    async def feedback_tasks(command: FeedbackQuery) -> JsonObject:
        """Page native feedback tasks."""
        async with client() as api:
            return (await api.feedback_tasks(namespace_id,  command)).model_dump(
                mode="json"
            )

    @server.tool()
    async def rollback_feedback(command: RollbackFeedback) -> JsonObject:
        """Roll back the applied correction through its saved native plan."""
        async with client() as api:
            return (await api.rollback_feedback(namespace_id,  command)).model_dump(
                mode="json"
            )

    @server.tool()
    async def submit_source_trash(command: SourceCommand) -> JsonObject:
        """Retire selected source observations in a durable job; returns its job_id."""
        async with client() as api:
            return {"job_id": await api.submit_source_trash(namespace_id,  command)}

    @server.tool()
    async def purge_namespace(command: PurgeNamespace) -> JsonObject:
        """Permanently remove this previously trashed Namespace and its owned data."""
        async with client() as api:
            return (await api.purge_namespace(namespace_id, command)).model_dump(mode="json")

    @server.tool()
    async def create_namespace_backup() -> JsonObject:
        """Create a complete Namespace snapshot, including managed originals and paused work."""
        async with client() as api:
            return (await api.create_namespace_backup(namespace_id)).model_dump(mode="json")

    @server.tool()
    async def namespace_backups(limit: int = 100, cursor: str | None = None) -> JsonObject:
        """Page complete backups for this Namespace."""
        async with client() as api:
            return (
                await api.namespace_backups(namespace_id, limit=limit, cursor=cursor)
            ).model_dump(mode="json")

    @server.tool()
    async def namespace_backup(backup_id: str) -> JsonObject:
        """Read a permitted backup's manifest metadata."""
        async with client() as api:
            return (await api.namespace_backup(backup_id)).model_dump(mode="json")

    @server.tool()
    async def delete_namespace_backup(backup_id: str) -> dict[str, bool]:
        """Delete a permitted complete backup."""
        async with client() as api:
            await api.delete_namespace_backup(backup_id)
            return {"deleted": True}

    @server.tool()
    async def download_namespace_backup(
        backup_id: str, offset: int = 0, max_bytes: int = 262144
    ) -> JsonObject:
        """Read one bounded base64 chunk with its resumable next_offset."""
        async with client() as api:
            return (
                await api.download_namespace_backup(backup_id, offset=offset, max_bytes=max_bytes)
            ).model_dump(mode="json")

    @server.tool()
    async def begin_namespace_backup_upload() -> JsonObject:
        """Begin a backup upload bound to the authenticated manager."""
        async with client() as api:
            return (await api.begin_namespace_backup_upload()).model_dump(mode="json")

    @server.tool()
    async def upload_namespace_backup_chunk(
        upload_id: str, offset: int, data_base64: str
    ) -> JsonObject:
        """Append at the exact acknowledged offset; chunks are at most 1 MiB."""
        async with client() as api:
            return (
                await api.upload_namespace_backup_chunk(
                    upload_id, offset, base64.b64decode(data_base64, validate=True)
                )
            ).model_dump(mode="json")

    @server.tool()
    async def complete_namespace_backup_upload(
        upload_id: str, expected_sha256: str = ""
    ) -> JsonObject:
        """Validate the complete ZIP manifest and hashes before publishing the backup."""
        async with client() as api:
            return (
                await api.complete_namespace_backup_upload(upload_id, expected_sha256)
            ).model_dump(mode="json")

    @server.tool()
    async def abort_namespace_backup_upload(upload_id: str) -> dict[str, bool]:
        """Discard this caller's unfinished upload."""
        async with client() as api:
            await api.abort_namespace_backup_upload(upload_id)
            return {"aborted": True}

    @server.tool()
    async def restore_namespace_backup(
        target_namespace_id: str, command: RestoreNamespaceBackup
    ) -> JsonObject:
        """Restore into a new Namespace; copy mode allocates new identities and pauses pending work."""
        async with client() as api:
            return (await api.restore_namespace_backup(target_namespace_id, command)).model_dump(
                mode="json"
            )

    @server.tool()
    async def export_namespace_archive() -> JsonObject:
        """Export retained semantic Subjects and namespace identity/configuration metadata."""
        async with client() as api:
            return (await api.export_namespace_archive(namespace_id)).model_dump(mode="json")

    @server.tool()
    async def restore_namespace_archive(
        target_namespace_id: str, command: RestoreNamespace
    ) -> JsonObject:
        """Restore a semantic namespace archive into a new namespace; instance management required."""
        async with client() as api:
            return (await api.restore_namespace_archive(target_namespace_id, command)).model_dump(
                mode="json"
            )


    @server.tool()
    async def namespace_directory(command: IdentityDirectoryQuery) -> JsonObject:
        """Page permitted Namespace identities; management purpose includes inactive/recycled entries."""
        async with client() as api:
            return (await api.namespace_page(command)).model_dump(mode="json")


    @server.tool()
    async def namespace_record(purpose: IdentityPurpose = "active") -> JsonObject:
        """Read this Namespace; management purpose requires manage authority."""
        async with client() as api:
            return (await api.get_namespace(namespace_id, purpose=purpose)).model_dump(mode="json")


    @server.tool()
    async def change_namespace(command: ChangeNamespace) -> JsonObject:
        """Rename, enable, disable, trash, restore or configure the trusted memory Subject binding."""
        async with client() as api:
            return (await api.change_namespace(namespace_id, command)).model_dump(mode="json")


    @server.tool()
    async def identity_capabilities(purpose: IdentityPurpose = "active") -> JsonObject:
        """Distinguish supported, configured and currently enabled Subject domains."""
        async with client() as api:
            return (
                await api.identity_capabilities(namespace_id,  purpose=purpose)
            ).model_dump(mode="json")

    @server.tool()
    async def bind_subject(command: BindSubject) -> JsonObject:
        """Bind an external host identity under the authenticated caller; repeated use retains its Subject."""
        async with client() as api:
            return (await api.bind_subject(namespace_id, command)).model_dump(mode="json")

    @server.tool()
    async def subject_binding(external_id: str, purpose: IdentityPurpose = "active") -> JsonObject:
        """Read an existing binding owned by this authenticated host identity."""
        async with client() as api:
            return (
                await api.subject_binding(namespace_id, external_id, purpose=purpose)
            ).model_dump(mode="json")

    from .episodes import register_episodes

    register_episodes(server, client, namespace_id)
    from .facts import register_facts

    register_facts(server, client, namespace_id)

    @server.tool()
    async def register_resource(command: RegisterResource) -> JsonObject:
        """Register an operator-selected file root or HTTP document; management permission required."""
        async with client() as api:
            return (await api.register_resource(namespace_id,  command)).model_dump(
                mode="json"
            )

    @server.tool()
    async def list_resources(cursor: str | None = None, limit: int = 100) -> JsonObject:
        """List accessible external resources and their latest saved source versions."""
        async with client() as api:
            return (
                await api.resources(namespace_id,  cursor=cursor, limit=limit)
            ).model_dump(mode="json")

    @server.tool()
    async def read_resource(command: ReadResource) -> JsonObject:
        """Read a saved snapshot or explicitly check its current external version."""
        async with client() as api:
            return (await api.read_resource(namespace_id,  command)).model_dump(
                mode="json"
            )

    @server.tool()
    async def change_resource(command: ChangeResource) -> JsonObject:
        """Change resource metadata or enablement at its expected revision; management required."""
        async with client() as api:
            return (await api.change_resource(namespace_id,  command)).model_dump(
                mode="json"
            )

    from .ego_social import register_ego_social

    register_ego_social(server, client, namespace_id)

    @server.tool()
    async def plan_correction(command: PlanCorrection) -> JsonObject:
        """Prepare a source-backed correction preview for the bound subject; requires management."""
        async with client() as api:
            return (await api.plan_correction(namespace_id,  command)).model_dump(
                mode="json"
            )

    @server.tool()
    async def apply_correction(command: ApplyCorrection) -> JsonObject:
        """Apply a saved plan atomically if all targets still match their proposed revisions."""
        async with client() as api:
            return (await api.apply_correction(namespace_id,  command)).model_dump(
                mode="json"
            )

    @server.tool()
    async def rollback_correction(command: RollbackCorrection) -> JsonObject:
        """Restore this plan's changes as new revisions and report later conflicting changes."""
        async with client() as api:
            return (await api.rollback_correction(namespace_id,  command)).model_dump(
                mode="json"
            )

    @server.tool()
    async def correction_plan(plan_id: str) -> JsonObject:
        """Read the saved correction preview, its adopted revisions and rollback results."""
        async with client() as api:
            return (await api.correction_plan(namespace_id,  plan_id)).model_dump(
                mode="json"
            )

    @server.tool()
    async def list_records(command: RecordsQuery) -> JsonObject:
        """Page complete cognitive records with stable cursors, filters and source references.

        management purpose requires a management credential. Reuse the same
        filters with next_cursor to continue; history selects past revisions.
        """
        if command.namespace_id not in (None, namespace_id):
            raise ValueError("This MCP connection is bound to its configured subject")
        async with client() as api:
            return (await api.records(namespace_id,  command)).model_dump(mode="json")

    @server.tool()
    async def read_records(command: ReadRecords) -> list[JsonObject]:
        """Read exact cognitive revisions within the bound subject and current source permissions."""
        async with client() as api:
            return [
                record.model_dump(mode="json")
                for record in await api.read_records(namespace_id,  command)
            ]

    @server.tool()
    async def record_detail(
        reference: CognitiveRef, history_limit: int = 100, purpose: RecordPurpose = "exact"
    ) -> JsonObject:
        """Read an exact object, its current reference and available revision history."""
        async with client() as api:
            return (
                await api.record_detail(
                    namespace_id,
                    
                    reference,
                    history_limit=history_limit,
                    purpose=purpose,
                )
            ).model_dump(mode="json")

    @server.tool()
    async def project_vectors(command: ProjectVectors) -> JsonObject:
        """Admit exact targets to the durable embedding queue; inspect or wait for the returned job."""
        async with client() as api:
            return (await api.project_vectors(namespace_id,  command)).model_dump(
                mode="json"
            )

    @server.tool()
    async def model_contract(command: ModelContractRequest) -> JsonObject:
        """Read a registered role's real input/output schema and active Prompt without a provider call."""
        async with client() as api:
            return (
                await api.model_contract(
                    {
                        **command.model_dump(mode="json"),
                        "namespace_id": namespace_id,
                        
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
                        
                    }
                )
            ).model_dump(mode="json")

    def scoped(action: str, command: JsonObject) -> JsonObject:
        for name, value in (("namespace_id", namespace_id), ('namespace_id', namespace_id)):
            if command.get(name) not in (None, value):
                raise ValueError(
                    "This MCP connection is bound to its configured namespace and subject"
                )
        return {
            "action": action,
            "request": {**command, "namespace_id": namespace_id},
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
    async def manage_cognition(
        action: Literal[
            "create",
            "revise",
            "adopt",
            "concept_resolve",
            "form",
            "merge_concepts",
            "split_concept",
            "resegment_episodes",
        ],
        command: JsonObject,
    ) -> JsonValue:
        """Adopt, revise or evolve cognition using exact source-backed operations."""
        request = scoped(action, command)
        async with client() as api:
            return await api.cognition(request)

    @server.tool()
    async def manage_media(action: str, command: JsonObject) -> JsonValue:
        """Media lineage, regions, representations, derivation and embedding in the bound subject.

        Use the public MediaRequest action and request. Original file transfer
        uses the SDK/CLI streaming upload; derive operates on existing originals.
        embed takes an exact reference and operation_id and uses the configured model.
        """
        async with client() as api:
            if action == "inspect":
                material = command.get("material_id")
                if not isinstance(material, str):
                    raise ValueError("Media inspection requires material_id")
                namespace = await api.namespace_by_id(namespace_id)
                await namespace.materials.get(material)
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
        action: Literal["list", "schema", "get", "effective", "set", "reset"],
        command: JsonObject,
    ) -> JsonObject:
        """Read registered keys and effective values or change a scoped override.

        command has key, value, scope and operation_id as applicable. Model
        resources and instance-wide changes require unrestricted management.
        """
        async with client() as api:
            return await api.configuration(
                {
                    **command,
                    "action": action,
                    "namespace_id": namespace_id,
                    
                }
            )

    @server.tool()
    async def prompts(
        action: Literal["list", "get", "history", "save", "activate", "preview"],
        command: JsonObject,
    ) -> JsonValue:
        """Read/preview prompt revisions; global save/activate needs instance management.

        command supplies id, template, variables, revision or operation_id.
        Preview renders a template without contacting a provider.
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
        purpose: Literal["actor", "management"] = "actor",
    ) -> JsonObject:
        """Page actual durable jobs, filtering before the limit; reuse next_cursor.

        kind is the precise workflow name, state a DBOS status such as SUCCESS
        or CANCELLED. Results contain metadata, never workflow inputs/outputs.
        """
        async with client() as api:
            return (
                await api.job_page(
                    kind=kind, state=state, limit=limit, cursor=cursor, scope=scope, purpose=purpose
                )
            ).model_dump(mode="json")

    @server.tool()
    async def cancel_job(
        job_id: str, purpose: Literal["actor", "management"] = "actor"
    ) -> JsonObject:
        """Cancel this subject's durable job and its owned work through DBOS."""
        async with client() as api:
            return await api.cancel_job(job_id, purpose=purpose)

    from nous_sdk.contracts.graph_management import GraphReadQuery, GraphSearchQuery
    from nous_sdk.contracts.person_profiles import PersonProfilesQuery

    @server.tool()
    async def read_graph(command: GraphReadQuery) -> JsonObject:
        """Read a canonical graph or a selected Entity/edge neighborhood and its source records."""
        async with client() as api:
            return (await api.read_graph(namespace_id,  command)).model_dump(mode="json")

    @server.tool()
    async def search_graph(command: GraphSearchQuery) -> JsonObject:
        """Search authorized native Entity and Association records with exact references."""
        async with client() as api:
            return (await api.search_graph(namespace_id,  command)).model_dump(
                mode="json"
            )

    @server.tool()
    async def person_profiles(command: PersonProfilesQuery) -> JsonObject:
        """Read native Person profiles and source-backed confirmed or uncertain evidence."""
        async with client() as api:
            return (await api.person_profiles(namespace_id,  command)).model_dump(
                mode="json"
            )

    @server.tool()
    async def subject_diagnostics() -> JsonObject:
        """Read actual inventories, effective model space and readiness in the bound subject."""
        async with client() as api:
            return (await api.subject_diagnostics(namespace_id)).model_dump(mode="json")

    @server.tool()
    async def backup_instance() -> JsonObject:
        """Create an online database/content backup; unrestricted management required.

        Returns the server archive filename, commit sequence and object count.
        Use the CLI/Web to download or restore its byte stream to a new instance.
        """
        async with client() as api:
            return await api.backup()
