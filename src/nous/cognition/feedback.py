"""Opinions about exact records or delivered queries and their disposition."""

from __future__ import annotations

from collections.abc import Mapping, Sequence
from datetime import datetime
from typing import TYPE_CHECKING, Literal
from uuid import uuid4

import sqlalchemy as sa
from sqlalchemy.engine import RowMapping

from nous.application.contracts import CallContext, define_operation
from nous.common.errors import NousError
from nous.common.identifier import insert_identifier
from nous.common.operations import record_completed
from nous.content.contracts import ContentSelection, ExternalInput
from nous.runtime.execution import ExecutionManager
from nous.storage.database import Database, ReadTx, WriteTx
from nous.temporal.values import Clock
from nous.wave.contracts import CandidateRef, CognitionOwner, RankEvidence
from nous.wave.operations import WaveOperations

from . import feedback_schema, schema
from .contracts import (
    CognitiveValue,
    FeedbackRef,
    Namespace,
    NamespaceRef,
    NamespaceSelector,
    RecordRef,
    RecordRevisionRef,
    RevisionSelection,
)
from .retrieval.contracts import (
    CognitiveGroupContext,
    QueryContext,
    QueryOperationContext,
    QueryResult,
)
from .scope import NamespaceOperations

if TYPE_CHECKING:
    from .writing.reading import RecordReader

Category = Literal["usefulness", "agreement", "accuracy", "retrieval"]
Value = Literal["positive", "negative", "neutral"]
ResolutionAction = Literal["acknowledged", "not_applied", "applied"]


class QueryFeedback(CognitiveValue):
    """Freeze the query and the ranking basis for the selected delivery."""

    context: QueryContext
    operation_context: QueryOperationContext
    group_rank: int | None = None
    basis: RankEvidence | None = None
    candidate: CandidateRef | None = None


class FeedbackTarget(CognitiveValue):
    kind: Literal["record", "query"]
    revision: RecordRevisionRef | None = None
    selection: ContentSelection | RevisionSelection | None = None
    query: QueryFeedback | None = None


class FeedbackResolution(CognitiveValue):
    action: ResolutionAction
    note: str | None = None
    mapping_note: str | None = None
    applied_revision: RecordRevisionRef | None = None
    actor: str | None = None
    resolved_at: datetime


class FeedbackView(CognitiveValue):
    id: str
    identifier: str
    namespace_id: str
    target: FeedbackTarget
    category: Category
    value: Value | None = None
    comment: str | None = None
    author: str | None = None
    recorded_at: datetime
    resolution: FeedbackResolution | None = None


class QueryFeedbackRequest(CognitiveValue):
    result: QueryResult | ExternalInput
    category: Category
    target: tuple[int, int] | RevisionSelection | None = None
    value: Value | None = None
    comment: str | None = None
    author: str | None = None
    namespace: NamespaceSelector = None


class RecordFeedbackRequest(CognitiveValue):
    target: str | RecordRevisionRef
    category: Category
    value: Value | None = None
    comment: str | None = None
    author: str | None = None
    namespace: NamespaceSelector = None
    selection: ContentSelection | ExternalInput | None = None


class ReadFeedback(CognitiveValue):
    ref: str
    namespace: NamespaceSelector = None


class ListFeedback(CognitiveValue):
    target: str | RecordRef | RecordRevisionRef | None = None
    category: Category | None = None
    unresolved_only: bool = False
    limit: int = 50
    offset: int = 0
    namespace: NamespaceSelector = None


class ResolveFeedback(CognitiveValue):
    ref: str
    action: Literal["acknowledged", "not_applied"]
    note: str | None = None
    actor: str | None = None
    namespace: NamespaceSelector = None


RECORD_QUERY_FEEDBACK = define_operation(
    "feedback.record_query",
    request=QueryFeedbackRequest,
    result=FeedbackView,
    description="Record feedback on a delivered query passage",
)
RECORD_FEEDBACK = define_operation(
    "feedback.record",
    request=RecordFeedbackRequest,
    result=FeedbackView,
    description="Record feedback on an exact record revision",
)
READ_FEEDBACK = define_operation(
    "feedback.read",
    request=ReadFeedback,
    result=FeedbackView,
    description="Read saved feedback",
)
LIST_FEEDBACK = define_operation(
    "feedback.list",
    request=ListFeedback,
    result=tuple[FeedbackView, ...],
    description="List feedback with optional filters",
)
RESOLVE_FEEDBACK = define_operation(
    "feedback.resolve",
    request=ResolveFeedback,
    result=FeedbackView,
    description="Record a resolution for feedback",
)


class FeedbackOperations:
    """Record opinions about exact revisions or delivered query results.

    A namespace-bound instance rejects feedback from other namespaces. Recording a
    new opinion creates a new entry; resolutions are separate durable actions.
    """

    def __init__(
        self,
        database: Database,
        execution: ExecutionManager,
        scope: NamespaceOperations,
        reader: RecordReader,
        wave: WaveOperations,
        clock: Clock,
        *,
        namespace_id: str | None = None,
    ) -> None:
        self.database, self.execution, self.scope = database, execution, scope
        self.reader, self.wave, self.clock, self.namespace_id = reader, wave, clock, namespace_id

    def _ref(self, view: FeedbackView) -> FeedbackRef:
        return FeedbackRef(
            instance_id=self.reader.artifacts.instance_id,
            namespace_id=view.namespace_id,
            id=view.id,
            identifier=view.identifier,
        )

    async def _namespace_in(self, tx: ReadTx, name: str | None = None) -> Namespace:
        selected = (
            name or self.namespace_id or self.execution.snapshot.values.cognition.default_namespace
        )
        namespace = await self.scope.get_in(tx, selected)
        if self.namespace_id is not None and namespace.id != self.namespace_id:
            raise NousError("cognition.namespace_mismatch", "Feedback belongs to another namespace")
        return namespace

    async def _insert_in(
        self,
        tx: WriteTx,
        namespace_id: str,
        target: FeedbackTarget,
        category: Category,
        value: Value | None,
        comment: str | None,
        author: str | None,
    ) -> FeedbackView:
        object_id = str(uuid4())
        recorded = self.clock.utc_now()
        identifier = await insert_identifier(
            tx,
            feedback_schema.feedback,
            "fbk",
            {
                "id": object_id,
                "namespace_id": namespace_id,
                "target": target.model_dump(mode="json"),
                "category": category,
                "value": value,
                "comment": comment,
                "author": author,
                "recorded_at": recorded.isoformat(),
            },
        )
        return FeedbackView(
            id=object_id,
            identifier=identifier,
            namespace_id=namespace_id,
            target=target,
            category=category,
            value=value,
            comment=comment,
            author=author,
            recorded_at=recorded,
        )

    async def _view_in(self, tx: ReadTx, row: RowMapping) -> FeedbackView:
        saved = (
            (
                await tx.connection.execute(
                    sa.select(feedback_schema.resolutions).where(
                        feedback_schema.resolutions.c.feedback_id == row["id"]
                    )
                )
            )
            .mappings()
            .one_or_none()
        )
        resolution = None
        if saved is not None:
            applied = None
            if saved["applied_revision_id"] is not None:
                revision = (
                    (
                        await tx.connection.execute(
                            sa.select(schema.revisions).where(
                                schema.revisions.c.id == saved["applied_revision_id"]
                            )
                        )
                    )
                    .mappings()
                    .one()
                )
                record, actual, _ = await self.reader.resolve_in(
                    tx, f"{revision['record_id']}@{revision['ordinal']}", str(row["namespace_id"])
                )
                applied = self.reader.revision_ref(record, actual)
            resolution = FeedbackResolution.model_validate(
                {
                    "action": saved["action"],
                    "note": saved["note"],
                    "mapping_note": saved["mapping_note"],
                    "applied_revision": applied,
                    "actor": saved["actor"],
                    "resolved_at": saved["resolved_at"],
                }
            )
        return FeedbackView.model_validate({**dict(row), "resolution": resolution})

    async def _get_in(self, tx: ReadTx, ref: str, namespace_id: str) -> RowMapping:
        row = (
            (
                await tx.connection.execute(
                    sa.select(feedback_schema.feedback).where(
                        sa.or_(
                            feedback_schema.feedback.c.id == ref,
                            feedback_schema.feedback.c.identifier == ref,
                        ),
                        feedback_schema.feedback.c.namespace_id == namespace_id,
                    )
                )
            )
            .mappings()
            .one_or_none()
        )
        if row is None:
            raise NousError(
                "cognition.feedback_not_found",
                "Feedback does not exist in this namespace",
                details={"ref": ref},
            )
        return row

    async def record(self, request: RecordFeedbackRequest, call: CallContext) -> FeedbackView:
        """Record an opinion about an exact record revision.

        A string target must include ``@N``. An optional selection must belong to that
        revision's body. The feedback is committed before its view is returned.

        Raises:
            NousError: The revision is not exact or the selection belongs to another
                body or instance.
        """
        target, category = request.target, request.category
        value, comment, author, selection = (
            request.value,
            request.comment,
            request.author,
            request.selection,
        )
        if isinstance(selection, ExternalInput):
            selected = await call.inputs.read_text(
                selection.key, selection.selection, call.settings.values.content.text_read_max_bytes
            )
            selection = ContentSelection.model_validate_json(selected.data)
        owner = (
            NamespaceRef(
                instance_id=target.record_ref.instance_id, id=target.record_ref.namespace_id
            )
            if isinstance(target, RecordRevisionRef)
            else None
        )
        namespace = (await self.scope.resolve_for(request.namespace, owner)).id
        if isinstance(target, str) and "@" not in target:
            raise NousError("input.expected_required", "Feedback requires an exact record revision")
        async with self.execution.operation("feedback.record"):
            if selection is not None:
                await self.reader.artifacts.read_selection(selection)
            async with self.database.write() as tx:
                ns = await self._namespace_in(tx, namespace)
                row, revision, _ = await self.reader.resolve_in(tx, target, ns.id)
                if selection is not None and (
                    selection.artifact.instance_id != self.reader.artifacts.instance_id
                    or selection.artifact.id != revision["body_artifact_id"]
                ):
                    raise NousError(
                        "cognition.selection_mismatch",
                        "Feedback selection belongs to another revision",
                    )
                result = await self._insert_in(
                    tx,
                    ns.id,
                    FeedbackTarget(
                        kind="record",
                        revision=self.reader.revision_ref(row, revision),
                        selection=selection,
                    ),
                    category,
                    value,
                    comment,
                    author,
                )
            ref = self._ref(result)
            record_completed(ref)
            await call.saved(ref)
            return result

    async def _record_query(
        self,
        result: QueryResult,
        category: Category,
        *,
        target: tuple[int, int] | RevisionSelection | None = None,
        value: Value | None = None,
        comment: str | None = None,
        author: str | None = None,
        namespace: str | None = None,
    ) -> FeedbackView:
        """Record an opinion on a query or one of its delivered selections.

        The query context, snapshot and every delivered candidate are checked against
        the selected namespace. ``target`` must be one of the actual deliveries; omit
        it for feedback about the query as a whole.

        Returns:
            The committed feedback and its frozen query context.

        Raises:
            NousError: ``cognition.namespace_mismatch`` for a query, snapshot, or
                delivery from another namespace; ``cognition.query_context_mismatch``
                if the context differs from the snapshot; or
                ``cognition.selection_mismatch`` if a selection was not delivered or
                differs from its snapshot candidate.
        """
        async with self.execution.operation("feedback.record_query"):
            async with self.database.read() as tx:
                ns = await self._namespace_in(tx, namespace or result.context.namespace_id)
            context = result.context
            if context.namespace_id != ns.id or result.namespace.id != ns.id:
                raise NousError(
                    "cognition.namespace_mismatch", "Query belongs to another namespace"
                )
            snapshot = await self.wave.describe(context.snapshot_ref)
            if (
                not isinstance(snapshot.owner, CognitionOwner)
                or snapshot.owner.namespace_id != ns.id
            ):
                raise NousError(
                    "cognition.namespace_mismatch", "Snapshot belongs to another namespace"
                )
            if (
                context.captured_generation != snapshot.captured_generation
                or context.profile_signature != snapshot.profile_signature
                or context.space_signature != snapshot.space_signature
            ):
                raise NousError(
                    "cognition.query_context_mismatch", "Query context differs from snapshot"
                )
            selected = None
            selected_group = None
            if isinstance(target, tuple):
                group_index, passage_index = target
                if group_index < 0 or passage_index < 0:
                    raise NousError("input.invalid_feedback", "Passage indexes must be nonnegative")
                try:
                    selected_group = result.groups[group_index]
                    selected = selected_group.passages[passage_index]
                except IndexError as error:
                    raise NousError(
                        "input.invalid_feedback", "Passage index is out of range"
                    ) from error
            elif target is not None:
                found = [
                    (group, passage)
                    for group in result.groups
                    for passage in group.passages
                    if passage.selection == target
                ]
                if len(found) != 1:
                    raise NousError(
                        "cognition.selection_mismatch", "Selection is not a unique delivery"
                    )
                selected_group, selected = found[0]
            async with self.wave.snapshots.hold_reference(snapshot.ref):
                keys = list(
                    dict.fromkeys(
                        [group.basis.candidate.key for group in result.groups]
                        + [
                            passage.delivery.candidate.key
                            for group in result.groups
                            for passage in group.passages
                        ]
                    )
                )
                descriptors = await self.wave.describe_candidates(snapshot.ref, keys)
                candidates = {item.ref.key: item for item in descriptors}
                groups = await self.wave.describe_groups(
                    snapshot.ref, [group.group_key for group in result.groups]
                )
                for group in result.groups:
                    group_descriptor = groups.get(group.group_key)
                    if (
                        group_descriptor is None
                        or CognitiveGroupContext.model_validate(group_descriptor.context)
                        != group.context
                        or group_descriptor.title != group.title
                        or group.basis.candidate.snapshot != snapshot.ref.identifier
                        or candidates[group.basis.candidate.key].group_key != group.group_key
                    ):
                        raise NousError(
                            "cognition.selection_mismatch", "Group differs from snapshot"
                        )
                    for passage in group.passages:
                        delivery = passage.delivery
                        candidate = candidates[delivery.candidate.key]
                        if (
                            delivery.candidate.snapshot != snapshot.ref.identifier
                            or candidate.group_key != group.group_key
                            or passage.selection.revision != group.context.revision_ref
                            or passage.selection.selections != delivery.selections
                            or (
                                delivery.evidence is not None
                                and delivery.evidence.candidate != delivery.candidate
                            )
                        ):
                            raise NousError(
                                "cognition.selection_mismatch", "Passage differs from snapshot"
                            )
                        ranges = await self.reader.artifacts.normalize_selections(
                            (candidate.content, *passage.selection.selections)
                        )
                        base, *parts = ranges
                        if any(
                            part.artifact != base.artifact
                            or part.selection.start_byte is None
                            or part.selection.end_byte is None
                            or base.selection.start_byte is None
                            or base.selection.end_byte is None
                            or part.selection.start_byte < base.selection.start_byte
                            or part.selection.end_byte > base.selection.end_byte
                            for part in parts
                        ):
                            raise NousError(
                                "cognition.selection_mismatch", "Passage exceeds its candidate"
                            )
                        actual = await self.reader.artifacts.read_selections(parts)
                        if any(
                            value.text != fragment.text
                            for value, fragment in zip(actual, delivery.fragments, strict=True)
                        ):
                            raise NousError(
                                "cognition.selection_mismatch",
                                "Passage text differs from its selection",
                            )
                async with self.database.write() as tx:
                    saved = await self._insert_in(
                        tx,
                        ns.id,
                        FeedbackTarget(
                            kind="query",
                            query=QueryFeedback(
                                context=result.context,
                                operation_context=result.operation_context,
                                group_rank=selected_group.rank if selected_group else None,
                                basis=selected_group.basis if selected_group else None,
                                candidate=selected.delivery.candidate if selected else None,
                            ),
                            revision=selected.selection.revision if selected else None,
                            selection=selected.selection if selected else None,
                        ),
                        category,
                        value,
                        comment,
                        author,
                    )
                    await self.wave.retain_in(
                        tx, snapshot.ref, owner_kind="feedback", owner_id=saved.id
                    )
            record_completed(self._ref(saved))
            return saved

    async def record_query(self, request: QueryFeedbackRequest, call: CallContext) -> FeedbackView:
        result = request.result
        if isinstance(result, ExternalInput):
            selected = await call.inputs.read_text(
                result.key, result.selection, call.settings.values.content.text_read_max_bytes
            )
            result = QueryResult.model_validate_json(selected.data)
        namespace = await self.scope.resolve_for(
            request.namespace,
            NamespaceRef(
                instance_id=result.context.snapshot_ref.instance_id, id=result.context.namespace_id
            ),
        )
        saved = await self._record_query(
            result,
            request.category,
            target=request.target,
            value=request.value,
            comment=request.comment,
            author=request.author,
            namespace=namespace.id,
        )
        await call.saved(self._ref(saved))
        return saved

    async def read(self, request: ReadFeedback, call: CallContext) -> FeedbackView:
        """Read feedback by ID or identifier, including its resolution if present."""
        async with self.execution.operation("feedback.read"):
            namespace = (await self.scope.resolve_for(request.namespace)).id
            async with self.database.read() as tx:
                ns = await self._namespace_in(tx, namespace)
                return await self._view_in(tx, await self._get_in(tx, request.ref, ns.id))

    async def list(self, request: ListFeedback, call: CallContext) -> tuple[FeedbackView, ...]:
        """List feedback newest first within the selected namespace.

        A target filters by record identity across revisions. Limit must be 1 through
        200 and offset nonnegative; ``unresolved_only`` excludes resolved entries.
        """
        target, category = request.target, request.category
        unresolved_only, limit, offset = request.unresolved_only, request.limit, request.offset
        target_ref = target.record_ref if isinstance(target, RecordRevisionRef) else target
        owner = (
            NamespaceRef(instance_id=target_ref.instance_id, id=target_ref.namespace_id)
            if isinstance(target_ref, RecordRef)
            else None
        )
        namespace = (await self.scope.resolve_for(request.namespace, owner)).id
        if not 1 <= limit <= 200 or offset < 0:
            raise NousError(
                "input.invalid_page", "Feedback page must have a valid limit and offset"
            )
        async with self.execution.operation("feedback.list"):
            async with self.database.read() as tx:
                ns = await self._namespace_in(tx, namespace)
                query = sa.select(feedback_schema.feedback).where(
                    feedback_schema.feedback.c.namespace_id == ns.id
                )
                if target is not None:
                    record, _, _ = await self.reader.resolve_in(tx, target, ns.id)
                    query = query.where(
                        sa.func.json_extract(
                            feedback_schema.feedback.c.target, "$.revision.record_ref.id"
                        )
                        == record["id"]
                    )
                if category is not None:
                    query = query.where(feedback_schema.feedback.c.category == category)
                if unresolved_only:
                    query = query.where(
                        ~sa.exists().where(
                            feedback_schema.resolutions.c.feedback_id
                            == feedback_schema.feedback.c.id
                        )
                    )
                rows = (
                    (
                        await tx.connection.execute(
                            query.order_by(feedback_schema.feedback.c.recorded_at.desc())
                            .limit(limit)
                            .offset(offset)
                        )
                    )
                    .mappings()
                    .all()
                )
                return tuple([await self._view_in(tx, row) for row in rows])

    async def resolve(self, request: ResolveFeedback, call: CallContext) -> FeedbackView:
        """Record an acknowledged or not-applied disposition for feedback.

        Repeating the same action, note and actor returns the existing resolution.

        Returns:
            The feedback view including its saved resolution.

        Raises:
            NousError: ``cognition.feedback_not_found`` if the reference is absent in
                the namespace; ``cognition.feedback_already_resolved`` if an existing
                resolution differs from the requested one.
        """
        async with self.execution.operation("feedback.resolve"):
            namespace = (await self.scope.resolve_for(request.namespace)).id
            async with self.database.write() as tx:
                ns = await self._namespace_in(tx, namespace)
                row = await self._get_in(tx, request.ref, ns.id)
                await self._save_resolution_in(
                    tx, row, request.action, note=request.note, actor=request.actor
                )
                result = await self._view_in(tx, row)
            ref = self._ref(result)
            record_completed(ref)
            await call.saved(ref)
            return result

    async def _save_resolution_in(
        self,
        tx: WriteTx,
        row: RowMapping,
        action: ResolutionAction,
        *,
        applied_revision: RecordRevisionRef | None = None,
        note: str | None = None,
        actor: str | None = None,
        mapping_note: str | None = None,
    ) -> None:
        existing = (
            (
                await tx.connection.execute(
                    sa.select(feedback_schema.resolutions).where(
                        feedback_schema.resolutions.c.feedback_id == row["id"]
                    )
                )
            )
            .mappings()
            .one_or_none()
        )
        applied_id = applied_revision.revision_id if applied_revision else None
        if existing is not None:
            if (
                existing["action"],
                existing["applied_revision_id"],
                existing["note"],
                existing["actor"],
                existing["mapping_note"],
            ) == (action, applied_id, note, actor, mapping_note):
                return
            raise NousError(
                "cognition.feedback_already_resolved",
                "Feedback has already been resolved",
                details={"ref": row["identifier"]},
            )
        await tx.connection.execute(
            feedback_schema.resolutions.insert().values(
                feedback_id=row["id"],
                action=action,
                note=note,
                actor=actor,
                mapping_note=mapping_note,
                applied_revision_id=applied_id,
                resolved_at=self.clock.utc_now().isoformat(),
            )
        )

    @staticmethod
    def _mapping_note(
        target: FeedbackTarget, ref: str, base: RecordRevisionRef, mappings: Mapping[str, str]
    ) -> str | None:
        if target.kind == "query" and target.selection is None or target.revision is None:
            raise NousError(
                "cognition.feedback_target_mismatch",
                "Feedback does not target an exact delivered record",
                details={"ref": ref},
            )
        note = mappings.get(ref)
        if note is not None and not note.strip():
            raise NousError(
                "input.invalid_feedback_mapping", "Feedback mapping notes must not be empty"
            )
        if target.revision == base:
            return note
        if target.revision.record_ref != base.record_ref or note is None:
            raise NousError(
                "cognition.feedback_target_mismatch",
                "Feedback for another revision requires an explicit mapping note for the same record",
                details={"ref": ref},
            )
        return note

    async def select_for_edit(
        self,
        refs: Sequence[str],
        base_revision: RecordRevisionRef,
        *,
        mappings: Mapping[str, str] | None = None,
    ) -> tuple[FeedbackView, ...]:
        """Freeze selected feedback for an exact base revision in one read view.

        Duplicate references are removed in input order. Feedback on another revision
        of the same record requires a nonempty mapping note keyed by its reference;
        feedback about a whole query cannot be used as a record edit target.
        """
        selected = tuple(dict.fromkeys(refs))
        notes = dict(mappings or {})
        if set(notes) - set(selected):
            raise NousError(
                "input.invalid_feedback_mapping", "A mapping must refer to selected feedback"
            )
        async with self.execution.operation("feedback.select_for_edit"):
            async with self.database.read() as tx:
                ns = await self._namespace_in(tx, base_revision.record_ref.namespace_id)
                output: list[FeedbackView] = []
                for ref in selected:
                    row = await self._get_in(tx, ref, ns.id)
                    view = await self._view_in(tx, row)
                    self._mapping_note(view.target, ref, base_revision, notes)
                    output.append(view)
                return tuple(output)

    async def resolve_in(
        self,
        tx: WriteTx,
        refs: Sequence[str],
        *,
        namespace_id: str,
        base_revision: RecordRevisionRef,
        applied_revision: RecordRevisionRef,
        note: str | None = None,
        actor: str | None = None,
        mappings: Mapping[str, str] | None = None,
    ) -> None:
        """Mark selected feedback applied in the caller's write transaction.

        The base and applied revisions must belong to the same record and namespace.
        Feedback from another revision requires a mapping note. Repeating an identical
        resolution is accepted; a different existing resolution raises an error.
        """
        if self.namespace_id is not None and self.namespace_id != namespace_id:
            raise NousError("cognition.namespace_mismatch", "Feedback belongs to another namespace")
        if (
            base_revision.record_ref.namespace_id != namespace_id
            or applied_revision.record_ref != base_revision.record_ref
        ):
            raise NousError(
                "cognition.namespace_mismatch",
                "Feedback and revisions must share a record and namespace",
            )
        selected = tuple(dict.fromkeys(refs))
        notes = dict(mappings or {})
        if set(notes) - set(selected):
            raise NousError(
                "input.invalid_feedback_mapping", "A mapping must refer to selected feedback"
            )
        for ref in selected:
            row = await self._get_in(tx, ref, namespace_id)
            target = FeedbackTarget.model_validate(row["target"])
            mapping_note = self._mapping_note(target, ref, base_revision, notes)
            await self._save_resolution_in(
                tx,
                row,
                "applied",
                applied_revision=applied_revision,
                note=note,
                actor=actor,
                mapping_note=mapping_note,
            )
