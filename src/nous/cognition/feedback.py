"""Record opinions and apply explicit corrections in the written-record transaction."""

from __future__ import annotations

from collections.abc import Sequence
from datetime import datetime
from typing import Literal, cast
from uuid import uuid4

import sqlalchemy as sa
from pydantic import JsonValue

from nous.common.address import insert_address
from nous.common.errors import NousError
from nous.common.operations import record_completed
from nous.content.contracts import ContentSelection
from nous.runtime.execution import ExecutionManager
from nous.storage.database import Database, WriteTx
from nous.wave.operations import WaveOperations

from . import feedback_schema, schema
from .contracts import CognitiveValue, RecordRevisionRef, SavedRecord, SourceBinding
from .records import Target, WrittenRecords
from .retrieval import Mode
from .scope import NamespaceOperations

Category = Literal["usefulness", "agreement", "accuracy", "retrieval"]
Value = Literal["positive", "negative", "neutral"]


class FeedbackView(CognitiveValue):
    id: str
    address: str
    namespace_id: str
    target: dict[str, JsonValue]
    category: Category
    value: Value | None = None
    comment: str | None = None
    author: str | None = None
    recorded_at: datetime
    applied_revision: RecordRevisionRef | None = None


class FeedbackOperations:
    def __init__(
        self,
        database: Database,
        execution: ExecutionManager,
        scope: NamespaceOperations,
        records: WrittenRecords,
        wave: WaveOperations,
    ) -> None:
        self.database, self.execution, self.scope = database, execution, scope
        self.records, self.wave = records, wave

    async def _insert_in(
        self,
        tx: WriteTx,
        namespace_id: str,
        target: dict[str, JsonValue],
        category: Category,
        value: Value | None,
        comment: str | None,
        author: str | None,
        applied: RecordRevisionRef | None = None,
    ) -> FeedbackView:
        identifier = str(uuid4())
        recorded = self.records.time.clock.utc_now()
        address = await insert_address(
            tx,
            feedback_schema.feedback,
            "fbk",
            {
                "id": identifier,
                "namespace_id": namespace_id,
                "target": target,
                "category": category,
                "value": value,
                "comment": comment,
                "author": author,
                "recorded_at": recorded.isoformat(),
                "applied_revision_id": applied.revision_id if applied else None,
            },
        )
        return FeedbackView(
            id=identifier,
            address=address,
            namespace_id=namespace_id,
            target=target,
            category=category,
            value=value,
            comment=comment,
            author=author,
            recorded_at=recorded,
            applied_revision=applied,
        )

    async def record(
        self,
        target: Target,
        category: Category,
        *,
        value: Value | None = None,
        comment: str | None = None,
        author: str | None = None,
        namespace: str | None = None,
        snapshot: str | None = None,
        candidate: str | None = None,
        selection: ContentSelection | None = None,
    ) -> FeedbackView:
        async with self.execution.operation("feedback.record"):
            selected = (
                namespace or self.execution.snapshot.typed_values().cognition.default_namespace
            )
            hit: dict[str, object] | None = None
            if candidate and not snapshot:
                raise NousError("input.invalid_feedback", "A candidate requires its snapshot")
            if snapshot:
                async with self.database.read() as tx:
                    ns = await self.scope.get_in(tx, selected)
                info = await self.wave.inspect(snapshot)
                if not str(info["collection"]).startswith(f"cognition:{ns.id}:"):
                    raise NousError(
                        "cognition.namespace_mismatch", "Snapshot belongs to another namespace"
                    )
                if candidate:
                    hit = await self.wave.read(snapshot, candidate)
            if selection is not None:
                await self.records.artifacts.read_selection(selection)
            async with self.database.write() as tx:
                row, revision, ns = await self.records.resolve_in(tx, target, selected)
                if selection is not None and (
                    selection.artifact.instance_id != self.records.artifacts.instance_id
                    or selection.artifact.id != revision["body_artifact_id"]
                ):
                    raise NousError(
                        "cognition.selection_mismatch",
                        "Feedback selection belongs to another revision",
                    )
                exact = self.records.revision_ref(row, revision)
                if hit is not None:
                    hit_metadata = cast(dict[str, object], hit["metadata"])
                    if RecordRevisionRef.model_validate(hit_metadata["record_revision"]) != exact:
                        raise NousError(
                            "cognition.selection_mismatch", "Candidate belongs to another revision"
                        )
                result = await self._insert_in(
                    tx,
                    ns.id,
                    {
                        "revision": cast(dict[str, JsonValue], exact.model_dump(mode="json")),
                        "snapshot": snapshot,
                        "candidate": candidate,
                        "selection": (
                            cast(dict[str, JsonValue], selection.model_dump(mode="json"))
                            if selection is not None
                            else None
                        ),
                    },
                    category,
                    value,
                    comment,
                    author,
                )
            record_completed(result.address)
            return result

    async def record_query(
        self,
        context: dict[str, JsonValue],
        category: Category,
        *,
        value: Value | None = None,
        comment: str | None = None,
        author: str | None = None,
        namespace: str | None = None,
    ) -> FeedbackView:
        async with self.execution.operation("feedback.record_query"):
            selected = (
                namespace or self.execution.snapshot.typed_values().cognition.default_namespace
            )
            async with self.database.read() as tx:
                ns = await self.scope.get_in(tx, selected)
            snapshot_value = context.get("snapshot")
            snapshot = (
                snapshot_value.get("address")
                if isinstance(snapshot_value, dict)
                else snapshot_value
            )
            if not isinstance(snapshot, str):
                raise NousError("input.invalid_feedback", "Query context must include its snapshot")
            info = await self.wave.inspect(snapshot)
            if not str(info["collection"]).startswith(f"cognition:{ns.id}:"):
                raise NousError(
                    "cognition.namespace_mismatch", "Snapshot belongs to another namespace"
                )
            hits = context.get("hits", [])
            if not isinstance(hits, list):
                raise NousError("input.invalid_feedback", "Query hits must be a list")
            for hit in hits:
                if isinstance(hit, dict) and hit.get("record_revision") is not None:
                    ref = RecordRevisionRef.model_validate(hit["record_revision"])
                    if ref.record_ref.namespace_id != ns.id:
                        raise NousError(
                            "cognition.namespace_mismatch", "Query hit belongs to another namespace"
                        )
            async with self.database.write() as tx:
                result = await self._insert_in(
                    tx, ns.id, {"query": context}, category, value, comment, author
                )
            record_completed(result.address)
            return result

    async def correct(
        self,
        target: Target,
        expected_revision: str | int | RecordRevisionRef,
        replacement: str,
        *,
        reason: str,
        author: str | None = None,
        namespace: str | None = None,
        sources: Sequence[SourceBinding] | None = None,
        refresh: bool = False,
        refresh_mode: Mode | None = None,
    ) -> tuple[FeedbackView, SavedRecord]:
        if not reason.strip():
            raise NousError("input.reason_required", "A correction requires a reason")
        async with self.execution.operation("feedback.correct"):
            selected = (
                namespace or self.execution.snapshot.typed_values().cognition.default_namespace
            )
            published = await self.records.artifacts.content.publish_bytes(
                replacement.encode("utf-8")
            )
            async with self.database.write() as tx:
                row, old, ns = await self.records.resolve_in(tx, target, selected)
                exact = self.records.revision_ref(row, old)
                saved = await self.records.append_in(
                    tx,
                    exact,
                    expected_revision,
                    replacement,
                    published,
                    namespace=ns.id,
                    change_kind="correction",
                    reason=reason,
                    author=author,
                    sources=sources,
                )
                result = await self._insert_in(
                    tx,
                    ns.id,
                    {"revision": cast(dict[str, JsonValue], exact.model_dump(mode="json"))},
                    "accuracy",
                    "negative",
                    reason,
                    author,
                    saved.revision_ref,
                )
            record_completed(saved.revision_ref.address)
            record_completed(result.address)
            return result, await self.records.complete_save(
                saved, refresh=refresh, refresh_mode=refresh_mode
            )

    async def list(self, namespace: str | None = None) -> tuple[FeedbackView, ...]:
        async with self.execution.operation("feedback.list"):
            selected = (
                namespace or self.execution.snapshot.typed_values().cognition.default_namespace
            )
            async with self.database.read() as tx:
                ns = await self.scope.get_in(tx, selected)
                rows = (
                    (
                        await tx.connection.execute(
                            sa.select(feedback_schema.feedback)
                            .where(feedback_schema.feedback.c.namespace_id == ns.id)
                            .order_by(feedback_schema.feedback.c.recorded_at.desc())
                        )
                    )
                    .mappings()
                    .all()
                )
                output: list[FeedbackView] = []
                for row in rows:
                    applied = None
                    if row["applied_revision_id"]:
                        revision = (
                            (
                                await tx.connection.execute(
                                    sa.select(schema.revisions).where(
                                        schema.revisions.c.id == row["applied_revision_id"]
                                    )
                                )
                            )
                            .mappings()
                            .one()
                        )
                        record, exact, _ = await self.records.resolve_in(
                            tx, f"{revision['record_id']}@{revision['ordinal']}", ns.id
                        )
                        applied = self.records.revision_ref(record, exact)
                    output.append(
                        FeedbackView(
                            id=row["id"],
                            address=row["address"],
                            namespace_id=ns.id,
                            target=row["target"],
                            category=row["category"],
                            value=row["value"],
                            comment=row["comment"],
                            author=row["author"],
                            recorded_at=datetime.fromisoformat(row["recorded_at"]),
                            applied_revision=applied,
                        )
                    )
                return tuple(output)
