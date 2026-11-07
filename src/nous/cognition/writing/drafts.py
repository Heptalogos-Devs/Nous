"""Joint writing, durable drafts and exact, retryable Journal adoption."""

import asyncio
from typing import Literal, cast
from uuid import uuid4
from weakref import WeakValueDictionary

import sqlalchemy as sa
from pydantic import TypeAdapter

from nous.common.errors import NousError
from nous.common.identifier import insert_identifier
from nous.common.operations import add_error_context, exception_context, record_completed
from nous.content.contracts import ContentReadView, InputGroup, InputPart, OutputContent, ResultRef
from nous.content.results import ResultStore
from nous.models.generation import GenerationOperations, ProducedTask
from nous.runtime.execution import ExecutionManager
from nous.storage.database import Database, ReadTx

from .. import schema
from ..contracts import RecordRevisionRef
from .contracts import (
    DraftContext,
    DraftRef,
    DraftView,
    NormalizedEdit,
    WritingOutcome,
    WritingRequest,
)
from .reading import RecordReader
from .records import WrittenRecords
from .tasks import writing_body


class DraftStore:
    """Save successful computations and frozen drafts without initiating language work."""

    def __init__(
        self,
        database: Database,
        execution: ExecutionManager,
        core: WrittenRecords,
        reader: RecordReader,
        generation: GenerationOperations,
        results: ResultStore,
    ):
        self.database, self.execution, self.core, self.reader = database, execution, core, reader
        self.generation, self.results = generation, results
        self._request_locks: WeakValueDictionary[str, asyncio.Lock] = WeakValueDictionary()

    async def save_request(self, request: WritingRequest) -> ResultRef:
        """Persist writing intent before any remote language request."""
        inputs = tuple(
            InputGroup(
                key=source.slot_key,
                slot_key=source.slot_key,
                role=source.role,
                name=source.display_name or source.slot_key,
                parts=tuple(InputPart(content=item) for item in source.selections),
            )
            for source in request.inputs.sources
        )
        result = await self.results.adopt(
            operation="writing.request",
            producer={"kind": "user"},
            inputs=inputs,
            outputs={
                "request": OutputContent(
                    data=request.model_dump_json().encode("utf-8"),
                    media_type="application/json",
                    display_name="writing-request.json",
                )
            },
            parameters={"computation_key": request.computation_key},
            dependencies=(request.task.descriptor,),
            producer_key=request.computation_key + ":request",
        )
        return result.ref

    async def read_request(self, ref: str | ResultRef) -> tuple[ResultRef, WritingRequest]:
        result = await self.results.get(ref)
        if result.operation != "writing.request":
            raise NousError("writing.request_required", "Recovery requires a writing request")
        artifact = next(item.artifact for item in result.outputs if item.name == "request")
        return result.ref, WritingRequest.model_validate_json(
            await self.results.artifacts.read_bytes(artifact)
        )

    async def find_request(self, ref: ResultRef) -> DraftView | None:
        async with self.database.read() as tx:
            row = (
                await tx.connection.execute(
                    sa.select(
                        schema.journal_drafts.c.identifier, schema.journal_drafts.c.namespace_id
                    ).where(schema.journal_drafts.c.request_result_id == ref.id)
                )
            ).one_or_none()
        return (
            await self.read(str(row.identifier), namespace=str(row.namespace_id))
            if row is not None
            else None
        )

    async def save_produced(
        self, produced: ProducedTask, context: DraftContext, request_ref: ResultRef
    ) -> WritingOutcome:
        lock = self._request_locks.setdefault(request_ref.id, asyncio.Lock())
        async with lock:
            existing = await self.find_request(request_ref)
            if existing is not None:
                return WritingOutcome(
                    status="draft",
                    draft_ref=existing.ref,
                    draft=existing,
                    stage_results=(*produced.completed_stage_refs, existing.result_ref.identifier),
                )
            return await self._save_produced(produced, context, request_ref)

    async def _save_produced(
        self, produced: ProducedTask, context: DraftContext, request_ref: ResultRef
    ) -> WritingOutcome:
        """Save a successful computation and its frozen draft without another model call.

        Output bytes are prepared before the result and draft descriptor are adopted
        together in a write transaction. An output without material returns
        ``no_material`` after saving its result. A saved request reference allows
        recovery after reopening the instance.
        """
        async with self.execution.operation("writing.save_draft"):
            published = None
            try:
                selected = await self.core.sources.prepare(
                    context.namespace.ref, context.selected_sources, preserve_keys=True
                )
                bindings = (
                    selected
                    if context.source_bindings == context.selected_sources
                    else await self.core.sources.prepare(
                        context.namespace.ref, context.source_bindings, preserve_keys=True
                    )
                )
                context = context.model_copy(
                    update={
                        "selected_sources": selected.bindings,
                        "source_bindings": bindings.bindings,
                    }
                )
                text = writing_body(context.intent, produced.outputs.get("output"))
                if text is None:
                    prepared = await self.generation.prepare_adoption(
                        produced, producer_key="writing:" + request_ref.id + ":no_material"
                    )
                    async with self.database.write() as tx:
                        result = await self.generation.adopt_in(tx, prepared)
                    return WritingOutcome(
                        status="no_material",
                        stage_results=(*produced.completed_stage_refs, result.ref.identifier),
                    )
                if context.intent == "edit":
                    edits = produced.outputs.get("edits")
                    if edits is None:
                        raise NousError(
                            "model.invalid_output", "Edit task returned no interpreted edits"
                        )
                    normalized = TypeAdapter(tuple[NormalizedEdit, ...]).validate_json(edits.data)
                    context = context.model_copy(update={"normalized_edits": normalized})
                # Readable model output survives citation errors; formal adoption validates it.
                unchanged = False
                if context.base_revision is not None:
                    base = await self.reader.read(
                        context.base_revision, namespace=context.namespace.id
                    )
                    unchanged = text == base.body and context.source_bindings == base.sources
                prepared = await self.generation.prepare_adoption(
                    produced, producer_key="writing:" + request_ref.id + ":draft-result"
                )
                published = prepared.outputs
                async with self.database.write() as tx:
                    ns = await self.reader.scope.get_in(tx, context.namespace.id)
                    await self.core.validate_sources_in(
                        tx, ns, (*context.selected_sources, *context.source_bindings)
                    )
                    result = await self.generation.adopt_in(tx, prepared)
                    body = next(
                        output.artifact for output in result.outputs if output.name == "output"
                    )
                    object_id = str(uuid4())
                    identifier = await insert_identifier(
                        tx,
                        schema.journal_drafts,
                        "drf",
                        {
                            "id": object_id,
                            "namespace_id": ns.id,
                            "result_id": result.ref.id,
                            "request_result_id": request_ref.id,
                            "body_artifact_id": body.id,
                            "context": context.model_dump(mode="json"),
                            "base_revision_id": context.base_revision.revision_id
                            if context.base_revision
                            else None,
                            "adopted_revision_id": None,
                            "adopted_as": None,
                            "recorded_at": self.results.artifacts.clock.utc_now().isoformat(),
                        },
                    )
                ref = DraftRef(
                    instance_id=self.results.artifacts.instance_id,
                    namespace_id=ns.id,
                    id=object_id,
                    identifier=identifier,
                )
                record_completed((result.ref.identifier, ref.identifier))
                draft = DraftView(
                    ref=ref,
                    result_ref=result.ref,
                    body_artifact=body,
                    body=text,
                    context=context,
                    diff=produced.outputs["diff"].data.decode("utf-8")
                    if "diff" in produced.outputs
                    else None,
                )
                return WritingOutcome(
                    status="unchanged" if unchanged else "draft",
                    draft_ref=ref,
                    draft=draft,
                    stage_results=(*produced.completed_stage_refs, result.ref.identifier),
                )
            except BaseException as error:
                add_error_context(
                    error,
                    {
                        "stage": "writing.save_draft",
                        "writing_request_ref": request_ref.model_dump(mode="json"),
                        "model_succeeded": True,
                        "raw_output": produced.outputs["output"].data.decode("utf-8")
                        if "output" in produced.outputs
                        else None,
                        "completed_result_refs": list(produced.completed_stage_refs),
                        "published_outputs": [
                            {
                                "name": output.name,
                                "blob": output.content.blob.model_dump(mode="json"),
                            }
                            for output in published.outputs
                        ]
                        if published is not None
                        else exception_context(error).get("published_outputs", []),
                    },
                )
                raise

    async def descriptor_in(
        self, tx: ReadTx, target: str | DraftRef, namespace: str | None = None
    ) -> tuple[
        DraftRef, DraftContext, str, str, str | None, Literal["generated", "override"] | None
    ]:
        """Resolve a saved draft descriptor within the caller's read view."""
        if isinstance(target, DraftRef):
            if target.instance_id != self.results.artifacts.instance_id:
                raise NousError("cognition.instance_mismatch", "Draft belongs to another instance")
            selected, target_id = namespace or target.namespace_id, target.id
        else:
            selected, target_id = (
                namespace,
                target,
            )
        ns = await self.reader.namespace_in(tx, selected)
        row = (
            (
                await tx.connection.execute(
                    sa.select(schema.journal_drafts).where(
                        sa.or_(
                            schema.journal_drafts.c.id == target_id,
                            schema.journal_drafts.c.identifier == target_id,
                        )
                    )
                )
            )
            .mappings()
            .one_or_none()
        )
        if row is None:
            raise NousError("cognition.draft_not_found", "Journal draft does not exist")
        if (
            row["namespace_id"] != ns.id
            or isinstance(target, DraftRef)
            and target.namespace_id != ns.id
        ):
            raise NousError(
                "cognition.namespace_mismatch", "Draft does not belong to the selected namespace"
            )
        return (
            DraftRef(
                instance_id=self.results.artifacts.instance_id,
                namespace_id=ns.id,
                id=str(row["id"]),
                identifier=str(row["identifier"]),
            ),
            DraftContext.model_validate(row["context"]),
            str(row["result_id"]),
            str(row["body_artifact_id"]),
            str(row["adopted_revision_id"]) if row["adopted_revision_id"] else None,
            cast(Literal["generated", "override"] | None, row["adopted_as"]),
        )

    async def revision_in(self, tx: ReadTx, revision_id: str) -> RecordRevisionRef:
        """Resolve a stored revision ID to its exact record revision in the caller's view."""
        row = (
            (
                await tx.connection.execute(
                    sa.select(
                        schema.records.c.identifier,
                        schema.records.c.namespace_id,
                        schema.revisions.c.ordinal,
                    )
                    .join(schema.revisions, schema.revisions.c.record_id == schema.records.c.id)
                    .where(schema.revisions.c.id == revision_id)
                )
            )
            .mappings()
            .one()
        )
        record, revision, _ = await self.reader.resolve_in(
            tx,
            str(row["identifier"]) + "@" + str(row["ordinal"]),
            namespace=str(row["namespace_id"]),
        )
        return self.reader.revision_ref(record, revision)

    async def read(
        self, target: str | DraftRef, *, namespace: str | None = None, sources: bool = False
    ) -> DraftView:
        """Read a saved draft and its frozen context by ID, identifier or reference.

        When ``sources`` is true, also materialize its source content views. An adopted
        revision, if present, identifies the formal result of adoption.
        """
        async with self.execution.operation("writing.read_draft"):
            async with self.database.read() as tx:
                ref, context, result_id, body_id, adopted, adopted_as = await self.descriptor_in(
                    tx, target, namespace
                )
                body = await self.results.artifacts.get_in(tx, body_id)
                revision = await self.revision_in(tx, adopted) if adopted else None
            result = await self.results.get(result_id)
            views: dict[str, tuple[ContentReadView, ...]] = {}
            if sources:
                selected_views = await self.results.artifacts.read_view_many(
                    [
                        selection
                        for binding in context.source_bindings
                        for selection in binding.selections
                    ]
                )
                offset = 0
                for binding in context.source_bindings:
                    views[binding.key] = selected_views[offset : offset + len(binding.selections)]
                    offset += len(binding.selections)
            return DraftView(
                ref=ref,
                result_ref=result.ref,
                body_artifact=body.ref,
                body=await self.results.artifacts.read_text(body.ref),
                context=context,
                adopted_revision=revision,
                adopted_as=adopted_as,
                diff=await self.results.artifacts.read_text(
                    next(output.artifact for output in result.outputs if output.name == "diff")
                )
                if any(output.name == "diff" for output in result.outputs)
                else None,
                source_views=views,
            )

    async def status(self) -> dict[str, object]:
        """Count saved and unadopted drafts across the instance."""
        async with self.execution.operation("writing.draft_status"):
            async with self.database.read() as tx:
                total = await tx.connection.scalar(
                    sa.select(sa.func.count()).select_from(schema.journal_drafts)
                )
                pending = await tx.connection.scalar(
                    sa.select(sa.func.count())
                    .select_from(schema.journal_drafts)
                    .where(schema.journal_drafts.c.adopted_revision_id.is_(None))
                )
                return {"drafts": total, "unadopted": pending}
