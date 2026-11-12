"""Complete record operations and their transaction-owned formal revisions."""

from __future__ import annotations

import difflib
import hashlib
import json
from collections.abc import Mapping, Sequence
from typing import Literal

import sqlalchemy as sa
from pydantic import TypeAdapter
from pydantic_core import to_jsonable_python

from nous.application.contracts import BasicEvents, CallContext, LanguageEvents, define_operation
from nous.common.errors import NousError, failure_code, failure_message
from nous.common.operations import (
    OperationIssue,
    add_error_context,
    record_completed,
    reference_value,
    report_outcome,
)
from nous.content.contracts import ExternalInput
from nous.content.edits import TextChange, prepare_text_edit
from nous.storage.content import PublishedContent

from .. import schema
from ..contracts import (
    NamespaceRef,
    NamespaceSelector,
    RecordRef,
    RecordRevisionRef,
    RecordView,
    RevisionSummary,
)
from ..feedback import FeedbackOperations
from ..maintenance.contracts import MaintenanceCompleted, MaintenanceNotStarted
from ..maintenance.operations import MaintenanceOperations
from .contracts import (
    AddNote,
    AdoptDraft,
    AfterWrite,
    ApplyEdits,
    CommittedRecord,
    CreateJournal,
    DiffRecords,
    DraftRef,
    ListRecords,
    ReadRecord,
    RecordMembership,
    RecordOutcome,
    ReplaceRecord,
    RevisionDiff,
    SetRecall,
    SourceChanges,
    SourceChoice,
)
from .drafts import DraftStore
from .reading import RecordReader, Target
from .records import AdoptionValues, RevisionMetadata, WrittenRecords


class RecordOperations:
    """Own complete writes; the revision transaction precedes all optional maintenance."""

    def __init__(
        self,
        core: WrittenRecords,
        reader: RecordReader,
        drafts: DraftStore,
        feedback: FeedbackOperations,
        maintenance: MaintenanceOperations,
    ) -> None:
        self.core, self.reader, self.drafts = core, reader, drafts
        self.feedback, self.maintenance = feedback, maintenance
        self.execution, self.database = reader.execution, reader.database

    async def _namespace(
        self, selector: NamespaceSelector, target: Target | None = None
    ) -> NamespaceRef:
        ref = target.record_ref if isinstance(target, RecordRevisionRef) else target
        owner = (
            NamespaceRef(instance_id=ref.instance_id, id=ref.namespace_id)
            if isinstance(ref, RecordRef)
            else None
        )
        return await self.core.scope.resolve_for(selector, owner)

    async def _json(self, value: ExternalInput, call: CallContext) -> bytes:
        selected = await call.inputs.read_text(
            value.key, value.selection, call.settings.values.content.text_read_max_bytes
        )
        return selected.data

    async def _source_choices(
        self, value: tuple[SourceChoice, ...] | ExternalInput, call: CallContext
    ) -> tuple[SourceChoice, ...]:
        if isinstance(value, ExternalInput):
            return TypeAdapter(tuple[SourceChoice, ...]).validate_json(
                await self._json(value, call)
            )
        return value

    async def _source_changes(
        self, value: SourceChanges | ExternalInput | None, call: CallContext
    ) -> SourceChanges | None:
        if isinstance(value, ExternalInput):
            return SourceChanges.model_validate_json(await self._json(value, call))
        return value

    async def _feedback_mappings(
        self, value: dict[str, str] | ExternalInput, call: CallContext
    ) -> dict[str, str]:
        if isinstance(value, ExternalInput):
            return TypeAdapter(dict[str, str]).validate_json(await self._json(value, call))
        return value

    async def _after(
        self, commit: CommittedRecord, policy: AfterWrite, call: CallContext
    ) -> RecordOutcome:
        record_completed(commit.revision_ref)
        if commit.changed:
            await call.saved(commit.revision_ref)
        try:
            maintenance = await self.maintenance.after_commit(commit, policy, call)
        except NousError as error:
            issue = OperationIssue(
                stage="maintenance",
                code=failure_code(error),
                message=failure_message(error),
                details=dict(error.details),
                retryable=error.retryable,
                affected_refs=(commit.revision_ref.identifier,),
                completed_refs=(reference_value(commit.revision_ref),),
            )
            maintenance = MaintenanceNotStarted(issue=issue)
        issues: list[OperationIssue] = []
        if isinstance(maintenance, MaintenanceNotStarted):
            issues.append(maintenance.issue)
        elif isinstance(maintenance, MaintenanceCompleted):
            if maintenance.annotation.state == "failed":
                issues.append(maintenance.annotation.issue)
            if maintenance.refresh.state == "failed":
                issues.append(maintenance.refresh.issue)
        report_outcome(
            "partial" if issues else "success",
            issues=issues,
            completed_refs=(commit.revision_ref,),
        )
        return RecordOutcome(commit=commit, maintenance=maintenance, issues=tuple(issues))

    async def _create(
        self,
        request: CreateJournal | AddNote,
        call: CallContext,
        *,
        kind: Literal["note", "journal"],
        recall: bool,
    ) -> RecordOutcome:
        self.maintenance.validate_after(request.after)
        namespace = await self._namespace(request.namespace)
        body = await call.inputs.body(request.body)
        choices = await self._source_choices(request.sources, call)
        selected = await self.core.sources.prepare(namespace, choices, input_reader=call.inputs)
        self.core.validate_body(body, selected.bindings)
        times = await self.core.prepare_times(request.times)
        published = await self.core.artifacts.content.publish_bytes(body.encode("utf-8"))
        async with self.database.write() as tx:
            commit = await self.core.create_in(
                tx,
                namespace.id,
                body,
                published,
                kind=kind,
                recall=recall,
                times=times,
                title=request.title,
                purpose=request.purpose,
                author=request.author,
                viewpoint=request.viewpoint,
                sources=selected.bindings,
            )
        return await self._after(commit, request.after, call)

    async def create_journal(self, request: CreateJournal, call: CallContext) -> RecordOutcome:
        return await self._create(request, call, kind="journal", recall=True)

    async def add_note(self, request: AddNote, call: CallContext) -> RecordOutcome:
        recall = (
            call.settings.values.cognition.notes.default_recall
            if request.remember is None
            else request.remember
        )
        return await self._create(request, call, kind="note", recall=recall)

    async def read_record(self, request: ReadRecord, call: CallContext) -> RecordView:
        namespace = await self._namespace(request.namespace, request.target)
        return await self.reader.read(
            request.target, namespace=namespace.id, sources=request.sources, blocks=request.blocks
        )

    async def _target_base(
        self,
        namespace: NamespaceRef,
        target: Target,
        expected: str | int | RecordRevisionRef | None,
    ) -> RecordView:
        async with self.database.read() as tx:
            row, revision, _ = await self.core.resolve_expected_in(
                tx, target, expected, namespace.id
            )
            exact = self.reader.revision_ref(row, revision)
        return await self.reader.read(exact, namespace=namespace.id)

    @staticmethod
    def _fingerprint(value: object) -> str:
        encoded = json.dumps(
            to_jsonable_python(value), ensure_ascii=False, sort_keys=True, separators=(",", ":")
        ).encode("utf-8")
        return hashlib.sha256(encoded).hexdigest()

    async def _replayed(
        self,
        namespace: NamespaceRef,
        target: Target,
        expected: str | int | RecordRevisionRef | None,
        key: str | None,
        fingerprint: str,
    ) -> CommittedRecord | None:
        if key is None:
            return None
        if not key:
            raise NousError("input.invalid", "Write key must not be empty")
        async with self.database.read() as tx:
            return await self.core.replay_in(tx, target, expected, namespace.id, key, fingerprint)

    async def _replace(
        self,
        namespace: NamespaceRef,
        target: Target,
        expected: str | int | RecordRevisionRef | None,
        body: str,
        *,
        after: AfterWrite,
        choices: tuple[SourceChoice, ...],
        source_changes: SourceChanges | None,
        reason: str | None,
        change_kind: Literal["edit", "correction"],
        feedback_refs: Sequence[str],
        feedback_mappings: Mapping[str, str],
        write_key: str | None,
        fingerprint: str,
        metadata: RevisionMetadata,
        call: CallContext,
    ) -> RecordOutcome:
        if previous := await self._replayed(namespace, target, expected, write_key, fingerprint):
            return await self._after(previous, after, call)
        original = await self._target_base(namespace, target, expected)
        changes = source_changes or SourceChanges()
        if choices:
            reserved = [source.key for source in original.sources] + [
                source.key for source in changes.add
            ]
            additions = await self.core.sources.prepare(
                namespace, choices, prefix="N", reserved_keys=reserved, input_reader=call.inputs
            )
            changes = changes.model_copy(update={"add": (*changes.add, *additions.bindings)})
        bindings = changes.apply_to(original.sources)
        if changes.add:
            prepared = await self.core.sources.prepare(
                namespace,
                changes.add,
                prefix="N",
                reserved_keys=[source.key for source in original.sources],
                preserve_keys=True,
                input_reader=call.inputs,
            )
            bindings = changes.model_copy(update={"add": prepared.bindings}).apply_to(
                original.sources
            )
        times = metadata.get("times")
        if times is not None:
            await self.core.time.validate([times])
        published = await self.core.artifacts.content.publish_bytes(body.encode("utf-8"))
        async with self.database.write() as tx:
            replayed = (
                await self.core.replay_in(
                    tx, target, expected, namespace.id, write_key, fingerprint
                )
                if write_key is not None
                else None
            )
            if replayed is not None:
                commit = replayed
            else:
                commit = await self.core.append_in(
                    tx,
                    target,
                    expected,
                    body,
                    published,
                    namespace=namespace.id,
                    change_kind=change_kind,
                    reason=reason,
                    sources=bindings,
                    write_key=write_key,
                    write_fingerprint=fingerprint if write_key else None,
                    **metadata,
                )
                if feedback_refs:
                    await self.feedback.resolve_in(
                        tx,
                        feedback_refs,
                        namespace_id=namespace.id,
                        base_revision=original.revision_ref,
                        applied_revision=commit.revision_ref,
                        note=reason,
                        mappings=feedback_mappings,
                    )
        return await self._after(commit, after, call)

    async def replace_record(self, request: ReplaceRecord, call: CallContext) -> RecordOutcome:
        self.maintenance.validate_after(request.after)
        namespace = await self._namespace(request.namespace, request.target)
        body = await call.inputs.body(request.body)
        choices = await self._source_choices(request.sources, call)
        changes = await self._source_changes(request.source_changes, call)
        feedback_mappings = await self._feedback_mappings(request.feedback_mappings, call)
        fingerprint = self._fingerprint(
            {
                "body": body,
                "sources": choices,
                "source_changes": changes,
                "reason": request.reason,
                "change_kind": request.change_kind,
                "feedback_refs": request.feedback_refs,
                "feedback_mappings": feedback_mappings,
                "title": request.title,
                "purpose": request.purpose,
                "author": request.author,
                "viewpoint": request.viewpoint,
                "times": request.times,
            }
        )
        return await self._replace(
            namespace,
            request.target,
            request.expected,
            body,
            after=request.after,
            choices=choices,
            source_changes=changes,
            reason=request.reason,
            change_kind=request.change_kind,
            feedback_refs=request.feedback_refs,
            feedback_mappings=feedback_mappings,
            write_key=request.write_key,
            fingerprint=fingerprint,
            metadata={
                "title": request.title,
                "purpose": request.purpose,
                "author": request.author,
                "viewpoint": request.viewpoint,
                "times": request.times,
            },
            call=call,
        )

    async def apply_edits(self, request: ApplyEdits, call: CallContext) -> RecordOutcome:
        self.maintenance.validate_after(request.after)
        namespace = await self._namespace(request.namespace, request.target)
        changes = request.changes
        if isinstance(changes, ExternalInput):
            changes = TypeAdapter(tuple[TextChange, ...]).validate_json(
                await self._json(changes, call)
            )
        choices = await self._source_choices(request.sources, call)
        source_changes = await self._source_changes(request.source_changes, call)
        feedback_mappings = await self._feedback_mappings(request.feedback_mappings, call)
        fingerprint = self._fingerprint(
            {
                "changes": changes,
                "sources": choices,
                "source_changes": source_changes,
                "reason": request.reason,
                "change_kind": request.change_kind,
                "feedback_refs": request.feedback_refs,
                "feedback_mappings": feedback_mappings,
            }
        )
        if previous := await self._replayed(
            namespace, request.target, request.expected, request.write_key, fingerprint
        ):
            return await self._after(previous, request.after, call)
        original = await self._target_base(namespace, request.target, request.expected)
        view = await self.core.artifacts.text_view(original.body_artifact)
        body = prepare_text_edit(view, changes).utf8_bytes.decode("utf-8")
        return await self._replace(
            namespace,
            request.target,
            request.expected,
            body,
            after=request.after,
            choices=choices,
            source_changes=source_changes,
            reason=request.reason,
            change_kind=request.change_kind,
            feedback_refs=request.feedback_refs,
            feedback_mappings=feedback_mappings,
            write_key=request.write_key,
            fingerprint=fingerprint,
            metadata={},
            call=call,
        )

    async def _membership(
        self,
        request: RecordMembership,
        call: CallContext,
        *,
        archived: bool | None = None,
        recall: bool | None = None,
    ) -> RecordOutcome:
        self.maintenance.validate_after(request.after)
        namespace = await self._namespace(request.namespace, request.target)
        async with self.database.write() as tx:
            commit = await self.core.membership_in(
                tx,
                request.target,
                request.expected,
                namespace=namespace.id,
                archived=archived,
                recall=recall,
            )
        return await self._after(commit, request.after, call)

    async def archive_record(self, request: RecordMembership, call: CallContext) -> RecordOutcome:
        return await self._membership(request, call, archived=True)

    async def restore_record(self, request: RecordMembership, call: CallContext) -> RecordOutcome:
        return await self._membership(request, call, archived=False)

    async def set_recall(self, request: SetRecall, call: CallContext) -> RecordOutcome:
        return await self._membership(request, call, recall=request.enabled)

    async def history_record(
        self, request: ReadRecord, call: CallContext
    ) -> tuple[RevisionSummary, ...]:
        namespace = await self._namespace(request.namespace, request.target)
        return await self.reader.history(request.target, namespace=namespace.id)

    async def list_records(
        self, request: ListRecords, call: CallContext
    ) -> tuple[CommittedRecord, ...]:
        namespace = await self._namespace(request.namespace)
        return await self.reader.list(
            namespace.id,
            kind=request.kind,
            archived=request.archived,
            offset=request.offset,
            limit=request.limit,
        )

    async def diff_records(self, request: DiffRecords, call: CallContext) -> RevisionDiff:
        namespace = await self._namespace(request.namespace, request.left)
        await self._namespace(namespace, request.right)
        for target in (request.left, request.right):
            if isinstance(target, RecordRef) or (isinstance(target, str) and "@" not in target):
                raise NousError("input.expected_required", "An exact revision is required")
        left = await self.reader.read(request.left, namespace=namespace.id)
        right = await self.reader.read(request.right, namespace=namespace.id)
        return RevisionDiff(
            left=left.revision_ref,
            right=right.revision_ref,
            unified="".join(
                difflib.unified_diff(
                    left.body.splitlines(keepends=True),
                    right.body.splitlines(keepends=True),
                    fromfile=left.revision_ref.identifier,
                    tofile=right.revision_ref.identifier,
                )
            ),
        )

    async def adopt_draft(self, request: AdoptDraft, call: CallContext) -> RecordOutcome:
        """Adopt a complete saved draft, preserving it on conflict or failed maintenance."""
        self.maintenance.validate_after(request.after)
        owner = (
            NamespaceRef(instance_id=request.draft.instance_id, id=request.draft.namespace_id)
            if isinstance(request.draft, DraftRef)
            else None
        )
        namespace = await self.core.scope.resolve_for(request.namespace, owner)
        draft = await self.drafts.read(request.draft, namespace=namespace.id)
        if draft.context.issues:
            raise NousError("cognition.draft_invalid", "Resolve draft issues before adoption")
        published = PublishedContent(await self.core.artifacts.blob(draft.body_artifact))
        try:
            async with self.database.write() as tx:
                descriptor = await self.drafts.descriptor_in(tx, draft.ref, namespace.id)
                ref, context = descriptor.ref, descriptor.context
                result_id, adopted = descriptor.product.id, descriptor.adopted_revision_id
                if adopted is not None and not request.fork:
                    exact = await self.drafts.revision_in(tx, adopted)
                    commit = await self.reader.committed_in(tx, exact, namespace=namespace.id)
                    if request.title is not None and request.title != commit.title:
                        raise NousError(
                            "cognition.draft_already_adopted",
                            "Draft was adopted with another title",
                            details={"adopted_revision": exact.model_dump(mode="json")},
                        )
                else:
                    metadata: AdoptionValues = {
                        "title": request.title if request.title is not None else context.title,
                        "purpose": context.purpose,
                        "author": context.author,
                        "viewpoint": context.viewpoint,
                        "sources": context.source_bindings,
                        "times": context.times,
                        "origin_result_id": result_id,
                        "body_artifact": draft.body_artifact,
                    }
                    base = context.base_revision if not request.fork else None
                    if base is None:
                        if context.feedback_refs:
                            raise NousError(
                                "input.feedback_requires_base", "Feedback requires a base revision"
                            )
                        commit = await self.core.create_in(
                            tx, namespace.id, draft.body, published, kind="journal", **metadata
                        )
                    else:
                        commit = await self.core.append_in(
                            tx,
                            base,
                            base,
                            draft.body,
                            published,
                            namespace=namespace.id,
                            change_kind=context.change_kind,
                            **metadata,
                        )
                    if not request.fork:
                        await tx.connection.execute(
                            sa.update(schema.journal_drafts)
                            .where(schema.journal_drafts.c.id == ref.id)
                            .values(
                                adopted_revision_id=commit.revision_ref.revision_id,
                                adopted_as="default",
                            )
                        )
                    if context.feedback_refs:
                        assert context.base_revision is not None
                        await self.feedback.resolve_in(
                            tx,
                            context.feedback_refs,
                            namespace_id=namespace.id,
                            base_revision=context.base_revision,
                            applied_revision=commit.revision_ref,
                            mappings=context.feedback_mappings,
                        )
            return await self._after(commit, request.after, call)
        except BaseException as error:
            add_error_context(error, {"draft_ref": draft.ref.model_dump(mode="json")})
            raise


CREATE_JOURNAL = define_operation(
    "records.create_journal",
    request=CreateJournal,
    result=RecordOutcome,
    events=LanguageEvents,
    description="Create a journal with its formal sources.",
)
ADD_NOTE = define_operation(
    "records.add_note",
    request=AddNote,
    result=RecordOutcome,
    events=LanguageEvents,
    description="Save a note with its formal sources.",
)
READ_RECORD = define_operation(
    "records.read",
    request=ReadRecord,
    result=RecordView,
    events=BasicEvents,
    description="Read a record revision and its sources.",
)
REPLACE_RECORD = define_operation(
    "records.replace",
    request=ReplaceRecord,
    result=RecordOutcome,
    events=LanguageEvents,
    description="Replace a record body and save a new revision.",
)
APPLY_EDITS = define_operation(
    "records.apply",
    request=ApplyEdits,
    result=RecordOutcome,
    events=LanguageEvents,
    description="Apply exact edits to a record revision.",
)
ARCHIVE_RECORD = define_operation(
    "records.archive",
    request=RecordMembership,
    result=RecordOutcome,
    events=LanguageEvents,
    description="Archive a record.",
)
RESTORE_RECORD = define_operation(
    "records.restore",
    request=RecordMembership,
    result=RecordOutcome,
    events=LanguageEvents,
    description="Restore an archived record.",
)
SET_RECALL = define_operation(
    "records.set_recall",
    request=SetRecall,
    result=RecordOutcome,
    events=LanguageEvents,
    description="Change whether a record participates in recall.",
)
HISTORY_RECORD = define_operation(
    "records.history",
    request=ReadRecord,
    result=tuple[RevisionSummary, ...],
    events=BasicEvents,
    description="List a record's saved revisions.",
)
LIST_RECORDS = define_operation(
    "records.list",
    request=ListRecords,
    result=tuple[CommittedRecord, ...],
    events=BasicEvents,
    description="List records in a namespace.",
)
DIFF_RECORDS = define_operation(
    "records.diff",
    request=DiffRecords,
    result=RevisionDiff,
    events=BasicEvents,
    description="Compare two exact record revisions.",
)
ADOPT_DRAFT = define_operation(
    "records.adopt_draft",
    request=AdoptDraft,
    result=RecordOutcome,
    events=LanguageEvents,
    description="Adopt a completed draft as a formal record revision.",
)
