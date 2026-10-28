"""Apply prepared contexts and attest actual complete host input before invocation."""

from __future__ import annotations

import hashlib
from collections.abc import Callable, Mapping, Sequence
from copy import deepcopy
from dataclasses import dataclass, field
from datetime import datetime
from typing import TYPE_CHECKING, Generic, Literal, TypeVar
from uuid import NAMESPACE_URL, uuid4, uuid5
from weakref import WeakKeyDictionary

import anyio
from pydantic import JsonValue

from .contracts import QueryRequest
from .contracts.context import (
    ContextAdmission,
    ContextAdmissionCommand,
    ContextCursor,
    ContextPrepare,
    ContextProjection,
    ContextReportResult,
    ContextSegment,
    ContextTarget,
    ContextUsage,
    ContextUse,
    EpistemicView,
    HostInputMeasurement,
    ManagedContextResponse,
)
from .source_scope import SourceReadScope

if TYPE_CHECKING:
    from ._client.context import ContextClient

T = TypeVar("T")


@dataclass(frozen=True, eq=False)
class PreparedContext:
    preparation_id: str
    response: ManagedContextResponse


@dataclass(frozen=True, eq=False)
class AppliedContext(Generic[T]):
    input: T
    preparation_id: str
    projection_id: str
    segment_ids: tuple[str, ...]
    rendered_digest: str


@dataclass
class _Application:
    projection: ContextProjection
    serialize_current: Callable[[], bytes]
    admissions: dict[str, str] = field(default_factory=dict)


class ManagedBinding:
    def __init__(
        self,
        client: ContextClient,
        *,
        namespace_id: str | None,
        subject_id: str,
        session_id: str,
        consumer_id: str,
        instance_id: str,
        source_read: SourceReadScope | Mapping[str, JsonValue] | None = None,
    ) -> None:
        self.client = client.with_source_read(source_read) if source_read is not None else client
        self.namespace_id, self.subject_id = namespace_id, subject_id
        self.session_id, self.consumer_id, self.instance_id = session_id, consumer_id, instance_id
        self._cursor: ContextCursor | None = None
        self._projection: ContextProjection | None = None
        self._pending: PreparedContext | None = None
        self._preparation_id: str | None = None
        self._preparing_job_id: str | None = None
        self._preparing = False
        self._closed = False
        self._applications: WeakKeyDictionary[object, _Application] = WeakKeyDictionary()

    def _namespace(self) -> str:
        if self.namespace_id is None:
            raise RuntimeError("Prepare the binding before using its namespace")
        return self.namespace_id

    def _assert_open(self) -> None:
        if self._closed:
            raise RuntimeError("managed cognition binding is closed")

    async def prepare(
        self,
        *,
        usage: ContextUsage | Mapping[str, JsonValue],
        max_items: int,
        max_text_bytes: int,
        current_sources: Sequence[ContextTarget] = (),
        query: QueryRequest | None = None,
        operation_id: str | None = None,
        timeout_ms: int = 45000,
    ) -> PreparedContext:
        self._assert_open()
        if self._preparing:
            raise RuntimeError(
                "parallel workers require separate managed bindings and instance IDs"
            )
        self._preparing = True
        try:
            if self.namespace_id is None:
                self.namespace_id = (await self.client.subject(self.subject_id)).namespace_id
            await self._cancel_pending()
            parsed_usage = (
                usage if isinstance(usage, ContextUsage) else ContextUsage.model_validate(usage)
            )
            command = ContextPrepare(
                operation_id=operation_id or str(uuid4()),
                session_id=self.session_id,
                consumer_id=self.consumer_id,
                instance_id=self.instance_id,
                usage=parsed_usage,
                max_items=max_items,
                max_text_bytes=max_text_bytes,
                current_sources=tuple(current_sources),
                query=query,
                known_cursor=self._cursor,
            )
            with anyio.fail_after(timeout_ms / 1000):
                submitted = await self.client.submit_context_preparation(
                    self._namespace(), self.subject_id, command
                )
                self._preparing_job_id = submitted.job_id
                jobs = await self.client.wait_success(
                    (submitted.job_id,),
                    namespace_id=self._namespace(),
                    subject_id=self.subject_id,
                    timeout_ms=timeout_ms,
                )
                output = jobs[0].output
                if not isinstance(output, dict) or not isinstance(
                    preparation_id := output.get("preparation_id"), str
                ):
                    raise ValueError("Context job did not return its preparation identity")
                result = await self.client.context_preparation(
                    self._namespace(), self.subject_id, preparation_id
                )
                self._preparing_job_id = None
            self._preparation_id = result.id
            self._assert_open()
            self._pending = PreparedContext(result.id, result.response)
            return self._pending
        except BaseException:
            with anyio.CancelScope(shield=True):
                if self._preparing_job_id is not None:
                    await self.client.cancel_job(
                        self._preparing_job_id,
                        namespace_id=self._namespace(),
                        subject_id=self.subject_id,
                    )
                    self._preparing_job_id = None
            raise
        finally:
            self._preparing = False

    def apply(
        self,
        base: T,
        prepared: PreparedContext,
        renderer: Callable[[T, Sequence[ContextSegment], ContextUsage, EpistemicView], T],
        serialize: Callable[[T], bytes],
    ) -> AppliedContext[T]:
        self._assert_open()
        if prepared is not self._pending:
            raise ValueError("context response is not this binding's pending preparation")
        response = prepared.response
        projection = response.projection
        if (
            projection.namespace_id,
            projection.subject_id,
            projection.session_id,
            projection.consumer_id,
            projection.instance_id,
        ) != (
            self._namespace(),
            self.subject_id,
            self.session_id,
            self.consumer_id,
            self.instance_id,
        ):
            raise ValueError("context projection belongs to a different managed binding")
        if response.kind == "reset":
            segments = projection.segments
        elif self._projection is not None and self._cursor is not None:
            cursor = response.cursor
            if (cursor.track_id, cursor.epoch_id, cursor.revision) != (
                self._cursor.track_id,
                self._cursor.epoch_id,
                self._cursor.revision + bool(projection.segments),
            ):
                raise ValueError("context append does not extend the applied cursor")
            segments = (*self._projection.segments, *projection.segments)
        else:
            raise ValueError("context append requires the previously applied projection")
        complete = projection.model_copy(update={"segments": segments}, deep=True)
        rendered = renderer(
            deepcopy(base),
            deepcopy(segments),
            projection.usage.model_copy(deep=True),
            projection.epistemic_view.model_copy(deep=True),
        )
        encoded = serialize(rendered)
        applied = AppliedContext(
            rendered,
            prepared.preparation_id,
            projection.id,
            tuple(item.key for item in segments),
            hashlib.sha256(encoded).hexdigest(),
        )
        self._applications[applied] = _Application(complete, lambda: serialize(rendered))
        self._projection, self._cursor = complete, response.cursor
        self._pending = None
        return applied

    def applied_projection(self, applied: AppliedContext[T]) -> ContextProjection:
        self._assert_open()
        state = self._applications.get(applied)
        if state is None:
            raise ValueError("projection requires an actual application from this binding")
        return state.projection.model_copy(deep=True)

    async def admit(
        self,
        applied: AppliedContext[T],
        *,
        invocation_id: str,
        history_generation: str,
        measurement: HostInputMeasurement,
    ) -> ContextAdmission:
        self.applied_projection(applied)
        state = self._applications[applied]
        if hashlib.sha256(state.serialize_current()).hexdigest() != applied.rendered_digest:
            raise ValueError("host input changed after rendering; apply the complete input again")
        command = ContextAdmissionCommand(
            operation_id=event_id("admit", invocation_id, applied.preparation_id),
            preparation_id=applied.preparation_id,
            invocation_id=invocation_id,
            applied_segment_ids=applied.segment_ids,
            rendered_digest=applied.rendered_digest,
            history_generation=history_generation,
            measurement=measurement,
        )
        receipt = await self.client.admit_managed_context(
            self._namespace(), self.subject_id, command
        )
        state.admissions[invocation_id] = receipt.id
        self._preparation_id = None
        return receipt

    async def report(
        self,
        applied: AppliedContext[T],
        *,
        invocation_id: str,
        outcome: Literal["completed", "failed", "cancelled"],
        occurred_at: datetime,
        uses: Sequence[ContextUse] = (),
    ) -> ContextReportResult:
        self.applied_projection(applied)
        admission_id = self._applications[applied].admissions.get(invocation_id)
        if admission_id is None:
            raise ValueError("outcome requires an admitted actual model input")
        return await self.client.report_context(
            admission_id,
            namespace_id=self._namespace(),
            subject_id=self.subject_id,
            invocation_id=invocation_id,
            outcome=outcome,
            occurred_at=occurred_at,
            uses=uses,
        )

    async def close(self) -> None:
        self._closed = True
        await self._cancel_pending()
        self._pending, self._projection, self._cursor = None, None, None
        self._applications.clear()

    async def _cancel_pending(self) -> None:
        identity = self._preparation_id
        if identity is None:
            return
        with anyio.fail_after(self.client.transport.options.cleanup_timeout_seconds, shield=True):
            await self.client.cancel_context_preparation(
                self._namespace(),
                self.subject_id,
                identity,
                event_id("cancel", self.instance_id, identity),
            )
        self._preparation_id = None
        self._pending = None


def event_id(kind: str, invocation_id: str, source: str) -> str:
    return str(uuid5(NAMESPACE_URL, f"nous:{kind}:{invocation_id}:{source}"))


__all__ = ["AppliedContext", "HostInputMeasurement", "ManagedBinding", "PreparedContext"]
