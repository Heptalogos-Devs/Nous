"""Joint writing, durable drafts and exact, retryable Journal adoption."""

from collections.abc import Callable, Sequence
from dataclasses import dataclass
from typing import Literal, TypedDict
from uuid import uuid4

import sqlalchemy as sa

from nous.common.address import insert_address
from nous.common.errors import NousError
from nous.common.operations import add_error_context, exception_context, record_completed
from nous.content.contracts import ArtifactRef, ContentSelection, InputSpec
from nous.content.results import ResultStore
from nous.models.operations import GenerationOperations, ProducedTask
from nous.models.tasks import JournalDraftTask
from nous.runtime.execution import ExecutionManager
from nous.storage.database import Database, ReadTx
from nous.temporal.values import CognitiveTimes

from . import schema
from .contracts import (
    DraftContext,
    DraftRef,
    DraftView,
    JournalizeOutcome,
    Namespace,
    RecordRef,
    RecordRevisionRef,
    SavedRecord,
    SourceBinding,
)
from .records import Target, WrittenRecords
from .retrieval import Mode

SourceInput = Target | SourceBinding | ContentSelection | ArtifactRef


class AdoptionMetadata(TypedDict):
    title: str | None
    purpose: str
    author: str | None
    viewpoint: str | None
    sources: Sequence[SourceBinding]
    times: CognitiveTimes
    origin_result_id: str
    body_artifact: ArtifactRef | None


@dataclass(frozen=True)
class PendingDraft:
    """Successful model computation and frozen meaning, retained for a SQL-only retry."""

    produced: ProducedTask
    context: DraftContext


class JournalizeOperations:
    def __init__(
        self,
        database: Database,
        execution: ExecutionManager,
        records: WrittenRecords,
        generation: GenerationOperations,
        results: ResultStore,
    ):
        self.database, self.execution, self.records, self.generation, self.results = (
            database,
            execution,
            records,
            generation,
            results,
        )

    async def _freeze(
        self, namespace: str, sources: Sequence[SourceInput], target: Target | None
    ) -> tuple[
        Namespace, tuple[SourceBinding, ...], RecordRevisionRef | None, ContentSelection | None
    ]:
        async with self.database.read() as tx:
            ns = await self.records.scope.get_in(tx, namespace)
            bindings: list[SourceBinding] = []
            for index, source in enumerate(sources):
                key = f"S{index + 1}"
                if isinstance(source, SourceBinding):
                    binding = source.model_copy(update={"key": key})
                elif isinstance(source, ContentSelection):
                    binding = SourceBinding(key=key, content_selection=source)
                elif (
                    isinstance(source, ArtifactRef)
                    or isinstance(source, str)
                    and source.startswith("art:")
                ):
                    artifact = await self.results.artifacts.get_in(tx, source)
                    binding = SourceBinding(
                        key=key,
                        content_selection=ContentSelection(artifact=artifact.ref),
                        name=artifact.display_name,
                    )
                else:
                    row, revision, _ = await self.records.resolve_in(tx, source, ns.id)
                    artifact = await self.results.artifacts.get_in(tx, revision["body_artifact_id"])
                    binding = SourceBinding(
                        key=key,
                        content_selection=ContentSelection(artifact=artifact.ref),
                        owner_revision=self.records.revision_ref(row, revision),
                        name=revision["title"],
                    )
                bindings.append(binding)
            await self.records.validate_sources_in(tx, ns, bindings)
            base, base_content = None, None
            if target is not None:
                if isinstance(target, RecordRef) or isinstance(target, str) and "@" not in target:
                    raise NousError(
                        "input.expected_required",
                        "Writing a revision requires an exact target with @N",
                    )
                row, revision, _ = await self.records.resolve_in(tx, target, ns.id)
                if row["kind"] != "journal":
                    raise NousError("input.invalid_kind", "Journalize target must be a Journal")
                base = self.records.revision_ref(row, revision)
                base_content = ContentSelection(
                    artifact=(
                        await self.results.artifacts.get_in(tx, revision["body_artifact_id"])
                    ).ref
                )
            return ns, tuple(bindings), base, base_content

    @staticmethod
    def _actual_bindings(
        produced: ProducedTask, selected: Sequence[SourceBinding]
    ) -> tuple[SourceBinding, ...]:
        """Successful-route keys bind to exact source coordinates, with all reading retained by ResultStore."""
        source_map = {source.key: source for source in selected}
        output: list[SourceBinding] = []
        for group in produced.actual_inputs:
            if group.role == "revision_base":
                continue
            source = source_map.get(group.key.split(":", 1)[0])
            if source is None:
                raise NousError(
                    "cognition.source_mismatch",
                    "Successful input has no frozen source",
                    details={"key": group.key},
                )
            selected_content = source.content_selection
            coordinates = tuple(
                dict.fromkeys(
                    content
                    for part in group.parts
                    for content in (part.content, *(origin.origin for origin in part.origins))
                    if content.artifact.id == selected_content.artifact.id
                )
            )
            # A joint representation may contain several pieces of the same selected range.
            # The exact pieces remain in ProcessingResult; a citation to that group covers the selected range.
            content = coordinates[0] if len(coordinates) == 1 else selected_content
            output.append(
                source.model_copy(update={"key": group.key, "content_selection": content})
            )
        return tuple(output)

    async def run(
        self,
        namespace: str | None,
        sources: Sequence[SourceInput],
        *,
        topic: str | None = None,
        purpose: str = "record",
        viewpoint: str | None = None,
        author: str | None = None,
        times: CognitiveTimes | None = None,
        target_revision: Target | None = None,
        adoption: Literal["draft", "save"] | None = None,
        on_delta: Callable[[str], None] | None = None,
        refresh: bool = False,
        refresh_mode: Mode | None = None,
    ) -> JournalizeOutcome:
        async with self.execution.operation("journalize"):
            snapshot = self.execution.snapshot
            settings = snapshot.typed_values()
            selected_namespace = namespace or settings.cognition.default_namespace
            ns, selected, base, base_content = await self._freeze(
                selected_namespace, sources, target_revision
            )
            frozen_times = await self.records.prepare_times(times)
            task = settings.cognition.journalize.task
            policy = adoption or settings.cognition.journalize.adoption
            if (refresh or refresh_mode is not None) and policy != "save":
                raise NousError(
                    "input.refresh_requires_save", "Use save adoption when requesting refresh"
                )
            if not selected:
                return JournalizeOutcome(status="no_material")
            inputs = [
                InputSpec(
                    kind="artifact",
                    artifact=source.content_selection.artifact,
                    selection=source.content_selection.selection,
                    role=source.usage,
                )
                for source in selected
            ]
            if base_content is not None:
                inputs.append(
                    InputSpec(
                        kind="artifact",
                        artifact=base_content.artifact,
                        selection=base_content.selection,
                        role="revision_base",
                    )
                )
            instructions = "\n".join(
                line
                for line in (
                    f"Purpose: {purpose}",
                    f"Author: {author}" if author else "",
                    f"Viewpoint: {viewpoint}" if viewpoint else "",
                    "Revise the supplied revision_base using the selected materials."
                    if base
                    else "",
                )
                if line
            )
            prepared = await self.generation.prepare_input(
                task, inputs, snapshot, question=topic or "", instructions=instructions
            )
            if len(prepared.batches) != 1:
                raise NousError(
                    "cognition.joint_required",
                    "Select one topic and fewer sources for a single Journal",
                )
            produced = await self.generation.produce(
                prepared.frozen,
                prepared.batches[0],
                snapshot,
                question=prepared.question,
                instructions=prepared.instructions,
                completed=list(prepared.preparations),
                on_delta=on_delta,
            )
            context = DraftContext(
                namespace=ns,
                selected_sources=selected,
                source_bindings=self._actual_bindings(produced, selected),
                target_revision=base,
                topic=topic,
                purpose=purpose,
                author=author,
                viewpoint=viewpoint,
                times=frozen_times,
                config_version=snapshot.version,
                task=task,
            )
            outcome = await self.save_produced(produced, context)
            if outcome.status == "no_material":
                return outcome
            draft = outcome.draft
            assert draft is not None
            if policy == "save":
                record = await self.adopt_draft(
                    draft.ref, refresh=refresh, refresh_mode=refresh_mode
                )
                draft = draft.model_copy(update={"adopted_revision": record.revision_ref})
                return JournalizeOutcome(
                    status="adopted",
                    draft_ref=draft.ref,
                    draft=draft,
                    record=record,
                    stage_results=outcome.stage_results,
                )
            return outcome

    async def save_produced(
        self, produced: ProducedTask, context: DraftContext
    ) -> JournalizeOutcome:
        """Commit the successful computation and descriptor together, without any model request."""
        async with self.execution.operation("journalize.save_draft"):
            published = None
            try:
                text = JournalDraftTask.body(produced.outputs.get("output"))
                if text is None:
                    result = await self.generation.adopt_produced(produced)
                    return JournalizeOutcome(
                        status="no_material",
                        stage_results=(*produced.completed_stage_refs, result.ref.address),
                    )
                published = await self.results.prepare_outputs(produced.outputs)
                async with self.database.write() as tx:
                    ns = await self.records.scope.get_in(tx, context.namespace.id)
                    await self.records.validate_sources_in(tx, ns, context.selected_sources)
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
                    identifier = str(uuid4())
                    address = await insert_address(
                        tx,
                        schema.journal_drafts,
                        "drf",
                        {
                            "id": identifier,
                            "namespace_id": ns.id,
                            "result_id": result.ref.id,
                            "body_artifact_id": body.id,
                            "context": context.model_dump(mode="json"),
                            "target_revision_id": context.target_revision.revision_id
                            if context.target_revision
                            else None,
                            "adopted_revision_id": None,
                            "recorded_at": self.results.artifacts.clock.utc_now().isoformat(),
                        },
                    )
                ref = DraftRef(
                    instance_id=self.results.artifacts.instance_id,
                    namespace_id=ns.id,
                    id=identifier,
                    address=address,
                )
                record_completed((result.ref.address, ref.address))
                draft = DraftView(
                    ref=ref,
                    result_ref=result.ref,
                    body_artifact=body,
                    body=text,
                    context=context,
                )
                return JournalizeOutcome(
                    status="draft",
                    draft_ref=ref,
                    draft=draft,
                    stage_results=(*produced.completed_stage_refs, result.ref.address),
                )
            except BaseException as error:
                add_error_context(
                    error,
                    {
                        "stage": "journalize.save_draft",
                        "model_succeeded": True,
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

    async def _draft_in(
        self, tx: ReadTx, target: str | DraftRef, namespace: str | None = None
    ) -> tuple[DraftRef, DraftContext, str, str, str | None]:
        if isinstance(target, DraftRef):
            if target.instance_id != self.results.artifacts.instance_id:
                raise NousError("cognition.instance_mismatch", "Draft belongs to another instance")
            selected, target_id = namespace or target.namespace_id, target.id
        else:
            selected, target_id = (
                namespace or self.execution.snapshot.typed_values().cognition.default_namespace,
                target,
            )
        ns = await self.records.scope.get_in(tx, selected)
        row = (
            (
                await tx.connection.execute(
                    sa.select(schema.journal_drafts).where(
                        sa.or_(
                            schema.journal_drafts.c.id == target_id,
                            schema.journal_drafts.c.address == target_id,
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
                address=str(row["address"]),
            ),
            DraftContext.model_validate(row["context"]),
            str(row["result_id"]),
            str(row["body_artifact_id"]),
            str(row["adopted_revision_id"]) if row["adopted_revision_id"] else None,
        )

    async def _revision_in(self, tx: ReadTx, revision_id: str) -> RecordRevisionRef:
        row = (
            (
                await tx.connection.execute(
                    sa.select(
                        schema.records.c.address,
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
        record, revision, _ = await self.records.resolve_in(
            tx, str(row["address"]) + "@" + str(row["ordinal"]), namespace=str(row["namespace_id"])
        )
        return self.records.revision_ref(record, revision)

    async def read_draft(
        self, target: str | DraftRef, *, namespace: str | None = None, sources: bool = False
    ) -> DraftView:
        async with self.execution.operation("journalize.read_draft"):
            async with self.database.read() as tx:
                ref, context, result_id, body_id, adopted = await self._draft_in(
                    tx, target, namespace
                )
                body = await self.results.artifacts.get_in(tx, body_id)
                revision = await self._revision_in(tx, adopted) if adopted else None
            result = await self.results.get(result_id)
            texts = (
                {
                    source.key: await self.results.artifacts.read_selection(
                        source.content_selection
                    )
                    for source in context.source_bindings
                }
                if sources
                else {}
            )
            return DraftView(
                ref=ref,
                result_ref=result.ref,
                body_artifact=body.ref,
                body=await self.results.artifacts.read_text(body.ref),
                context=context,
                adopted_revision=revision,
                source_texts=texts,
            )

    async def adopt_draft(
        self,
        target: str | DraftRef,
        *,
        namespace: str | None = None,
        body: str | None = None,
        sources: Sequence[SourceBinding] | None = None,
        title: str | None = None,
        target_revision: Target | None = None,
        fork: bool = False,
        refresh: bool = False,
        refresh_mode: Mode | None = None,
    ) -> SavedRecord:
        async with self.execution.operation("journalize.adopt_draft"):
            draft = await self.read_draft(target, namespace=namespace)
            text = draft.body if body is None else body
            published = await self.results.artifacts.content.publish_bytes(text.encode("utf-8"))
            try:
                async with self.database.write() as tx:
                    ref, context, result_id, _, adopted = await self._draft_in(
                        tx, draft.ref, namespace
                    )
                    if adopted is not None and not fork:
                        revision = await self._revision_in(tx, adopted)
                        record = await self.records.saved_in(
                            tx, revision, namespace=ref.namespace_id
                        )
                    else:
                        base = target_revision or context.target_revision
                        bindings = context.source_bindings if sources is None else tuple(sources)
                        metadata: AdoptionMetadata = {
                            "title": title,
                            "purpose": context.purpose,
                            "author": context.author,
                            "viewpoint": context.viewpoint,
                            "sources": bindings,
                            "times": context.times,
                            "origin_result_id": result_id,
                            "body_artifact": draft.body_artifact if text == draft.body else None,
                        }
                        if base is not None and not fork:
                            if (
                                isinstance(base, RecordRef)
                                or isinstance(base, str)
                                and "@" not in base
                            ):
                                raise NousError(
                                    "input.expected_required",
                                    "Adoption requires an exact target revision",
                                )
                            row, revision, _ = await self.records.resolve_in(
                                tx, base, ref.namespace_id
                            )
                            if row["kind"] != "journal":
                                raise NousError(
                                    "input.invalid_kind", "Journal draft target must be a Journal"
                                )
                            record = await self.records.append_in(
                                tx,
                                base,
                                self.records.revision_ref(row, revision),
                                text,
                                published,
                                namespace=ref.namespace_id,
                                change_kind="journalize",
                                **metadata,
                            )
                        else:
                            record = await self.records.create_in(
                                tx, ref.namespace_id, text, published, kind="journal", **metadata
                            )
                        if not fork:
                            await tx.connection.execute(
                                sa.update(schema.journal_drafts)
                                .where(schema.journal_drafts.c.id == ref.id)
                                .values(adopted_revision_id=record.revision_ref.revision_id)
                            )
                record_completed(record.revision_ref.address)
                return await self.records.complete_save(
                    record, refresh=refresh, refresh_mode=refresh_mode
                )
            except BaseException as error:
                add_error_context(
                    error,
                    {
                        "stage": "journalize.adopt_draft",
                        "draft_ref": draft.ref.model_dump(mode="json"),
                        "target_revision": (
                            draft.context.target_revision.model_dump(mode="json")
                            if draft.context.target_revision
                            else None
                        ),
                        "draft_saved": True,
                    },
                )
                raise

    async def status(self) -> dict[str, object]:
        async with self.execution.operation("journalize.status"):
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
