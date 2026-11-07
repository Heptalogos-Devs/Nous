"""Read exact written revisions and source bindings for every cognitive operation."""

from collections.abc import Sequence
from datetime import datetime
from typing import Literal, TypedDict, cast

import sqlalchemy as sa

from nous.common.errors import NousError
from nous.common.identifier import split_revision
from nous.content.artifacts import ArtifactStore
from nous.content.contracts import (
    ContentReadView,
    ContentSelection,
    SourceSelection,
)
from nous.runtime.execution import ExecutionManager
from nous.storage.database import Database, ReadTx
from nous.temporal.values import CognitiveTimes

from .. import schema
from ..contracts import (
    Namespace,
    NamespaceRef,
    RecordRef,
    RecordRevisionRef,
    RecordView,
    RevisionSelection,
    RevisionSummary,
    SourceBinding,
    TitleSource,
)
from ..scope import NamespaceOperations
from .contracts import CommittedRecord


class RecordRow(TypedDict):
    id: str
    namespace_id: str
    kind: Literal["note", "journal"]
    identifier: str
    head_revision_id: str
    archived: bool
    recall: bool


class RevisionRow(TypedDict):
    id: str
    record_id: str
    ordinal: int
    committed_generation: int
    parent_id: str | None
    title: str | None
    title_source: TitleSource
    body_artifact_id: str
    purpose: str | None
    author: str | None
    viewpoint: str | None
    cognitive_times: dict[str, object]
    change_kind: str
    reason: str | None
    write_key: str | None
    write_fingerprint: str | None
    origin_result_id: str | None
    recorded_at: str


Target = str | RecordRef | RecordRevisionRef


class RecordReader:
    """Read formal record revisions and frozen source bindings.

    Targets may identify a record or an exact revision. Unqualified record targets
    resolve to the current head; a namespace-bound reader rejects other namespaces.
    """

    def __init__(
        self,
        database: Database,
        execution: ExecutionManager,
        artifacts: ArtifactStore,
        scope: NamespaceOperations,
        namespace: NamespaceRef | None = None,
    ):
        self.bound_namespace = namespace
        self.database, self.execution, self.artifacts, self.scope = (
            database,
            execution,
            artifacts,
            scope,
        )

    async def namespace_in(self, tx: ReadTx, namespace: str | NamespaceRef | None) -> Namespace:
        return await self.scope.bound_in(tx, self.bound_namespace, namespace)

    def _ref(self, row: RecordRow) -> RecordRef:
        return RecordRef(
            instance_id=self.artifacts.instance_id,
            namespace_id=row["namespace_id"],
            kind=row["kind"],
            id=row["id"],
            identifier=row["identifier"],
        )

    async def resolve_in(
        self, tx: ReadTx, target: Target, namespace: str | None = None
    ) -> tuple[RecordRow, RevisionRow, Namespace]:
        """Resolve a target and namespace in the caller's read view.

        An identifier suffixed with ``@N`` or a revision reference selects that exact
        revision; an unqualified record selects its head. The returned record and
        revision rows are copies.

        Raises:
            NousError: The reference is malformed, missing, or belongs to another
                instance or namespace, or the exact revision does not exist.
        """
        ordinal = None
        revision_id = None
        if isinstance(target, RecordRevisionRef):
            revision_id, ordinal, ref = target.revision_id, target.ordinal, target.record_ref
        elif isinstance(target, RecordRef):
            ref = target
        else:
            ref = None
            target, ordinal = split_revision(target)
        if ref:
            if ref.instance_id != self.artifacts.instance_id:
                raise NousError("cognition.instance_mismatch", "Record belongs to another instance")
            target = ref.id
        row = (
            (
                await tx.connection.execute(
                    sa.select(schema.records).where(
                        sa.or_(schema.records.c.id == target, schema.records.c.identifier == target)
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
        ns = await self.namespace_in(tx, namespace or (ref.namespace_id if ref else None))
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
        return (await self.sources_many_in(tx, (revision_id,)))[revision_id]

    async def sources_many_in(
        self, tx: ReadTx, revision_ids: Sequence[str]
    ) -> dict[str, tuple[SourceBinding, ...]]:
        """Read ordered source groups and all discontinuous targets in the same consistent view."""
        if not revision_ids:
            return {}
        groups = (
            (
                await tx.connection.execute(
                    sa.select(schema.sources)
                    .where(schema.sources.c.revision_id.in_(revision_ids))
                    .order_by(schema.sources.c.revision_id, schema.sources.c.ordinal)
                )
            )
            .mappings()
            .all()
        )
        targets = (
            (
                await tx.connection.execute(
                    sa.select(schema.source_targets)
                    .where(schema.source_targets.c.revision_id.in_(revision_ids))
                    .order_by(
                        schema.source_targets.c.revision_id,
                        schema.source_targets.c.key,
                        schema.source_targets.c.ordinal,
                    )
                )
            )
            .mappings()
            .all()
        )
        artifacts = await self.artifacts.get_many_in(
            tx, [str(row["artifact_id"]) for row in targets]
        )
        selected: dict[tuple[str, str], list[ContentSelection]] = {}
        for row, artifact in zip(targets, artifacts, strict=True):
            selected.setdefault((str(row["revision_id"]), str(row["key"])), []).append(
                ContentSelection(
                    artifact=artifact.ref,
                    selection=SourceSelection.model_validate(row["selection"]),
                )
            )
        owner_ids = {str(row["target_revision_id"]) for row in groups if row["target_revision_id"]}
        owners: dict[str, RecordRevisionRef] = {}
        if owner_ids:
            revisions = (
                (
                    await tx.connection.execute(
                        sa.select(schema.revisions).where(schema.revisions.c.id.in_(owner_ids))
                    )
                )
                .mappings()
                .all()
            )
            records = (
                await tx.connection.execute(
                    sa.select(schema.records).where(
                        schema.records.c.id.in_([row["record_id"] for row in revisions])
                    )
                )
            ).mappings()
            records_by_id = {str(row["id"]): cast(RecordRow, dict(row)) for row in records}
            owners = {
                str(row["id"]): self.revision_ref(
                    records_by_id[str(row["record_id"])], cast(RevisionRow, dict(row))
                )
                for row in revisions
            }
        output: dict[str, list[SourceBinding]] = {object_id: [] for object_id in revision_ids}
        for row in groups:
            output[str(row["revision_id"])].append(
                SourceBinding(
                    key=str(row["key"]),
                    selections=tuple(selected[(str(row["revision_id"]), str(row["key"]))]),
                    owner_revision=owners.get(str(row["target_revision_id"])),
                    usage=row["usage"],
                    name=row["name"],
                )
            )
        return {object_id: tuple(bindings) for object_id, bindings in output.items()}

    async def read(
        self,
        target: Target,
        *,
        namespace: str | None = None,
        sources: bool = False,
        blocks: Sequence[str] = (),
    ) -> RecordView:
        """Read a record's selected revision, body and source bindings.

        Unqualified targets select the head. ``blocks`` restricts the returned body to
        those block keys in order, joined by blank lines. ``sources`` additionally
        loads source content views; bindings are included in either case. The view
        also reports the current revision and current membership state.
        """
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
            if blocks:
                view = await self.artifacts.text_view(artifact.ref)
                selected_blocks = [view.block(key) for key in blocks]
                body = "\n\n".join(block.text for block in selected_blocks)
                selections = tuple(block.selection for block in selected_blocks)
            else:
                body = await self.artifacts.read_text(artifact.ref)
                selections = (ContentSelection(artifact=artifact.ref),)
            views: dict[str, tuple[ContentReadView, ...]] = {}
            if sources:
                selected_views = await self.artifacts.read_view_many(
                    [selection for binding in bindings for selection in binding.selections]
                )
                offset = 0
                for binding in bindings:
                    views[binding.key] = selected_views[offset : offset + len(binding.selections)]
                    offset += len(binding.selections)
            return RecordView(
                ref=self._ref(row),
                revision_ref=self.revision_ref(row, rev),
                committed_generation=rev["committed_generation"],
                current_revision=self.revision_ref(row, cast(RevisionRow, dict(head))),
                namespace=ns,
                title=rev["title"],
                title_source=rev["title_source"],
                body_artifact=artifact.ref,
                body=body,
                selection=RevisionSelection(
                    revision=self.revision_ref(row, rev), selections=selections
                ),
                purpose=rev["purpose"],
                author=rev["author"],
                viewpoint=rev["viewpoint"],
                times=CognitiveTimes.model_validate(rev["cognitive_times"]),
                sources=bindings,
                source_views=views,
                archived=row["archived"],
                recall=row["recall"],
                change_kind=rev["change_kind"],
                reason=rev["reason"],
                write_key=rev["write_key"],
                origin_result_id=rev["origin_result_id"],
            )

    def committed(
        self, row: RecordRow, revision: RevisionRow, ns: Namespace, changed: bool = True
    ) -> CommittedRecord:
        return CommittedRecord(
            ref=self._ref(row),
            revision_ref=self.revision_ref(row, revision),
            title=revision["title"],
            title_source=revision["title_source"],
            namespace=ns,
            committed_generation=revision["committed_generation"],
            changed=changed,
            in_recall=bool(row["recall"] and not row["archived"]),
        )

    async def committed_in(
        self, tx: ReadTx, target: Target, *, namespace: str | None = None, changed: bool = False
    ) -> CommittedRecord:
        row, revision, ns = await self.resolve_in(tx, target, namespace)
        return self.committed(row, revision, ns, changed)

    async def history(
        self, target: Target, *, namespace: str | None = None
    ) -> tuple[RevisionSummary, ...]:
        """Return revision summaries in ascending ordinal order for the record."""
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
                        committed_generation=revision["committed_generation"],
                        title=revision["title"],
                        title_source=revision["title_source"],
                        recorded_at=datetime.fromisoformat(revision["recorded_at"]),
                        change_kind=revision["change_kind"],
                        reason=revision["reason"],
                        write_key=revision["write_key"],
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
    ) -> tuple[CommittedRecord, ...]:
        """List current record heads in identifier order for the selected namespace.

        The archived flag selects archived or unarchived records. Offset must be
        nonnegative and limit between 1 and 200; an omitted limit uses configuration.
        """
        async with self.execution.operation("records.list"):
            limit = (
                limit
                if limit is not None
                else self.execution.snapshot.values.cognition.list.page_size
            )
            if offset < 0 or not 1 <= limit <= 200:
                raise NousError(
                    "input.invalid_page", "Offset must be nonnegative and limit between 1 and 200"
                )
            async with self.database.read() as tx:
                ns = await self.namespace_in(tx, namespace)
                query = sa.select(schema.records.c.id).where(
                    schema.records.c.namespace_id == ns.id, schema.records.c.archived == archived
                )
                if kind:
                    query = query.where(schema.records.c.kind == kind)
                ids = (
                    await tx.connection.scalars(
                        query.order_by(schema.records.c.identifier).offset(offset).limit(limit)
                    )
                ).all()
                output: list[CommittedRecord] = []
                for object_id in ids:
                    row, revision, _ = await self.resolve_in(tx, object_id, ns.id)
                    output.append(self.committed(row, revision, ns, False))
                return tuple(output)

    async def status(self) -> dict[str, object]:
        """Report record counts and namespace state without modifying records."""
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
