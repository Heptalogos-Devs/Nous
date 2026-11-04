"""Read exact written revisions and source bindings for every cognitive operation."""

from collections.abc import Sequence
from datetime import datetime
from typing import Literal, TypedDict, cast

import sqlalchemy as sa

from nous.common.errors import NousError
from nous.content.artifacts import ArtifactStore
from nous.content.contracts import (
    ArtifactRef,
    ContentSelection,
    SourceSelection,
)
from nous.runtime.execution import ExecutionManager
from nous.storage.database import Database, ReadTx
from nous.temporal.values import CognitiveTimes

from . import schema
from .contracts import (
    Namespace,
    RecordRef,
    RecordRevisionRef,
    RecordView,
    RevisionSummary,
    SavedRecord,
    SourceBinding,
)
from .scope import NamespaceOperations


class RecordRow(TypedDict):
    id: str
    namespace_id: str
    kind: Literal["note", "journal"]
    address: str
    head_revision_id: str
    archived: bool
    recall: bool


class RevisionRow(TypedDict):
    id: str
    record_id: str
    ordinal: int
    parent_id: str | None
    title: str
    body_artifact_id: str
    purpose: str | None
    author: str | None
    viewpoint: str | None
    cognitive_times: dict[str, object]
    change_kind: str
    reason: str | None
    origin_result_id: str | None
    recorded_at: str


Target = str | RecordRef | RecordRevisionRef


class WrittenRecordReader:
    def __init__(
        self,
        database: Database,
        execution: ExecutionManager,
        artifacts: ArtifactStore,
        scope: NamespaceOperations,
    ):
        self.database, self.execution, self.artifacts, self.scope = (
            database,
            execution,
            artifacts,
            scope,
        )

    def _namespace(self, namespace: str | None) -> str:
        return namespace or self.execution.snapshot.typed_values().cognition.default_namespace

    def _ref(self, row: RecordRow) -> RecordRef:
        return RecordRef(
            instance_id=self.artifacts.instance_id,
            namespace_id=row["namespace_id"],
            kind=row["kind"],
            id=row["id"],
            address=row["address"],
        )

    async def resolve_in(
        self, tx: ReadTx, target: Target, namespace: str | None = None
    ) -> tuple[RecordRow, RevisionRow, Namespace]:
        ordinal = None
        revision_id = None
        if isinstance(target, RecordRevisionRef):
            revision_id, ordinal, ref = target.revision_id, target.ordinal, target.record_ref
        elif isinstance(target, RecordRef):
            ref = target
        else:
            ref = None
            address, marker, suffix = target.rpartition("@")
            if marker:
                try:
                    ordinal = int(suffix)
                except ValueError as error:
                    raise NousError(
                        "input.invalid_reference", "Revision ordinal must be an integer"
                    ) from error
                target = address
        if ref:
            if ref.instance_id != self.artifacts.instance_id:
                raise NousError("cognition.instance_mismatch", "Record belongs to another instance")
            target = ref.id
        row = (
            (
                await tx.connection.execute(
                    sa.select(schema.records).where(
                        sa.or_(schema.records.c.id == target, schema.records.c.address == target)
                    )
                )
            )
            .mappings()
            .one_or_none()
        )
        if row is None:
            raise NousError(
                "cognition.not_found", "Written record does not exist", details={"ref": str(target)}
            )
        ns = await self.scope.get_in(
            tx, namespace or (ref.namespace_id if ref else self._namespace(None))
        )
        if row["namespace_id"] != ns.id or (
            ref and (ref.namespace_id != ns.id or ref.kind != row["kind"])
        ):
            raise NousError(
                "cognition.namespace_mismatch", "Record does not belong to the selected namespace"
            )
        clause = (
            schema.revisions.c.id == row["head_revision_id"]
            if ordinal is None
            else sa.and_(
                schema.revisions.c.record_id == row["id"], schema.revisions.c.ordinal == ordinal
            )
        )
        revision = (
            (await tx.connection.execute(sa.select(schema.revisions).where(clause)))
            .mappings()
            .one_or_none()
        )
        if revision is None or (revision_id and revision["id"] != revision_id):
            raise NousError("cognition.revision_not_found", "Exact record revision does not exist")
        return cast(RecordRow, dict(row)), cast(RevisionRow, dict(revision)), ns

    def revision_ref(self, row: RecordRow, revision: RevisionRow) -> RecordRevisionRef:
        return RecordRevisionRef(
            record_ref=self._ref(row), revision_id=revision["id"], ordinal=revision["ordinal"]
        )

    async def sources_in(self, tx: ReadTx, revision_id: str) -> tuple[SourceBinding, ...]:
        rows = (
            (
                await tx.connection.execute(
                    sa.select(schema.sources)
                    .where(schema.sources.c.revision_id == revision_id)
                    .order_by(schema.sources.c.ordinal)
                )
            )
            .mappings()
            .all()
        )
        output: list[SourceBinding] = []
        for row in rows:
            artifact = await self.artifacts.get_in(tx, row["artifact_id"])
            owner = None
            if row["target_revision_id"]:
                rev = (
                    (
                        await tx.connection.execute(
                            sa.select(schema.revisions).where(
                                schema.revisions.c.id == row["target_revision_id"]
                            )
                        )
                    )
                    .mappings()
                    .one()
                )
                record = (
                    (
                        await tx.connection.execute(
                            sa.select(schema.records).where(schema.records.c.id == rev["record_id"])
                        )
                    )
                    .mappings()
                    .one()
                )
                owner = self.revision_ref(
                    cast(RecordRow, dict(record)), cast(RevisionRow, dict(rev))
                )
            output.append(
                SourceBinding(
                    key=row["key"],
                    content_selection=ContentSelection(
                        artifact=artifact.ref,
                        selection=SourceSelection.model_validate(row["selection"]),
                    ),
                    owner_revision=owner,
                    usage=row["usage"],
                    name=row["name"],
                )
            )
        return tuple(output)

    async def select_sources(
        self,
        namespace: str | None,
        targets: Sequence[Target | ArtifactRef | ContentSelection],
        *,
        usage: Literal["reference", "current_basis", "inspiration"] = "reference",
    ) -> tuple[SourceBinding, ...]:
        """Freeze all selected logical records in one consistent short read."""
        async with self.execution.operation("records.select_sources"):
            async with self.database.read() as tx:
                ns = await self.scope.get_in(tx, self._namespace(namespace))
                output: list[SourceBinding] = []
                for index, target in enumerate(targets):
                    if isinstance(target, ContentSelection):
                        await self.artifacts.get_in(tx, target.artifact)
                        output.append(
                            SourceBinding(
                                key=f"S{index + 1}", content_selection=target, usage=usage
                            )
                        )
                        continue
                    if (
                        isinstance(target, ArtifactRef)
                        or isinstance(target, str)
                        and target.startswith("art:")
                    ):
                        artifact = await self.artifacts.get_in(tx, target)
                        output.append(
                            SourceBinding(
                                key=f"S{index + 1}",
                                content_selection=ContentSelection(artifact=artifact.ref),
                                name=artifact.display_name,
                                usage=usage,
                            )
                        )
                        continue
                    row, revision, _ = await self.resolve_in(tx, target, ns.id)
                    artifact = await self.artifacts.get_in(tx, revision["body_artifact_id"])
                    output.append(
                        SourceBinding(
                            key=f"S{index + 1}",
                            content_selection=ContentSelection(artifact=artifact.ref),
                            owner_revision=self.revision_ref(row, revision),
                            usage=usage,
                            name=revision["title"],
                        )
                    )
                return tuple(output)

    async def read(
        self,
        target: Target,
        *,
        namespace: str | None = None,
        sources: bool = False,
        blocks: Sequence[str] = (),
    ) -> RecordView:
        async with self.execution.operation("records.read"):
            async with self.database.read() as tx:
                row, rev, ns = await self.resolve_in(tx, target, namespace)
                head = (
                    (
                        await tx.connection.execute(
                            sa.select(schema.revisions).where(
                                schema.revisions.c.id == row["head_revision_id"]
                            )
                        )
                    )
                    .mappings()
                    .one()
                )
                artifact = await self.artifacts.get_in(tx, rev["body_artifact_id"])
                bindings = await self.sources_in(tx, rev["id"])
            body = await self.artifacts.read_text(artifact.ref)
            if blocks:
                view = await self.artifacts.text_view(artifact.ref)
                body = "\n\n".join(view.block(key).text for key in blocks)
            texts = (
                {
                    source.key: await self.artifacts.read_selection(source.content_selection)
                    for source in bindings
                }
                if sources
                else {}
            )
            return RecordView(
                ref=self._ref(row),
                revision_ref=self.revision_ref(row, rev),
                current_revision=self.revision_ref(row, cast(RevisionRow, dict(head))),
                namespace=ns,
                title=rev["title"],
                body_artifact=artifact.ref,
                body=body,
                purpose=rev["purpose"],
                author=rev["author"],
                viewpoint=rev["viewpoint"],
                times=CognitiveTimes.model_validate(rev["cognitive_times"]),
                sources=bindings,
                source_texts=texts,
                archived=row["archived"],
                recall=row["recall"],
                change_kind=rev["change_kind"],
                reason=rev["reason"],
                origin_result_id=rev["origin_result_id"],
            )

    def _saved(
        self, row: RecordRow, revision: RevisionRow, ns: Namespace, changed: bool = True
    ) -> SavedRecord:
        return SavedRecord(
            ref=self._ref(row),
            revision_ref=self.revision_ref(row, revision),
            title=revision["title"],
            namespace=ns,
            generation=ns.content_generation,
            changed=changed,
            retrieval_state="pending" if row["recall"] and not row["archived"] else "not_in_scope",
        )

    async def saved_in(
        self, tx: ReadTx, target: Target, *, namespace: str | None = None, changed: bool = False
    ) -> SavedRecord:
        row, revision, ns = await self.resolve_in(tx, target, namespace)
        return self._saved(row, revision, ns, changed)

    async def history(
        self, target: Target, *, namespace: str | None = None
    ) -> tuple[RevisionSummary, ...]:
        async with self.execution.operation("records.history"):
            async with self.database.read() as tx:
                row, _, _ = await self.resolve_in(tx, target, namespace)
                revisions = (
                    (
                        await tx.connection.execute(
                            sa.select(schema.revisions)
                            .where(schema.revisions.c.record_id == row["id"])
                            .order_by(schema.revisions.c.ordinal)
                        )
                    )
                    .mappings()
                    .all()
                )
                return tuple(
                    RevisionSummary(
                        revision_ref=self.revision_ref(row, cast(RevisionRow, dict(revision))),
                        title=revision["title"],
                        recorded_at=datetime.fromisoformat(revision["recorded_at"]),
                        change_kind=revision["change_kind"],
                        reason=revision["reason"],
                    )
                    for revision in revisions
                )

    async def list(
        self,
        namespace: str | None = None,
        *,
        kind: Literal["note", "journal"] | None = None,
        archived: bool = False,
        offset: int = 0,
        limit: int | None = None,
    ) -> tuple[SavedRecord, ...]:
        async with self.execution.operation("records.list"):
            limit = (
                limit
                if limit is not None
                else self.execution.snapshot.typed_values().cognition.list.page_size
            )
            if offset < 0 or not 1 <= limit <= 200:
                raise NousError(
                    "input.invalid_page", "Offset must be nonnegative and limit between 1 and 200"
                )
            async with self.database.read() as tx:
                ns = await self.scope.get_in(tx, self._namespace(namespace))
                query = sa.select(schema.records.c.id).where(
                    schema.records.c.namespace_id == ns.id, schema.records.c.archived == archived
                )
                if kind:
                    query = query.where(schema.records.c.kind == kind)
                ids = (
                    await tx.connection.scalars(
                        query.order_by(schema.records.c.address).offset(offset).limit(limit)
                    )
                ).all()
                output: list[SavedRecord] = []
                for identifier in ids:
                    row, revision, _ = await self.resolve_in(tx, identifier, ns.id)
                    output.append(self._saved(row, revision, ns, False))
                return tuple(output)

    async def status(self) -> dict[str, object]:
        async with self.execution.operation("records.status"):
            async with self.database.read() as tx:
                counts = (
                    (
                        await tx.connection.execute(
                            sa.select(
                                schema.records.c.namespace_id,
                                schema.records.c.kind,
                                schema.records.c.archived,
                                sa.func.count().label("count"),
                            ).group_by(
                                schema.records.c.namespace_id,
                                schema.records.c.kind,
                                schema.records.c.archived,
                            )
                        )
                    )
                    .mappings()
                    .all()
                )
                namespaces = (
                    (await tx.connection.execute(sa.select(schema.namespaces))).mappings().all()
                )
                return {
                    "namespaces": [
                        {
                            "id": row["id"],
                            "name": row["name"],
                            "content_generation": row["content_generation"],
                            "records": [
                                {
                                    "kind": count["kind"],
                                    "archived": count["archived"],
                                    "count": count["count"],
                                }
                                for count in counts
                                if count["namespace_id"] == row["id"]
                            ],
                        }
                        for row in namespaces
                    ]
                }
