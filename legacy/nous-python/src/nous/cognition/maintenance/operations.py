"""Complete optional annotation and retrieval maintenance after a committed revision."""

from pydantic import TypeAdapter

from nous.application.contracts import (
    CallContext,
    EventEmitter,
    LanguageEvents,
    Notice,
    define_operation,
)
from nous.application.inputs import InputReader
from nous.common.errors import NousError, failure_code, failure_message
from nous.common.operations import OperationIssue, completed_references
from nous.runtime.execution import ExecutionManager, OperationContext
from nous.storage.database import Database

from ..annotation.contracts import (
    AnnotateRecord,
    AnnotationFailed,
    AnnotationNotRequested,
    AnnotationOutcome,
)
from ..annotation.operations import AnnotationOperations
from ..contracts import NamespaceRef, RecordRevisionRef
from ..retrieval.operations import CognitiveRetrieval
from ..scope import NamespaceOperations
from ..writing.contracts import AfterWrite, CommittedRecord
from ..writing.reading import RecordReader
from .contracts import (
    MaintainRecord,
    MaintenanceCompleted,
    MaintenanceNotStarted,
    MaintenanceOutcome,
    MaintenanceStarted,
    RefreshFailed,
    RefreshNotRequested,
    RefreshOutcome,
    RefreshPublished,
)


def _issue(stage: str, error: Exception, target: str) -> OperationIssue:
    return OperationIssue(
        stage=stage,
        code=failure_code(error),
        message=failure_message(error),
        details=dict(error.details) if isinstance(error, NousError) else {},
        retryable=error.retryable if isinstance(error, NousError) else False,
        affected_refs=(target,),
        completed_refs=completed_references(),
    )


class MaintenanceOperations:
    """Interpret follow-up policy and run independent maintenance in this runtime."""

    def __init__(
        self,
        database: Database,
        execution: ExecutionManager,
        namespaces: NamespaceOperations,
        records: RecordReader,
        annotations: AnnotationOperations,
        retrieval: CognitiveRetrieval,
    ) -> None:
        self.database = database
        self.execution = execution
        self.namespaces = namespaces
        self.records = records
        self.annotations = annotations
        self.retrieval = retrieval

    def validate_after(self, policy: AfterWrite) -> None:
        """Reject background mode before the caller publishes a formal write."""
        if policy.execution == "background":
            self.execution.require_background()

    async def after_commit(
        self, commit: CommittedRecord, policy: AfterWrite, call: CallContext
    ) -> MaintenanceOutcome:
        if not commit.changed:
            return MaintenanceCompleted()
        settings = call.settings.values.cognition
        annotate = policy.annotation == "run" or (
            policy.annotation == "default" and commit.in_recall and settings.annotation.on_save
        )
        refresh = policy.refresh == "run"
        if not annotate and not refresh:
            return MaintenanceCompleted()
        return await self.run(
            MaintainRecord(
                target=commit.revision_ref,
                namespace=commit.namespace.ref,
                annotation=annotate,
                refresh=refresh,
                mode=policy.mode,
                require_recall=policy.annotation == "default",
                execution=policy.execution,
            ),
            call,
        )

    async def run(self, request: MaintainRecord, call: CallContext) -> MaintenanceOutcome:
        """Run maintenance on one exact revision or start an owned background task."""
        namespace = request.namespace
        if isinstance(namespace, NamespaceRef):
            if namespace.instance_id != self.annotations.artifacts.instance_id:
                raise NousError(
                    "cognition.instance_mismatch", "Namespace belongs to another instance"
                )
            namespace = namespace.id
        async with self.database.read() as tx:
            row, revision, ns = await self.records.resolve_in(tx, request.target, namespace)
            exact = self.records.revision_ref(row, revision)
        resolved = request.model_copy(
            update={"target": exact, "namespace": ns.ref, "execution": "wait"}
        )
        if request.execution == "wait":
            return await self._run(resolved, call)
        self.execution.require_background()
        settings = call.settings

        async def work(context: OperationContext) -> MaintenanceCompleted:
            inputs = InputReader(
                {},
                artifacts=self.annotations.artifacts,
                readers=self.annotations.artifacts.content.file_readers,
                execution=self.execution,
            )
            events = EventEmitter(context.operation_id, TypeAdapter(LanguageEvents))
            return await self._run(resolved, CallContext(context, inputs, events))

        try:
            ref = self.execution.spawn(
                "maintenance.run", work, target_refs=(exact,), snapshot=settings
            )
        except NousError as error:
            if error.code not in {"operation.unavailable", "execution.background_capacity"}:
                raise
            issue = _issue("maintenance", error, exact.identifier)
            await call.events.emit(Notice(issue=issue))
            return MaintenanceNotStarted(issue=issue)
        return MaintenanceStarted(operation_ref=ref)

    async def _run(self, request: MaintainRecord, call: CallContext) -> MaintenanceCompleted:
        target = request.target
        assert isinstance(target, RecordRevisionRef)
        annotation: AnnotationOutcome = AnnotationNotRequested()
        refresh: RefreshOutcome = RefreshNotRequested()
        if request.annotation:
            try:
                annotation = await self.annotations.annotate(
                    AnnotateRecord(
                        target=target,
                        namespace=request.namespace,
                        task=request.task,
                        require_recall=request.require_recall,
                        activate_for_current=True,
                    ),
                    call,
                )
            except NousError as error:
                issue = _issue("annotation", error, target.identifier)
                annotation = AnnotationFailed(issue=issue)
                await call.events.emit(Notice(issue=issue))
        if request.refresh:
            try:
                result = await self.retrieval.refresh(
                    target.record_ref.namespace_id, mode=request.mode
                )
                if result.captured_generation is None:
                    raise NousError(
                        "cognition.invalid_snapshot", "Published snapshot has no generation"
                    )
                refresh = RefreshPublished(
                    snapshot_ref=result.snapshot,
                    captured_generation=result.captured_generation,
                    current_generation=result.current_generation,
                    current=result.current,
                )
                await call.saved(result.snapshot)
            except NousError as error:
                issue = _issue("retrieval", error, target.identifier)
                refresh = RefreshFailed(issue=issue)
                await call.events.emit(Notice(issue=issue))
        return MaintenanceCompleted(annotation=annotation, refresh=refresh)


MAINTAIN_RECORD = define_operation(
    "maintenance.run",
    request=MaintainRecord,
    result=MaintenanceOutcome,
    events=LanguageEvents,
    description="Run annotation and retrieval maintenance for a record.",
)
