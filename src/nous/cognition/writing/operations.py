"""Complete namespace-bound record writes, exact edits and post-commit maintenance."""

from __future__ import annotations

import difflib
import hashlib
import json
from collections.abc import Mapping, Sequence
from typing import TYPE_CHECKING, Literal, TypedDict, Unpack, cast

import sqlalchemy as sa
from pydantic_core import to_jsonable_python

from nous.common.errors import NousError
from nous.common.operations import (
    OperationIssue,
    add_error_context,
    record_completed,
    report_outcome,
)
from nous.content.citations import normalize_source_links
from nous.content.contracts import ArtifactRef, ContentSelection
from nous.content.edits import TextChange, prepare_text_edit
from nous.temporal.values import CognitiveTimes

from .. import schema
from ..contracts import (
    NamespaceRef,
    RecordRevisionRef,
    RecordView,
    RevisionSelection,
    RevisionSummary,
    SourceBinding,
)
from .contracts import (
    DEFAULT_AFTER_WRITE,
    AfterWrite,
    CommittedRecord,
    DraftOverride,
    DraftRef,
    OperationLink,
    RecordOutcome,
    RevisionDiff,
    SourceChanges,
    SourceRequest,
    StageOutcome,
)
from .drafts import DraftStore
from .reading import RecordReader, Target
from .records import CreateMetadata, RevisionMetadata, WrittenRecords

if TYPE_CHECKING:
    from nous.jobs.contracts import JobRef

    from ..annotation.operations import AnnotationOperations
    from ..feedback import FeedbackOperations
    from ..maintenance import MaintenanceOperations
    from ..maintenance.contracts import MaintenancePlan
    from ..maintenance.operations import EnqueuedMaintenance
    from ..retrieval.operations import CognitiveRetrieval


class AdoptionValues(TypedDict):
    title: str | None
    purpose: str | None
    author: str | None
    viewpoint: str | None
    sources: Sequence[SourceBinding]
    times: CognitiveTimes
    origin_result_id: str
    body_artifact: ArtifactRef | None


class RecordOperations:
    """Complete formal record operations within one bound namespace.

    Writes publish body bytes before committing the record and any related feedback.
    Requested annotation and retrieval run after that commit. Inspect RecordOutcome:
    ``partial`` means the record was committed but a follow-up failed. Retry the
    failed stage using the returned revision, not the original create operation.
    """

    def __init__(
        self,
        namespace: NamespaceRef,
        core: WrittenRecords,
        reader: RecordReader,
        drafts: DraftStore,
        annotations: AnnotationOperations,
        retrieval: CognitiveRetrieval,
        feedback: FeedbackOperations,
        maintenance: MaintenanceOperations,
    ):
        self.namespace, self.core, self.reader, self.drafts = namespace, core, reader, drafts
        self.annotations, self.retrieval, self.feedback = annotations, retrieval, feedback
        self.maintenance = maintenance
        self.execution, self.database = reader.execution, reader.database

    async def finish_write(
        self,
        record: CommittedRecord,
        plan: MaintenancePlan,
        operation: OperationLink,
        *,
        work: EnqueuedMaintenance | None = None,
    ) -> RecordOutcome:
        """Report the committed record and execute its already-decided inline work."""
        record_completed(record.revision_ref.identifier)
        annotation = StageOutcome(state="not_requested")
        retrieval = StageOutcome(state="pending" if record.in_recall else "not_requested")
        issues: list[OperationIssue] = []
        for item in plan.unavailable if record.changed else ():
            issue = OperationIssue(
                item.stage,
                item.code,
                item.message,
                item.details,
                item.retryable,
                (record.revision_ref.identifier,),
                (record.revision_ref.identifier,),
            )
            issues.append(issue)
            if issue.stage == "annotation":
                annotation = StageOutcome(state="failed", issue=issue)
            else:
                retrieval = StageOutcome(state="failed", issue=issue)
        if record.changed and plan.execution == "background":
            if work is not None and work.annotation is not None:
                annotation = StageOutcome(state="pending", job=work.annotation)
            if work is not None and work.retrieval is not None:
                retrieval = StageOutcome(state="pending", job=work.retrieval)
        elif record.changed:
            if plan.annotation is not None:
                try:
                    adopted = await self.maintenance.annotate_inline(record, plan.annotation)
                    annotation = StageOutcome(
                        state="ready", generation=adopted.generation, ref=adopted.ref.identifier
                    )
                except NousError as error:
                    issue = OperationIssue(
                        "annotation",
                        error.code,
                        error.message,
                        dict(error.details),
                        error.retryable,
                        (record.revision_ref.identifier,),
                        (record.revision_ref.identifier,),
                    )
                    issues.append(issue)
                    annotation = StageOutcome(state="failed", issue=issue)
            if plan.refresh is not None:
                try:
                    outcome = await self.maintenance.refresh(
                        record.namespace.ref,
                        record.namespace.content_generation,
                        plan.refresh,
                        build_key=f"inline:{record.revision_ref.revision_id}",
                    )
                    retrieval = StageOutcome(
                        state="ready" if outcome.state == "ready" else "pending",
                        generation=outcome.covered_generation,
                        ref=outcome.result_refs[0] if outcome.result_refs else None,
                    )
                except NousError as error:
                    issue = OperationIssue(
                        "retrieval",
                        error.code,
                        error.message,
                        dict(error.details),
                        error.retryable,
                        (record.revision_ref.identifier,),
                        (record.revision_ref.identifier,),
                    )
                    issues.append(issue)
                    retrieval = StageOutcome(state="failed", issue=issue)
        status = "partial" if issues else "success"
        report_outcome(status, issues=issues, completed_refs=(record.revision_ref.identifier,))
        return RecordOutcome(
            record=record,
            status=status,
            annotation=annotation,
            retrieval=retrieval,
            operation=operation,
            jobs=work.jobs if work is not None else (),
        )

    def _link(self) -> OperationLink:
        from nous.common.operations import current_operation

        frame = current_operation.get()
        assert frame is not None
        return OperationLink(operation_id=frame.root.operation_id, stage=frame.stage)

    async def _resolve_requests(
        self, requests: Sequence[SourceRequest], *, prefix: str = "S", reserved: Sequence[str] = ()
    ) -> tuple[SourceBinding, ...]:
        selected = await self.core.sources.prepare(
            self.namespace, requests, prefix=prefix, reserved_keys=reserved
        )
        return selected.bindings

    async def _create(
        self,
        body: str | ArtifactRef,
        *,
        kind: Literal["note", "journal"],
        recall: bool,
        after: AfterWrite,
        metadata: CreateMetadata,
        source_requests: Sequence[SourceRequest],
    ) -> RecordOutcome:
        if isinstance(body, ArtifactRef):
            body = await self.core.artifacts.read_text(body)
        if source_requests:
            if metadata.get("sources"):
                raise NousError(
                    "input.conflicting_sources", "Choose frozen bindings or source requests"
                )
            metadata["sources"] = await self._resolve_requests(source_requests)
        elif existing_sources := metadata.get("sources"):
            prepared = await self.core.sources.prepare(
                self.namespace, existing_sources, preserve_keys=True
            )
            metadata["sources"] = prepared.bindings
        self.core.validate_body(body, metadata.get("sources", ()))
        times = await self.core.prepare_times(metadata.get("times"))
        preparation = await self.maintenance.prepare_after(after, in_recall=recall)
        published = await self.core.artifacts.content.publish_bytes(body.encode("utf-8"))
        async with self.database.write() as tx:
            committed = await self.core.create_in(
                tx,
                self.namespace.id,
                body,
                published,
                kind=kind,
                recall=recall,
                times=times,
                title=metadata.get("title"),
                purpose=metadata.get("purpose"),
                author=metadata.get("author"),
                viewpoint=metadata.get("viewpoint"),
                sources=metadata.get("sources", ()),
            )
            jobs = await self.maintenance.enqueue_in(tx, committed, preparation)
        return await self.finish_write(
            committed,
            preparation,
            self._link(),
            work=jobs,
        )

    async def add_note(
        self,
        body: str | ArtifactRef,
        *,
        remember: bool | None = None,
        source_requests: Sequence[SourceRequest] = (),
        after: AfterWrite = DEFAULT_AFTER_WRITE,
        **metadata: Unpack[CreateMetadata],
    ) -> RecordOutcome:
        """Create a Note from Markdown or a text artifact in the bound namespace.

        ``remember`` controls recall membership; None uses the configured default.
        Supply either frozen source bindings in metadata or source requests, not both.

        Returns:
            The committed revision and the states of requested follow-up work.

        Raises:
            NousError: ``input.conflicting_sources`` if both source forms are supplied;
                ``input.empty_body``, ``input.duplicate_source``, or
                ``cognition.source_unknown`` if the body or bindings are invalid.
        """
        async with self.execution.operation("note.add"):
            recall = (
                self.execution.snapshot.values.cognition.notes.default_recall
                if remember is None
                else remember
            )
            return await self._create(
                body,
                kind="note",
                recall=recall,
                after=after,
                metadata=metadata,
                source_requests=source_requests,
            )

    async def create_journal(
        self,
        body: str | ArtifactRef,
        *,
        after: AfterWrite = DEFAULT_AFTER_WRITE,
        source_requests: Sequence[SourceRequest] = (),
        **metadata: Unpack[CreateMetadata],
    ) -> RecordOutcome:
        """Create a recallable Journal from Markdown or a text artifact.

        Purpose defaults to ``record``. Supply either frozen source bindings in metadata
        or source requests, not both.

        Returns:
            The committed revision and the states of requested follow-up work.

        Raises:
            NousError: ``input.conflicting_sources`` if both source forms are supplied;
                ``input.empty_body``, ``input.duplicate_source``, or
                ``cognition.source_unknown`` if the body or bindings are invalid.
        """
        async with self.execution.operation("journal.create"):
            metadata.setdefault("purpose", "record")
            return await self._create(
                body,
                kind="journal",
                recall=True,
                after=after,
                metadata=metadata,
                source_requests=source_requests,
            )

    async def read(
        self, target: Target, *, sources: bool = False, blocks: Sequence[str] = ()
    ) -> RecordView:
        """Read a record or exact revision in the bound namespace.

        Unqualified records select the head. ``blocks`` restricts body text to the
        requested block keys; ``sources`` also loads source content views.
        """
        return await self.reader.read(
            target, namespace=self.namespace.id, sources=sources, blocks=blocks
        )

    async def _base(self, base: str | RecordRevisionRef) -> RecordView:
        if isinstance(base, str) and "@" not in base:
            raise NousError("input.expected_required", "An exact revision with @N is required")
        return await self.read(base)

    async def _target_base(
        self, target: Target, expected: str | int | RecordRevisionRef | None
    ) -> RecordView:
        async with self.database.read() as tx:
            row, revision, _ = await self.core.resolve_expected_in(
                tx, target, expected, self.namespace.id
            )
            exact = self.reader.revision_ref(row, revision)
        return await self.read(exact)

    async def _replace(
        self,
        base: Target,
        body: str,
        *,
        expected: str | int | RecordRevisionRef | None,
        after: AfterWrite,
        feedback_refs: Sequence[str],
        feedback_mappings: Mapping[str, str],
        intent: Literal["edit", "correction"],
        reason: str | None,
        metadata: RevisionMetadata,
        source_changes: SourceChanges | None,
        write_key: str | None = None,
        write_fingerprint: str | None = None,
    ) -> RecordOutcome:
        original = await self._target_base(base, expected)
        bindings = (
            source_changes.apply_to(original.sources)
            if source_changes is not None
            else original.sources
        )
        if source_changes is not None and source_changes.add:
            prepared = await self.core.sources.prepare(
                self.namespace,
                source_changes.add,
                prefix="N",
                reserved_keys=[source.key for source in original.sources],
                preserve_keys=True,
            )
            bindings = source_changes.model_copy(update={"add": prepared.bindings}).apply_to(
                original.sources
            )
        changed_times = metadata.get("times")
        if changed_times is not None:
            await self.core.time.validate([changed_times])
        published = await self.core.artifacts.content.publish_bytes(body.encode("utf-8"))
        async with self.database.read() as tx:
            proposed = await self.core.prepare_revision_in(
                tx,
                base,
                expected,
                body,
                published,
                namespace=self.namespace.id,
                sources=bindings,
                **metadata,
            )
        expected_recall = bool(proposed.row["recall"] and not proposed.row["archived"])
        preparation = await self.maintenance.prepare_after(
            after, in_recall=expected_recall, changed=proposed.changed
        )
        replayed: CommittedRecord | None = None
        committed: CommittedRecord | None = None
        jobs: EnqueuedMaintenance | None = None
        async with self.database.write() as tx:
            if write_key is not None:
                assert write_fingerprint is not None
                replayed = await self.core.replay_in(
                    tx, base, expected, self.namespace.id, write_key, write_fingerprint
                )
            if replayed is None:
                committed = await self.core.append_in(
                    tx,
                    base,
                    expected,
                    body,
                    published,
                    namespace=self.namespace.id,
                    change_kind=intent,
                    reason=reason,
                    sources=bindings,
                    write_key=write_key,
                    write_fingerprint=write_fingerprint,
                    **metadata,
                )
                if (
                    committed.changed
                    and preparation.recall_precondition is not None
                    and committed.in_recall != preparation.recall_precondition
                ):
                    raise NousError(
                        "cognition.maintenance_conflict",
                        "Recall membership changed while maintenance was prepared",
                    )
                if feedback_refs:
                    await self.feedback.resolve_in(
                        tx,
                        feedback_refs,
                        namespace_id=self.namespace.id,
                        base_revision=original.revision_ref,
                        applied_revision=committed.revision_ref,
                        note=reason,
                        mappings=feedback_mappings,
                    )
                jobs = await self.maintenance.enqueue_in(tx, committed, preparation)
        if replayed is not None:
            return await self._replayed(replayed, after)
        assert committed is not None
        return await self.finish_write(
            committed,
            preparation,
            self._link(),
            work=jobs,
        )

    async def _replayed(self, record: CommittedRecord, after: AfterWrite) -> RecordOutcome:
        """Read current maintenance facts without repeating the committed write."""
        annotation = StageOutcome(state="not_requested")
        issues: list[OperationIssue] = []
        jobs: list[JobRef] = []
        if after.annotation != "skip":
            async with self.database.read() as tx:
                active = await self.annotations.read_many_in(tx, [record.revision_ref])
            view = active.get(record.revision_ref.revision_id)
            if view is not None:
                annotation = StageOutcome(
                    state="ready", ref=view.ref.identifier, generation=view.generation
                )
            else:
                job = await self.maintenance.record_annotation_job(record.revision_ref)
                if job is not None:
                    jobs.append(job.ref)
                if job is not None and job.status in {
                    "ERROR",
                    "CANCELLED",
                    "MAX_RECOVERY_ATTEMPTS_EXCEEDED",
                }:
                    issue = job.error or OperationIssue(
                        "annotation", "job." + job.status.lower(), f"Job ended in {job.status}"
                    )
                    issues.append(issue)
                    annotation = StageOutcome(state="failed", issue=issue, job=job.ref)
                elif job is not None and job.status in {"ENQUEUED", "PENDING", "DELAYED"}:
                    annotation = StageOutcome(state="pending", job=job.ref)
                elif job is not None or after.annotation == "run":
                    annotation = StageOutcome(state="pending")

        requested = after.retrieval == "refresh" or after.retrieval_mode is not None
        retrieval = StageOutcome(
            state="pending" if record.in_recall or requested else "not_requested"
        )
        if record.in_recall or requested:
            status = await self.retrieval.status(mode=after.retrieval_mode)
            namespaces = cast(list[dict[str, object]], status["namespaces"])
            current = next(
                (value for value in namespaces if value["namespace_id"] == record.namespace.id),
                None,
            )
            if current is not None:
                covered = current["covered_generation"]
                snapshot = current["snapshot"]
                if isinstance(covered, int) and covered >= record.committed_generation:
                    ref = cast(dict[str, object], snapshot) if isinstance(snapshot, dict) else {}
                    if isinstance(ref.get("identifier"), str):
                        retrieval = StageOutcome(
                            state="ready", generation=covered, ref=cast(str, ref["identifier"])
                        )
            if requested:
                job = await self.maintenance.record_refresh_job(record)
                if job is not None:
                    jobs.append(job.ref)
                    if retrieval.state != "ready" and job.status in {
                        "ERROR",
                        "CANCELLED",
                        "MAX_RECOVERY_ATTEMPTS_EXCEEDED",
                    }:
                        issue = job.error or OperationIssue(
                            "retrieval",
                            "job." + job.status.lower(),
                            f"Job ended in {job.status}",
                        )
                        issues.append(issue)
                        retrieval = StageOutcome(state="failed", issue=issue, job=job.ref)
                    elif retrieval.state != "ready" and job.status in {
                        "ENQUEUED",
                        "PENDING",
                        "DELAYED",
                    }:
                        retrieval = StageOutcome(state="pending", job=job.ref)
        return RecordOutcome(
            record=record,
            status="partial" if issues else "success",
            annotation=annotation,
            retrieval=retrieval,
            operation=self._link(),
            jobs=tuple(jobs),
        )

    async def replace(
        self,
        target: Target,
        markdown: str,
        *,
        expected: str | int | RecordRevisionRef | None = None,
        reason: str | None = None,
        intent: Literal["edit", "correction"] = "edit",
        after: AfterWrite = DEFAULT_AFTER_WRITE,
        feedback_refs: Sequence[str] = (),
        feedback_mappings: Mapping[str, str] | None = None,
        source_requests: Sequence[SourceRequest] | None = None,
        sources: SourceChanges | None = None,
        write_key: str | None = None,
        **metadata: Unpack[RevisionMetadata],
    ) -> RecordOutcome:
        """Replace the complete Markdown body against an exact base revision.

        Sources are inherited unless changed; source requests add frozen bindings.
        Selected feedback is resolved in the revision transaction. A correction
        invalidates affected current-basis relationships.

        Returns:
            The committed record and follow-up states. An unchanged write reports the
            existing revision with ``changed`` false.

        Raises:
            NousError: ``input.expected_required`` if a string base lacks ``@N``;
                ``cognition.revision_conflict`` if the base is no longer current;
                ``cognition.feedback_target_mismatch`` if feedback cannot be applied
                to the base; or a body or source validation error.
        """
        async with self.execution.operation("records.replace"):
            fingerprint = None
            if write_key is not None:
                if not write_key:
                    raise NousError("input.invalid", "Write key must not be empty")
                request = to_jsonable_python(
                    {
                        "body": markdown,
                        "reason": reason,
                        "intent": intent,
                        "after": after,
                        "feedback_refs": feedback_refs,
                        "feedback_mappings": feedback_mappings or {},
                        "source_requests": source_requests,
                        "sources": sources,
                        "metadata": metadata,
                    }
                )
                fingerprint = hashlib.sha256(
                    json.dumps(request, sort_keys=True, separators=(",", ":")).encode()
                ).hexdigest()

            async def replay() -> CommittedRecord | None:
                if write_key is None:
                    return None
                assert fingerprint is not None
                async with self.database.read() as tx:
                    committed = await self.core.replay_in(
                        tx, target, expected, self.namespace.id, write_key, fingerprint
                    )
                return committed

            existing = await replay()
            if existing is not None:
                return await self._replayed(existing, after)
            try:
                if source_requests is not None:
                    original = await self._target_base(target, expected)
                    additions = await self._resolve_requests(
                        source_requests,
                        prefix="N",
                        reserved=[source.key for source in original.sources]
                        + [source.key for source in sources.add]
                        if sources
                        else [source.key for source in original.sources],
                    )
                    changes = sources or SourceChanges()
                    sources = changes.model_copy(update={"add": (*changes.add, *additions)})
                return await self._replace(
                    target,
                    markdown,
                    expected=expected,
                    after=after,
                    feedback_refs=feedback_refs,
                    feedback_mappings=feedback_mappings or {},
                    intent=intent,
                    reason=reason,
                    metadata=metadata,
                    source_changes=sources,
                    write_key=write_key,
                    write_fingerprint=fingerprint,
                )
            except NousError as error:
                if error.code == "cognition.revision_conflict":
                    existing = await replay()
                    if existing is not None:
                        return await self._replayed(existing, after)
                raise

    async def apply(
        self,
        target: Target,
        changes: Sequence[TextChange],
        *,
        expected: str | int | RecordRevisionRef | None = None,
        reason: str | None = None,
        intent: Literal["edit", "correction"] = "edit",
        sources: SourceChanges | None = None,
        feedback_refs: Sequence[str] = (),
        feedback_mappings: Mapping[str, str] | None = None,
        after: AfterWrite = DEFAULT_AFTER_WRITE,
    ) -> RecordOutcome:
        """Apply exact text changes to a frozen base and commit the resulting revision.

        Changes are interpreted against the exact body. Source changes and feedback
        resolution share the revision transaction; post-commit maintenance is separate.

        Returns:
            The committed record and follow-up states. An unchanged write reports the
            existing revision with ``changed`` false.

        Raises:
            NousError: ``input.expected_required`` if a string base lacks ``@N``;
                ``cognition.revision_conflict`` if the base is no longer current; or a
                text edit, source, or feedback validation error.
        """
        async with self.execution.operation("records.apply"):
            original = await self._target_base(target, expected)
            view = await self.core.artifacts.text_view(original.body_artifact)
            edited = prepare_text_edit(view, changes)
            return await self._replace(
                target,
                edited.utf8_bytes.decode("utf-8"),
                expected=expected,
                after=after,
                feedback_refs=feedback_refs,
                feedback_mappings=feedback_mappings or {},
                intent=intent,
                reason=reason,
                metadata={},
                source_changes=sources,
            )

    async def diff(
        self, left: str | RecordRevisionRef, right: str | RecordRevisionRef
    ) -> RevisionDiff:
        """Return a unified body diff between two exact revisions.

        String references must include ``@N``; this operation does not write records.
        """
        async with self.execution.operation("records.diff"):
            before, after = await self._base(left), await self._base(right)
            return RevisionDiff(
                left=before.revision_ref,
                right=after.revision_ref,
                unified="".join(
                    difflib.unified_diff(
                        before.body.splitlines(keepends=True),
                        after.body.splitlines(keepends=True),
                        fromfile=before.revision_ref.identifier,
                        tofile=after.revision_ref.identifier,
                    )
                ),
            )

    async def _membership(
        self,
        target: Target,
        *,
        expected: str | int | RecordRevisionRef | None = None,
        archived: bool | None = None,
        recall: bool | None = None,
        after: AfterWrite,
    ) -> RecordOutcome:
        preparation = await self.maintenance.prepare_after(after, annotate=False)
        async with self.database.write() as tx:
            committed = await self.core.membership_in(
                tx,
                target,
                expected,
                namespace=self.namespace.id,
                archived=archived,
                recall=recall,
            )
            jobs = await self.maintenance.enqueue_in(tx, committed, preparation)
        return await self.finish_write(
            committed,
            preparation,
            self._link(),
            work=jobs,
        )

    async def archive(
        self,
        target: Target,
        *,
        expected: str | int | RecordRevisionRef | None = None,
        after: AfterWrite = DEFAULT_AFTER_WRITE,
    ) -> RecordOutcome:
        """Archive a record against its exact current base and report follow-up state."""
        async with self.execution.operation("records.archive"):
            return await self._membership(target, expected=expected, archived=True, after=after)

    async def restore(
        self,
        target: Target,
        *,
        expected: str | int | RecordRevisionRef | None = None,
        after: AfterWrite = DEFAULT_AFTER_WRITE,
    ) -> RecordOutcome:
        """Restore an archived record against its exact current base."""
        async with self.execution.operation("records.restore"):
            return await self._membership(target, expected=expected, archived=False, after=after)

    async def set_recall(
        self,
        target: Target,
        enabled: bool,
        *,
        expected: str | int | RecordRevisionRef | None = None,
        after: AfterWrite = DEFAULT_AFTER_WRITE,
    ) -> RecordOutcome:
        """Set a Note's recall membership against its exact current base.

        Raises:
            NousError: The base is stale or the target is not a Note.
        """
        async with self.execution.operation("records.recall"):
            return await self._membership(target, expected=expected, recall=enabled, after=after)

    async def history(self, target: Target) -> tuple[RevisionSummary, ...]:
        """Return the record's revision summaries in ascending ordinal order."""
        return await self.reader.history(target, namespace=self.namespace.id)

    async def list(
        self,
        *,
        kind: Literal["note", "journal"] | None = None,
        archived: bool = False,
        offset: int = 0,
        limit: int | None = None,
    ) -> tuple[CommittedRecord, ...]:
        """List current heads in identifier order using the selected kind and archive state.

        An omitted limit uses the configured page size. Offset must be nonnegative and
        limit must be between 1 and 200.
        """
        return await self.reader.list(
            self.namespace.id, kind=kind, archived=archived, offset=offset, limit=limit
        )

    async def select_sources(
        self,
        targets: Sequence[Target | ArtifactRef | ContentSelection | RevisionSelection],
        *,
        usage: Literal["reference", "current_basis", "inspiration"] = "reference",
    ) -> tuple[SourceBinding, ...]:
        """Freeze record, artifact or content targets as ordered source bindings.

        Logical record targets resolve to exact revisions in one read view; the returned
        bindings use S1, S2, and subsequent keys and the requested usage.
        """
        selected = await self.core.sources.prepare(self.namespace, targets, usage=usage)
        return selected.bindings

    async def adopt(
        self,
        target: str | DraftRef,
        *,
        override: DraftOverride | None = None,
        title: str | None = None,
        fork: bool = False,
        feedback_refs: Sequence[str] = (),
        after: AfterWrite = DEFAULT_AFTER_WRITE,
    ) -> RecordOutcome:
        """Adopt a saved writing draft into a formal record in the bound namespace.

        An edit or rewrite uses the frozen exact base, which must still be current;
        new and reflection drafts create a Journal. ``override`` supplies replacement
        Markdown and optional source changes. ``fork`` creates a new Journal and a
        local edit requires a complete Markdown override. Selected feedback and draft
        adoption are committed with the record; follow-up stages run after commit.

        Repeating a non-fork adoption returns the previously adopted revision when the
        requested content, source changes and title agree. After an adoption failure,
        the saved draft remains available for retry without another model call.

        Returns:
            The committed record and its requested follow-up states. A repeated adoption
            of the same draft reports the existing revision with ``changed`` false.

        Raises:
            NousError: ``cognition.draft_fork_requires_fulltext`` for a local edit fork
                without an override; ``cognition.revision_conflict`` for a stale base;
                ``cognition.draft_already_adopted`` for a differing repeat adoption;
                or ``cognition.source_link_unqualified`` for unqualified known links.
        """
        async with self.execution.operation("records.adopt"):
            draft = await self.drafts.read(target, namespace=self.namespace.id, sources=False)
            if fork and draft.context.intent == "edit" and override is None:
                raise NousError(
                    "cognition.draft_fork_requires_fulltext",
                    "Forking a local edit requires an explicit complete Markdown override",
                )
            text = draft.body if override is None else override.markdown
            bindings = (
                draft.context.source_bindings
                if override is None or override.source_changes is None
                else override.source_changes.apply_to(draft.context.source_bindings)
            )
            if bindings:
                prepared = await self.core.sources.prepare(
                    self.namespace, bindings, preserve_keys=True
                )
                bindings = prepared.bindings
            if override is None and (draft.adopted_revision is None or fork):
                previous = (
                    await self.reader.read(draft.context.base_revision, namespace=self.namespace.id)
                    if draft.context.base_revision
                    else None
                )
                unqualified = normalize_source_links(
                    text,
                    tuple(binding.key for binding in bindings),
                    preserve=previous.body if previous else None,
                )
                if unqualified.keys:
                    raise NousError(
                        "cognition.source_link_unqualified",
                        "Known source links require the source: prefix; adopt a corrected Markdown override",
                        details={
                            "keys": list(unqualified.keys),
                            "draft_ref": draft.ref.model_dump(mode="json"),
                        },
                    )
            published = await self.core.artifacts.content.publish_bytes(text.encode("utf-8"))
            expected_recall = True
            changed = draft.adopted_revision is None or fork
            if changed and draft.context.base_revision is not None and not fork:
                async with self.database.read() as tx:
                    proposed = await self.core.prepare_revision_in(
                        tx,
                        draft.context.base_revision,
                        draft.context.base_revision,
                        text,
                        published,
                        namespace=self.namespace.id,
                        title=title,
                        purpose=draft.context.purpose,
                        author=draft.context.author,
                        viewpoint=draft.context.viewpoint,
                        sources=bindings,
                        times=draft.context.times,
                    )
                changed = proposed.changed
                expected_recall = bool(proposed.row["recall"] and not proposed.row["archived"])
            preparation = await self.maintenance.prepare_after(
                after, in_recall=expected_recall, changed=changed
            )
            try:
                async with self.database.write() as tx:
                    ref, context, result_id, _, adopted, _ = await self.drafts.descriptor_in(
                        tx, draft.ref, self.namespace.id
                    )
                    refs = tuple(dict.fromkeys((*context.feedback_refs, *feedback_refs)))
                    if adopted is not None and not fork:
                        exact = await self.drafts.revision_in(tx, adopted)
                        committed = await self.reader.committed_in(
                            tx, exact, namespace=self.namespace.id
                        )
                        _, revision, _ = await self.reader.resolve_in(tx, exact, self.namespace.id)
                        stored = await self.core.artifacts.content.resolve_in(
                            tx, revision["body_artifact_id"]
                        )
                        if (
                            (override is not None and stored.digest != published.blob.digest)
                            or (
                                override is not None
                                and override.source_changes is not None
                                and bindings != await self.reader.sources_in(tx, exact.revision_id)
                            )
                            or (title is not None and title != committed.title)
                        ):
                            raise NousError(
                                "cognition.draft_already_adopted",
                                "Draft was already adopted with different content",
                                details={"adopted_revision": exact.model_dump(mode="json")},
                            )
                    else:
                        metadata: AdoptionValues = {
                            "title": title,
                            "purpose": context.purpose,
                            "author": context.author,
                            "viewpoint": context.viewpoint,
                            "sources": bindings,
                            "times": context.times,
                            "origin_result_id": result_id,
                            "body_artifact": draft.body_artifact if override is None else None,
                        }
                        base = context.base_revision if not fork else None
                        if base is not None:
                            committed = await self.core.append_in(
                                tx,
                                base,
                                base,
                                text,
                                published,
                                namespace=self.namespace.id,
                                change_kind=context.change_kind,
                                **metadata,
                            )
                        else:
                            if refs:
                                raise NousError(
                                    "input.feedback_requires_base",
                                    "Applying feedback requires a frozen base revision",
                                )
                            committed = await self.core.create_in(
                                tx, self.namespace.id, text, published, kind="journal", **metadata
                            )
                        if not fork:
                            await tx.connection.execute(
                                sa.update(schema.journal_drafts)
                                .where(schema.journal_drafts.c.id == ref.id)
                                .values(
                                    adopted_revision_id=committed.revision_ref.revision_id,
                                    adopted_as="override" if override is not None else "generated",
                                )
                            )
                    if (
                        committed.changed
                        and preparation.recall_precondition is not None
                        and committed.in_recall != preparation.recall_precondition
                    ):
                        raise NousError(
                            "cognition.maintenance_conflict",
                            "Recall membership changed while maintenance was prepared",
                        )
                    if refs:
                        if context.base_revision is None:
                            raise NousError(
                                "input.feedback_requires_base",
                                "Applying feedback requires a frozen base revision",
                            )
                        await self.feedback.resolve_in(
                            tx,
                            refs,
                            namespace_id=self.namespace.id,
                            base_revision=context.base_revision,
                            applied_revision=committed.revision_ref,
                            mappings=context.feedback_mappings,
                        )
                    jobs = await self.maintenance.enqueue_in(tx, committed, preparation)
                return await self.finish_write(
                    committed,
                    preparation,
                    self._link(),
                    work=jobs,
                )
            except BaseException as error:
                add_error_context(
                    error,
                    {
                        "stage": "records.adopt",
                        "draft_ref": draft.ref.model_dump(mode="json"),
                        "base_revision": draft.context.base_revision.model_dump(mode="json")
                        if draft.context.base_revision
                        else None,
                        "draft_saved": True,
                    },
                )
                raise
