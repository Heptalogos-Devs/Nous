"""Read exact written revisions and source bindings for every cognitive operation."""

from collections.abc import Sequence
from dataclasses import dataclass
from datetime import datetime
from typing import Literal, TypedDict, cast

import sqlalchemy as sa
from pydantic import TypeAdapter

from nous.common.errors import NousError
from nous.common.identifier import split_revision
from nous.common.material_refs import RecordRef, RecordRevisionRef
from nous.content.artifacts import ArtifactStore
from nous.content.contracts import (
    ArtifactRef,
    ContentSelection,
    MaterialRef,
    MaterialSource,
)
from nous.runtime.execution import ExecutionManager
from nous.storage.database import Database, ReadTx
from nous.temporal.values import CognitiveTimes

from .. import schema
from ..contracts import (
    Namespace,
    NamespaceRef,
    RecordSourceView,
    RecordView,
    RevisionSelection,
    RevisionSummary,
    TitleSource,
)
from ..scope import NamespaceOperations
from ..source_storage import read_sources_in
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
    source_signature: str
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
_MATERIAL_REF = TypeAdapter(MaterialRef)


@dataclass(frozen=True)
class CapturedRevision:
    """Exact record facts selected together without reading or parsing its body."""

    record: RecordRow
    revision: RevisionRow
    body: ArtifactRef
    sources: tuple[MaterialSource, ...]


@dataclass(frozen=True)
class RecordSearchFacts:
    available: bool
    purpose: str | None
    original_body: ArtifactRef | None


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

    async def current_refs_in(
        self, tx: ReadTx, namespace: NamespaceRef
    ) -> tuple[RecordRevisionRef, ...]:
        """List every current revision in the supplied namespace's read view."""
        selected = await self.namespace_in(tx, namespace)
        rows = (
            (
                await tx.connection.execute(
                    sa.select(schema.records, schema.revisions)
                    .join(
                        schema.revisions, schema.revisions.c.id == schema.records.c.head_revision_id
                    )
                    .where(schema.records.c.namespace_id == selected.id)
                )
            )
            .mappings()
            .all()
        )
        # SQLAlchemy disambiguates duplicate column names in a joined row, so fetch
        # each table's values by its column object rather than the rendered name.
        return tuple(
            self.revision_ref(
                cast(RecordRow, {column.name: row[column] for column in schema.records.columns}),
                cast(
                    RevisionRow, {column.name: row[column] for column in schema.revisions.columns}
                ),
            )
            for row in rows
        )

    async def capture_many_in(
        self, tx: ReadTx, refs: Sequence[RecordRevisionRef]
    ) -> tuple[CapturedRevision, ...]:
        """Capture exact revisions, current membership, origins and sources as one view."""
        if not refs:
            return ()
        if any(ref.record_ref.instance_id != self.artifacts.instance_id for ref in refs):
            raise NousError("cognition.instance_mismatch", "Record belongs to another instance")
        if self.bound_namespace and any(
            ref.record_ref.namespace_id != self.bound_namespace.id for ref in refs
        ):
            raise NousError("cognition.namespace_mismatch", "Record belongs to another namespace")
        records: dict[str, RecordRow] = {}
        revisions: dict[str, RevisionRow] = {}
        record_ids = list(dict.fromkeys(ref.record_ref.id for ref in refs))
        for offset in range(0, len(record_ids), 800):
            rows = (
                await tx.connection.execute(
                    sa.select(schema.records).where(
                        schema.records.c.id.in_(record_ids[offset : offset + 800])
                    )
                )
            ).mappings()
            records.update({str(row["id"]): cast(RecordRow, dict(row)) for row in rows})
        revision_ids = list(dict.fromkeys(ref.revision_id for ref in refs))
        for offset in range(0, len(revision_ids), 800):
            rows = (
                await tx.connection.execute(
                    sa.select(schema.revisions).where(
                        schema.revisions.c.id.in_(revision_ids[offset : offset + 800])
                    )
                )
            ).mappings()
            revisions.update({str(row["id"]): cast(RevisionRow, dict(row)) for row in rows})
        for ref in refs:
            row, revision = records.get(ref.record_ref.id), revisions.get(ref.revision_id)
            if row is None or revision is None or self.revision_ref(row, revision) != ref:
                raise NousError(
                    "cognition.revision_not_found", "Exact record revision does not exist"
                )
            if revision["record_id"] != row["id"]:
                raise NousError(
                    "cognition.revision_not_found", "Exact record revision does not exist"
                )
        selected_ids = list(dict.fromkeys(ref.revision_id for ref in refs))
        sources: dict[str, tuple[MaterialSource, ...]] = {}
        for offset in range(0, len(selected_ids), 800):
            sources.update(await self.sources_many_in(tx, selected_ids[offset : offset + 800]))
        artifact_ids = list(
            dict.fromkeys(revisions[ref.revision_id]["body_artifact_id"] for ref in refs)
        )
        artifacts = {
            value.ref.id: value.ref for value in await self.artifacts.get_many_in(tx, artifact_ids)
        }
        return tuple(
            CapturedRevision(
                record=records[ref.record_ref.id],
                revision=revisions[ref.revision_id],
                body=artifacts[revisions[ref.revision_id]["body_artifact_id"]],
                sources=sources[ref.revision_id],
            )
            for ref in refs
        )

    async def search_facts_in(
        self, tx: ReadTx, refs: Sequence[RecordRevisionRef]
    ) -> dict[str, RecordSearchFacts]:
        """Supply the record-owned eligibility and original roots used by retrieval."""
        record_ids = list(dict.fromkeys(ref.record_ref.id for ref in refs))
        revision_ids = list(dict.fromkeys(ref.revision_id for ref in refs))
        facts: dict[str, tuple[bool, str | None]] = {}
        originals: dict[str, str] = {}
        for offset in range(0, len(revision_ids), 800):
            rows = (
                await tx.connection.execute(
                    sa.select(
                        schema.revisions.c.id,
                        schema.revisions.c.purpose,
                        schema.records.c.archived,
                        schema.records.c.recall,
                    )
                    .join(schema.records, schema.records.c.id == schema.revisions.c.record_id)
                    .where(schema.revisions.c.id.in_(revision_ids[offset : offset + 800]))
                )
            ).mappings()
            facts.update(
                {
                    str(row["id"]): (not row["archived"] and bool(row["recall"]), row["purpose"])
                    for row in rows
                }
            )
        for offset in range(0, len(record_ids), 800):
            rows = (
                await tx.connection.execute(
                    sa.select(
                        schema.revisions.c.record_id, schema.revisions.c.body_artifact_id
                    ).where(
                        schema.revisions.c.record_id.in_(record_ids[offset : offset + 800]),
                        schema.revisions.c.ordinal == 1,
                        ~sa.select(1)
                        .where(schema.sources.c.revision_id == schema.revisions.c.id)
                        .exists(),
                    )
                )
            ).mappings()
            originals.update({str(row["record_id"]): str(row["body_artifact_id"]) for row in rows})
        artifacts = {
            item.ref.id: item.ref
            for item in await self.artifacts.get_many_in(tx, list(originals.values()))
        }
        return {
            ref.revision_id: RecordSearchFacts(
                available=facts[ref.revision_id][0],
                purpose=facts[ref.revision_id][1],
                original_body=artifacts[originals[ref.record_ref.id]]
                if ref.record_ref.id in originals
                else None,
            )
            for ref in refs
        }

    async def sources_in(self, tx: ReadTx, revision_id: str) -> tuple[MaterialSource, ...]:
        return (await self.sources_many_in(tx, (revision_id,)))[revision_id]

    async def _source_details(
        self, bindings: Sequence[MaterialSource]
    ) -> tuple[RecordSourceView, ...]:
        owner_ids = {
            binding.material.revision_id
            for binding in bindings
            if isinstance(binding.material, RecordRevisionRef)
        }
        titles: dict[str, str | None] = {}
        if owner_ids:
            async with self.database.read() as tx:
                rows = (
                    await tx.connection.execute(
                        sa.select(schema.revisions.c.id, schema.revisions.c.title).where(
                            schema.revisions.c.id.in_(owner_ids)
                        )
                    )
                ).all()
                titles = {str(row.id): row.title for row in rows}
        selected = await self.artifacts.read_view_many(
            [selection for binding in bindings for selection in binding.selections]
        )
        output: list[RecordSourceView] = []
        offset = 0
        for binding in bindings:
            count = len(binding.selections)
            output.append(
                RecordSourceView(
                    binding=binding,
                    views=selected[offset : offset + count],
                    title=titles.get(binding.material.revision_id)
                    if isinstance(binding.material, RecordRevisionRef)
                    else None,
                )
            )
            offset += count
        return tuple(output)

    async def read_sources(self, revision: RecordRevisionRef) -> tuple[RecordSourceView, ...]:
        """Read the precise source selections and titles attached to one revision."""
        async with self.database.read() as tx:
            _, row, _ = await self.resolve_in(tx, revision, revision.record_ref.namespace_id)
            bindings = await self.sources_in(tx, row["id"])
        return await self._source_details(bindings)

    async def sources_many_in(
        self, tx: ReadTx, revision_ids: Sequence[str]
    ) -> dict[str, tuple[MaterialSource, ...]]:
        return await read_sources_in(tx, self.artifacts, revision_ids)

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
            details = await self._source_details(bindings) if sources else ()
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
                source_details=details,
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
