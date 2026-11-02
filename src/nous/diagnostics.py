"""Authorized subject inventories composed from the native domain owners."""

from collections import Counter
from typing import TYPE_CHECKING

from nous_sdk.contracts import CallContext, CognitiveKind
from nous_sdk.contracts.diagnostics import NamespaceDiagnostics
from nous_sdk.contracts.records import RecordsQuery
from pydantic import JsonValue, TypeAdapter

from nous.identity.access import require_namespace_in

if TYPE_CHECKING:
    from nous.cognition.records import CognitiveRecords
    from nous.imports.service import ImportService
    from nous.platform.execution import DurableExecution
    from nous.platform.models import ModelService
    from nous.platform.persistence import Authority
    from nous.retrieval.service import RetrievalService


class NamespaceDiagnosticsService:
    def __init__(
        self,
        authority: Authority,
        cognition: CognitiveRecords,
        execution: DurableExecution,
        models: ModelService,
        retrieval: RetrievalService,
        imports: ImportService,
    ) -> None:
        self.authority = authority
        self.cognition = cognition
        self.execution = execution
        self.models = models
        self.retrieval = retrieval
        self.imports = imports

    async def read(self, ctx: CallContext) -> NamespaceDiagnostics:
        counts: Counter[CognitiveKind] = Counter()
        async with self.authority.read(ctx) as view:
            await require_namespace_in(view, ctx)
            command = RecordsQuery(limit=1000)
            while True:
                page = await self.cognition.records_in(view, ctx, command)
                counts.update(entry.record.reference.kind for entry in page.items)
                if page.next_cursor is None:
                    break
                command = command.model_copy(update={"cursor": page.next_cursor})
            watermark = view.commit_seq
        jobs = await self.execution.list_jobs(ctx)
        return NamespaceDiagnostics(
            namespace_id=ctx.namespace_id,
            authority_commit_seq=watermark,
            cognition=dict(counts),
            tasks=dict(Counter(job.status for job in jobs)),
            retrieval=await self.retrieval.status(ctx),
            imports=await self.imports.vector_status(ctx),
            models=TypeAdapter(dict[str, JsonValue]).validate_python(
                await self.models.status(ctx)
            ),
        )
