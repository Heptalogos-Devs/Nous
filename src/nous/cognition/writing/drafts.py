"""Store complete writing drafts and their exact product and source references."""

from __future__ import annotations

import difflib
from dataclasses import dataclass
from typing import Literal, cast
from uuid import uuid4

import sqlalchemy as sa

from nous.common.errors import NousError
from nous.common.identifier import insert_identifier
from nous.content.contracts import ArtifactRef, ContentReadView, ResultRef
from nous.content.results import ResultStore
from nous.runtime.execution import ExecutionManager
from nous.storage.database import Database, ReadTx, WriteTx

from .. import schema
from ..contracts import RecordRevisionRef
from .contracts import DraftContext, DraftRef, DraftView
from .reading import RecordReader
from .records import WrittenRecords


@dataclass(frozen=True)
class DraftDescriptor:
    ref: DraftRef
    context: DraftContext
    product: ResultRef
    body: ArtifactRef
    composition: ResultRef | None
    parent: DraftRef | None
    adopted_revision_id: str | None
    adopted_as: Literal["default", "fork"] | None


class DraftStore:
    """Own immutable drafts; product/composition and draft commit in one transaction."""

    def __init__(
        self,
        database: Database,
        execution: ExecutionManager,
        core: WrittenRecords,
        reader: RecordReader,
        results: ResultStore,
    ) -> None:
        self.database, self.execution, self.core, self.reader = database, execution, core, reader
        self.results = results

    async def create_in(
        self,
        tx: WriteTx,
        *,
        product: ResultRef,
        body: ArtifactRef,
        context: DraftContext,
        composition: ResultRef | None = None,
        parent: DraftRef | None = None,
    ) -> DraftRef:
        """Insert a complete draft in the caller's transaction."""
        ns = await self.reader.scope.get_in(tx, context.namespace_ref)
        await self.core.validate_sources_in(tx, ns, context.source_bindings)
        await self.results.get_in(tx, product)
        await self.results.artifacts.get_in(tx, body)
        if composition is not None:
            await self.results.get_in(tx, composition)
        if context.base_revision is not None:
            await self.reader.resolve_in(tx, context.base_revision, ns.id)
        if parent is not None:
            await self.descriptor_in(tx, parent, ns.id)
        object_id = str(uuid4())
        identifier = await insert_identifier(
            tx,
            schema.journal_drafts,
            "drf",
            {
                "id": object_id,
                "namespace_id": ns.id,
                "result_id": product.id,
                "composition_result_id": composition.id if composition else None,
                "body_artifact_id": body.id,
                "context": context.model_dump(mode="json"),
                "base_revision_id": context.base_revision.revision_id
                if context.base_revision
                else None,
                "parent_draft_id": parent.id if parent else None,
                "adopted_revision_id": None,
                "adopted_as": None,
                "recorded_at": self.results.artifacts.clock.utc_now().isoformat(),
            },
        )
        return DraftRef(
            instance_id=self.results.artifacts.instance_id,
            namespace_id=ns.id,
            id=object_id,
            identifier=identifier,
        )

    async def descriptor_in(
        self, tx: ReadTx, target: str | DraftRef, namespace: str | None = None
    ) -> DraftDescriptor:
        """Resolve one saved draft within the caller's consistent read view."""
        if isinstance(target, DraftRef):
            if target.instance_id != self.results.artifacts.instance_id:
                raise NousError("cognition.instance_mismatch", "Draft belongs to another instance")
            selected, target_id = namespace or target.namespace_id, target.id
        else:
            selected, target_id = namespace, target
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
        ref = DraftRef(
            instance_id=self.results.artifacts.instance_id,
            namespace_id=str(row["namespace_id"]),
            id=str(row["id"]),
            identifier=str(row["identifier"]),
        )
        if ref.namespace_id != ns.id or isinstance(target, DraftRef) and target != ref:
            raise NousError(
                "cognition.namespace_mismatch", "Draft does not belong to this namespace"
            )
        product = await self.results.get_in(tx, str(row["result_id"]))
        body = await self.results.artifacts.get_in(tx, str(row["body_artifact_id"]))
        composition = (
            (await self.results.get_in(tx, str(row["composition_result_id"]))).ref
            if row["composition_result_id"]
            else None
        )
        parent = None
        if row["parent_draft_id"]:
            parent_row = (
                (
                    await tx.connection.execute(
                        sa.select(
                            schema.journal_drafts.c.id,
                            schema.journal_drafts.c.identifier,
                            schema.journal_drafts.c.namespace_id,
                        ).where(schema.journal_drafts.c.id == row["parent_draft_id"])
                    )
                )
                .mappings()
                .one()
            )
            parent = DraftRef(
                instance_id=ref.instance_id,
                namespace_id=str(parent_row["namespace_id"]),
                id=str(parent_row["id"]),
                identifier=str(parent_row["identifier"]),
            )
        return DraftDescriptor(
            ref=ref,
            context=DraftContext.model_validate(row["context"]),
            product=product.ref,
            body=body.ref,
            composition=composition,
            parent=parent,
            adopted_revision_id=str(row["adopted_revision_id"])
            if row["adopted_revision_id"]
            else None,
            adopted_as=cast(Literal["default", "fork"] | None, row["adopted_as"]),
        )

    async def revision_in(self, tx: ReadTx, revision_id: str) -> RecordRevisionRef:
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
        """Read a saved draft without any model or execution-recovery dependency."""
        async with self.execution.operation("writing.read_draft"):
            async with self.database.read() as tx:
                item = await self.descriptor_in(tx, target, namespace)
                revision = (
                    await self.revision_in(tx, item.adopted_revision_id)
                    if item.adopted_revision_id
                    else None
                )
            body = await self.results.artifacts.read_text(item.body)
            views: dict[str, tuple[ContentReadView, ...]] = {}
            if sources:
                selected = await self.results.artifacts.read_view_many(
                    [
                        selection
                        for binding in item.context.source_bindings
                        for selection in binding.selections
                    ]
                )
                offset = 0
                for binding in item.context.source_bindings:
                    views[binding.key] = selected[offset : offset + len(binding.selections)]
                    offset += len(binding.selections)
            diff = None
            if item.context.base_revision is not None:
                base = await self.reader.read(
                    item.context.base_revision, namespace=item.ref.namespace_id
                )
                diff = "".join(
                    difflib.unified_diff(
                        base.body.splitlines(keepends=True),
                        body.splitlines(keepends=True),
                        fromfile=base.revision_ref.identifier,
                        tofile=item.ref.identifier,
                    )
                )
            return DraftView(
                ref=item.ref,
                result_ref=item.product,
                composition_result_ref=item.composition,
                parent_draft=item.parent,
                body_artifact=item.body,
                body=body,
                context=item.context,
                adopted_revision=revision,
                adopted_as=item.adopted_as,
                diff=diff,
                source_views=views,
            )

    async def status(self) -> dict[str, object]:
        async with self.database.read() as tx:
            total = await tx.connection.scalar(
                sa.select(sa.func.count()).select_from(schema.journal_drafts)
            )
            unadopted = await tx.connection.scalar(
                sa.select(sa.func.count())
                .select_from(schema.journal_drafts)
                .where(schema.journal_drafts.c.adopted_revision_id.is_(None))
            )
        return {"drafts": total, "unadopted": unadopted}
