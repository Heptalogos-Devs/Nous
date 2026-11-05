"""Complete namespace-bound record writes, exact edits and post-commit maintenance."""

from __future__ import annotations

import difflib
from collections.abc import Mapping, Sequence
from typing import TYPE_CHECKING, Literal, TypedDict, Unpack, cast

import sqlalchemy as sa

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
from ..concept_contracts import AnnotationView
from ..contracts import Namespace, RecordRevisionRef, RecordView, RevisionSummary, SourceBinding
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
    from ..annotations import AnnotationOperations
    from ..feedback import FeedbackOperations
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
        namespace: Namespace,
        core: WrittenRecords,
        reader: RecordReader,
        drafts: DraftStore,
        annotations: AnnotationOperations,
        retrieval: CognitiveRetrieval,
        feedback: FeedbackOperations,
    ):
        self.namespace, self.core, self.reader, self.drafts = namespace, core, reader, drafts
        self.annotations, self.retrieval, self.feedback = annotations, retrieval, feedback
        self.execution, self.database = reader.execution, reader.database

    async def finish_write(
        self,
        record: CommittedRecord,
        after: AfterWrite,
        operation: OperationLink,
        *,
        annotate: bool = True,
    ) -> RecordOutcome:
        """Run requested post-commit maintenance for an already committed record.

        Returns the committed revision and individual annotation and retrieval states.
        A handled follow-up error yields ``partial`` with an issue; the record remains
        committed. Deferred retrieval for a recalled record is reported as pending.
        """
        record_completed(record.revision_ref.address)
        annotation = StageOutcome(state="not_requested")
        retrieval = StageOutcome(state="pending" if record.in_recall else "not_requested")
        issues: list[OperationIssue] = []
        run_annotation = annotate and (
            after.annotation == "run"
            or after.annotation == "default"
            and record.changed
            and self.execution.snapshot.typed_values().cognition.annotation.on_save
        )
        if run_annotation:
            try:
                adopted = await self.annotations.annotate(
                    record.revision_ref, namespace=self.namespace.id
                )
                assert isinstance(adopted, AnnotationView)
                annotation = StageOutcome(
                    state="ready", generation=adopted.generation, ref=adopted.ref.address
                )
            except NousError as error:
                issue = OperationIssue(
                    "annotation",
                    error.code,
                    error.message,
                    dict(error.details),
                    error.retryable,
                    (record.revision_ref.address,),
                    (record.revision_ref.address,),
                )
                issues.append(issue)
                annotation = StageOutcome(state="failed", issue=issue)
        if after.retrieval == "refresh" or after.retrieval_mode is not None:
            try:
                published = await self.retrieval.refresh(
                    self.namespace.id, mode=after.retrieval_mode
                )
                covered = cast(int, published["covered_generation"])
                async with self.database.read() as tx:
                    current = await self.reader.scope.get_in(tx, self.namespace.id)
                ready = (
                    bool(published.get("current", True)) and current.content_generation == covered
                )
                retrieval = StageOutcome(
                    state="ready" if ready else "pending",
                    generation=covered,
                    ref=cast(dict[str, str], published["snapshot"])["address"],
                )
            except NousError as error:
                issue = OperationIssue(
                    "retrieval",
                    error.code,
                    error.message,
                    dict(error.details),
                    error.retryable,
                    (record.revision_ref.address,),
                    (record.revision_ref.address,),
                )
                issues.append(issue)
                retrieval = StageOutcome(state="failed", issue=issue)
        status = "partial" if issues else "success"
        report_outcome(status, issues=issues, completed_refs=(record.revision_ref.address,))
        return RecordOutcome(
            record=record,
            status=status,
            annotation=annotation,
            retrieval=retrieval,
            operation=operation,
        )

    def _link(self) -> OperationLink:
        from nous.common.operations import current_operation

        frame = current_operation.get()
        assert frame is not None
        return OperationLink(operation_id=frame.root.operation_id, stage=frame.stage)

    async def _resolve_requests(
        self, requests: Sequence[SourceRequest], *, prefix: str = "S", reserved: Sequence[str] = ()
    ) -> tuple[SourceBinding, ...]:
        selected = await self.reader.select_sources(
            self.namespace.id, [request.target for request in requests]
        )
        output: list[SourceBinding] = []
        index = 0
        for binding, request in zip(selected, requests, strict=True):
            while True:
                index += 1
                key = f"{prefix}{index}"
                if not any(old == key or old.startswith(key + ":") for old in reserved):
                    break
            output.append(binding.model_copy(update={"key": key, "usage": request.usage}))
        return tuple(output)

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
        self.core.validate_body(body, metadata.get("sources", ()))
        times = await self.core.prepare_times(metadata.get("times"))
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
        return await self.finish_write(committed, after, self._link())

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
                self.execution.snapshot.typed_values().cognition.notes.default_recall
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

    async def _replace(
        self,
        base: str | RecordRevisionRef,
        body: str,
        *,
        after: AfterWrite,
        feedback_refs: Sequence[str],
        feedback_mappings: Mapping[str, str],
        intent: Literal["edit", "correction"],
        reason: str | None,
        metadata: RevisionMetadata,
        source_changes: SourceChanges | None,
    ) -> RecordOutcome:
        original = await self._base(base)
        bindings = (
            source_changes.apply_to(original.sources)
            if source_changes is not None
            else original.sources
        )
        changed_times = metadata.get("times")
        if changed_times is not None:
            await self.core.time.validate([changed_times])
        published = await self.core.artifacts.content.publish_bytes(body.encode("utf-8"))
        async with self.database.write() as tx:
            committed = await self.core.append_in(
                tx,
                original.revision_ref,
                original.revision_ref,
                body,
                published,
                namespace=self.namespace.id,
                change_kind=intent,
                reason=reason,
                sources=bindings,
                **metadata,
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
        return await self.finish_write(committed, after, self._link())

    async def replace(
        self,
        base: str | RecordRevisionRef,
        markdown: str,
        *,
        reason: str | None = None,
        intent: Literal["edit", "correction"] = "edit",
        after: AfterWrite = DEFAULT_AFTER_WRITE,
        feedback_refs: Sequence[str] = (),
        feedback_mappings: Mapping[str, str] | None = None,
        source_requests: Sequence[SourceRequest] | None = None,
        sources: SourceChanges | None = None,
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
            if source_requests is not None:
                original = await self._base(base)
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
                base,
                markdown,
                after=after,
                feedback_refs=feedback_refs,
                feedback_mappings=feedback_mappings or {},
                intent=intent,
                reason=reason,
                metadata=metadata,
                source_changes=sources,
            )

    async def apply(
        self,
        base: str | RecordRevisionRef,
        changes: Sequence[TextChange],
        *,
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
            original = await self._base(base)
            view = await self.core.artifacts.text_view(original.body_artifact)
            edited = prepare_text_edit(view, changes)
            return await self._replace(
                original.revision_ref,
                edited.utf8_bytes.decode("utf-8"),
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
                        fromfile=before.revision_ref.address,
                        tofile=after.revision_ref.address,
                    )
                ),
            )

    async def _membership(
        self,
        base: str | RecordRevisionRef,
        *,
        archived: bool | None = None,
        recall: bool | None = None,
        after: AfterWrite,
    ) -> RecordOutcome:
        original = await self._base(base)
        async with self.database.write() as tx:
            committed = await self.core.membership_in(
                tx,
                original.revision_ref,
                original.revision_ref,
                namespace=self.namespace.id,
                archived=archived,
                recall=recall,
            )
        return await self.finish_write(committed, after, self._link(), annotate=False)

    async def archive(
        self, base: str | RecordRevisionRef, *, after: AfterWrite = DEFAULT_AFTER_WRITE
    ) -> RecordOutcome:
        """Archive a record against its exact current base and report follow-up state."""
        async with self.execution.operation("records.archive"):
            return await self._membership(base, archived=True, after=after)

    async def restore(
        self, base: str | RecordRevisionRef, *, after: AfterWrite = DEFAULT_AFTER_WRITE
    ) -> RecordOutcome:
        """Restore an archived record against its exact current base."""
        async with self.execution.operation("records.restore"):
            return await self._membership(base, archived=False, after=after)

    async def set_recall(
        self,
        base: str | RecordRevisionRef,
        enabled: bool,
        *,
        after: AfterWrite = DEFAULT_AFTER_WRITE,
    ) -> RecordOutcome:
        """Set a Note's recall membership against its exact current base.

        Raises:
            NousError: The base is stale or the target is not a Note.
        """
        async with self.execution.operation("records.recall"):
            return await self._membership(base, recall=enabled, after=after)

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
        """List current heads in address order using the selected kind and archive state.

        An omitted limit uses the configured page size. Offset must be nonnegative and
        limit must be between 1 and 200.
        """
        return await self.reader.list(
            self.namespace.id, kind=kind, archived=archived, offset=offset, limit=limit
        )

    async def select_sources(
        self,
        targets: Sequence[Target | ArtifactRef | ContentSelection],
        *,
        usage: Literal["reference", "current_basis", "inspiration"] = "reference",
    ) -> tuple[SourceBinding, ...]:
        """Freeze record, artifact or content targets as ordered source bindings.

        Logical record targets resolve to exact revisions in one read view; the returned
        bindings use S1, S2, and subsequent keys and the requested usage.
        """
        return await self.reader.select_sources(self.namespace.id, targets, usage=usage)

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
            if override is None and (draft.adopted_revision is None or fork):
                previous = (
                    await self.reader.read(draft.context.base_revision, namespace=self.namespace.id)
                    if draft.context.base_revision else None
                )
                unqualified = normalize_source_links(
                    text, tuple(binding.key for binding in bindings),
                    preserve=previous.body if previous else None,
                )
                if unqualified.keys:
                    raise NousError(
                        "cognition.source_link_unqualified",
                        "Known source links require the source: prefix; adopt a corrected Markdown override",
                        details={"keys": list(unqualified.keys), "draft_ref": draft.ref.model_dump(mode="json")},
                    )
            published = await self.core.artifacts.content.publish_bytes(text.encode("utf-8"))
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
                return await self.finish_write(committed, after, self._link())
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
