from __future__ import annotations

import hashlib
import logging
from collections.abc import Callable, Mapping, Sequence
from copy import deepcopy
from dataclasses import asdict, dataclass
from datetime import datetime
from typing import TYPE_CHECKING, Any, Generic, TypeVar
from uuid import NAMESPACE_URL, uuid4, uuid5
from weakref import WeakKeyDictionary

import anyio

from .errors import NousError

if TYPE_CHECKING:
    from .client import NousClient

T = TypeVar("T")
logger = logging.getLogger(__name__)


@dataclass(frozen=True)
class HostInputMeasurement:
    input_tokens: int
    reserved_output_tokens: int
    context_window_tokens: int
    source: str

    def validate(self) -> None:
        values = (
            self.input_tokens,
            self.reserved_output_tokens,
            self.context_window_tokens,
        )
        if (
            any(type(value) is not int or value < 0 for value in values)
            or not self.context_window_tokens
        ):
            raise ValueError(
                "token measurements must be nonnegative integers with a positive window"
            )
        if self.input_tokens + self.reserved_output_tokens > self.context_window_tokens:
            raise ValueError(
                "the complete rendered input and reserved output exceed the model window"
            )
        if not self.source.strip():
            raise ValueError("supply the actual host measurement source")


@dataclass(frozen=True, eq=False)
class PreparedContext:
    preparation_id: str
    response: dict[str, Any]


@dataclass(frozen=True, eq=False)
class AppliedContext(Generic[T]):
    input: T
    preparation_id: str
    projection_id: str
    segment_ids: tuple[str, ...]
    rendered_digest: str


class ManagedBinding:
    def __init__(
        self,
        client: NousClient,
        *,
        subject_id: str,
        session_id: str,
        consumer_id: str,
        instance_id: str,
        source_read: Mapping | None = None,
    ) -> None:
        self.client = (
            client.with_source_read(source_read) if source_read is not None else client
        )
        self.subject_id = subject_id
        self.session_id = session_id
        self.consumer_id = consumer_id
        self.instance_id = instance_id
        self._cursor: dict | None = None
        self._projection: dict | None = None
        self._pending: PreparedContext | None = None
        self._pending_snapshot: dict | None = None
        self._preparation_id: str | None = None
        self._preparing = False
        self._closed = False
        self._applications: WeakKeyDictionary[AppliedContext, tuple[dict, Callable]] = (
            WeakKeyDictionary()
        )
        self._admissions: WeakKeyDictionary[AppliedContext, dict[str, str]] = (
            WeakKeyDictionary()
        )

    def _assert_open(self) -> None:
        if self._closed:
            raise RuntimeError("managed cognition binding is closed")

    async def prepare(
        self,
        *,
        usage: Mapping,
        max_items: int,
        max_text_bytes: int,
        current_sources: Sequence[Mapping] = (),
        query: Mapping | None = None,
        operation_id: str | None = None,
        timeout_ms: int = 45000,
    ) -> PreparedContext:
        self._assert_open()
        if self._preparing:
            raise RuntimeError(
                "parallel workers require separate managed bindings and instance IDs"
            )
        self._preparing = True
        preparation_id = None
        try:
            await self._cancel_pending(required=True)
            self._pending = None
            self._pending_snapshot = None
            receipt = await self.client.contexts(
                {
                    "action": "prepare",
                    "operation_id": operation_id or str(uuid4()),
                    "request": {
                        "subject_id": self.subject_id,
                        "session_id": self.session_id,
                        "consumer_id": self.consumer_id,
                        "instance_id": self.instance_id,
                        "usage": deepcopy(dict(usage)),
                        "max_items": max_items,
                        "max_text_bytes": max_text_bytes,
                        "current_sources": deepcopy(list(current_sources)),
                        "query": deepcopy(query),
                        "known_cursor": deepcopy(self._cursor),
                    },
                }
            )
            preparation_id = receipt["result"]["preparation_id"]
            self._preparation_id = preparation_id
            if self._closed:
                await self._cancel_pending()
                self._assert_open()
            await self.client.wait_success(receipt["job_ids"], timeout_ms=timeout_ms)
            preparation = await self.client.contexts(
                {"action": "preparation", "id": preparation_id}
            )
            self._assert_open()
            response = preparation.get("response")
            if response is None:
                raise NousError(
                    {
                        "code": "DependencyUnavailable",
                        "message": "context preparation has no usable projection",
                    }
                )
            prepared = PreparedContext(preparation_id, deepcopy(response))
            self._pending = prepared
            self._pending_snapshot = deepcopy(response)
            return prepared
        except NousError as error:
            if preparation_id:
                error.context["preparation_id"] = preparation_id
            raise
        except BaseException as error:
            if isinstance(error, anyio.get_cancelled_exc_class()):
                await self._cancel_pending()
            raise
        finally:
            self._preparing = False

    def apply(
        self,
        base: T,
        prepared: PreparedContext,
        renderer: Callable[[T, Sequence[Mapping], Mapping, Mapping], T],
        serialize: Callable[[T], bytes],
    ) -> AppliedContext[T]:
        self._assert_open()
        if prepared is not self._pending or prepared.response != self._pending_snapshot:
            raise ValueError(
                "context response is not this binding's unchanged pending preparation"
            )
        response = deepcopy(self._pending_snapshot)
        if not isinstance(response, dict):
            raise TypeError("context response must be a JSON object")
        projection, cursor = response["projection"], response["cursor"]
        expected = {
            "subject_id": self.subject_id,
            "session_id": self.session_id,
            "consumer_id": self.consumer_id,
            "instance_id": self.instance_id,
        }
        if any(projection.get(key) != value for key, value in expected.items()):
            raise ValueError(
                "context projection belongs to a different managed binding"
            )
        if response["kind"] == "reset":
            segments = projection["segments"]
        elif (
            response["kind"] == "append"
            and self._projection is not None
            and self._cursor is not None
        ):
            delta = projection["segments"]
            if (
                cursor["track_id"] != self._cursor["track_id"]
                or cursor["epoch_id"] != self._cursor["epoch_id"]
            ):
                raise ValueError("context append belongs to a different track or epoch")
            if cursor["revision"] != self._cursor["revision"] + bool(delta):
                raise ValueError("context append does not extend the applied cursor")
            segments = [*deepcopy(self._projection["segments"]), *delta]
        else:
            raise ValueError("context response cannot extend this binding")
        projection["segments"] = segments
        rendered = renderer(
            deepcopy(base),
            deepcopy(segments),
            deepcopy(projection["usage"]),
            deepcopy(projection["epistemic_view"]),
        )
        encoded = serialize(rendered)
        if not isinstance(encoded, bytes):
            raise TypeError(
                "host serializer must return the actual rendered input bytes"
            )
        applied = AppliedContext(
            rendered,
            prepared.preparation_id,
            projection["id"],
            tuple(segment["key"] for segment in segments),
            hashlib.sha256(encoded).hexdigest(),
        )
        self._applications[applied] = (deepcopy(projection), serialize)
        self._projection = deepcopy(projection)
        self._cursor = deepcopy(cursor)
        self._pending = None
        self._pending_snapshot = None
        self._preparation_id = None
        return applied

    def applied_projection(self, applied: AppliedContext) -> dict:
        self._assert_open()
        if applied not in self._applications:
            raise ValueError(
                "projection requires an actual application from this binding"
            )
        return deepcopy(self._applications[applied][0])

    async def admit(
        self,
        applied: AppliedContext,
        *,
        invocation_id: str,
        history_generation: str,
        measurement: HostInputMeasurement,
    ) -> dict:
        self.applied_projection(applied)
        measurement.validate()
        serializer = self._applications[applied][1]
        if (
            hashlib.sha256(serializer(applied.input)).hexdigest()
            != applied.rendered_digest
        ):
            raise ValueError(
                "host input changed after rendering; apply the complete input again"
            )
        if not invocation_id.strip():
            raise ValueError("actual invocation ID is required")
        receipt = await self.client.contexts(
            {
                "action": "admit",
                "operation_id": event_id(
                    "admit", invocation_id, applied.preparation_id
                ),
                "request": {
                    "preparation_id": applied.preparation_id,
                    "invocation_id": invocation_id,
                    "applied_segment_ids": list(applied.segment_ids),
                    "rendered_digest": applied.rendered_digest,
                    "history_generation": history_generation,
                    "measurement": asdict(measurement),
                },
            }
        )
        self._admissions.setdefault(applied, {})[invocation_id] = receipt["result"][
            "id"
        ]
        return receipt

    async def report(
        self,
        applied: AppliedContext,
        *,
        invocation_id: str,
        outcome: str,
        occurred_at: datetime,
        uses: Sequence[Mapping] = (),
    ) -> dict:
        self.applied_projection(applied)
        admission_id = self._admissions.get(applied, {}).get(invocation_id)
        if admission_id is None:
            raise ValueError("outcome requires an admitted actual model input")
        return await self.client.report_context(
            admission_id,
            invocation_id=invocation_id,
            outcome=outcome,
            occurred_at=occurred_at,
            uses=uses,
        )

    async def close(self) -> None:
        self._closed = True
        await self._cancel_pending()
        self._pending = None
        self._pending_snapshot = None
        self._projection = None
        self._cursor = None
        self._applications.clear()
        self._admissions.clear()

    async def _cancel_pending(self, *, required: bool = False) -> None:
        preparation_id = self._preparation_id
        if preparation_id is None:
            return
        if self._pending is not None:
            self._preparation_id = None
            return
        with anyio.move_on_after(
            self.client.transport.options.cleanup_timeout_seconds, shield=True
        ):
            try:
                await self.client.contexts(
                    {
                        "action": "cancel_preparation",
                        "id": preparation_id,
                        "operation_id": event_id(
                            "cancel", self.instance_id, preparation_id
                        ),
                    }
                )
                self._preparation_id = None
            except Exception:
                logger.warning(
                    "context preparation cancellation failed: %s",
                    preparation_id,
                    exc_info=True,
                )
        if required and self._preparation_id is not None:
            raise NousError(
                {
                    "code": "DependencyUnavailable",
                    "message": "previous context preparation is still pending",
                    "context": {"preparation_id": preparation_id},
                }
            )


def event_id(kind: str, invocation_id: str, source: str) -> str:
    return str(uuid5(NAMESPACE_URL, f"nous:{kind}:{invocation_id}:{source}"))
