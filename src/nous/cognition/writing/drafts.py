"""Joint writing, durable drafts and exact, retryable Journal adoption."""

from dataclasses import dataclass
from typing import Literal, cast
from uuid import uuid4

import sqlalchemy as sa
from pydantic import TypeAdapter

from nous.common.errors import NousError
from nous.common.identifier import insert_identifier
from nous.common.operations import add_error_context, exception_context, record_completed
from nous.content.contracts import ContentReadView
from nous.content.results import ResultStore
from nous.models.operations import GenerationOperations, ProducedTask
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
)
from .reading import RecordReader
from .records import WrittenRecords
from .tasks import writing_body


@dataclass(frozen=True)
class PendingDraft:
    """Successful model computation and frozen meaning, retained for a SQL-only retry."""

    produced: ProducedTask
    context: DraftContext


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

    async def save_produced(self, produced: ProducedTask, context: DraftContext) -> WritingOutcome:
        """Save a successful computation and its frozen draft without another model call.

        Output bytes are prepared before the result and draft descriptor are adopted
        together in a write transaction. An output without material returns
        ``no_material`` after saving its result. On save failure the exception carries
        a ``PendingDraft`` in ``nous_recovery``; pass its produced result and context
        to this method to retry saving without requesting the model again.
        """
        async with self.execution.operation("writing.save_draft"):
            published = None
            try:
                text = writing_body(context.intent, produced.outputs.get("output"))
                if text is None:
                    result = await self.generation.adopt_produced(produced)
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
                published = await self.results.prepare_outputs(produced.outputs)
                async with self.database.write() as tx:
                    ns = await self.reader.scope.get_in(tx, context.namespace.id)
                    await self.core.validate_sources_in(
                        tx, ns, (*context.selected_sources, *context.source_bindings)
                    )
                    result = await self.results.adopt_in(
                        tx,
                        operation=produced.operation,
                        producer=produced.producer,
                        inputs=produced.actual_inputs,
                        prepared_outputs=published,
                        request=produced.request,
                        parameters=produced.parameters,
                        coverage=produced.coverage,
                    )
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
                    recovery=PendingDraft(produced, context),
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
            selected, target_id = self.reader.namespace(namespace or target.namespace_id), target.id
        else:
            selected, target_id = (
                self.reader.namespace(namespace),
                target,
            )
        ns = await self.reader.scope.get_in(tx, selected)
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
            tx, str(row["identifier"]) + "@" + str(row["ordinal"]), namespace=str(row["namespace_id"])
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
