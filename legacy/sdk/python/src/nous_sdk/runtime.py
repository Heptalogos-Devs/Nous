"""Host adapters consume a sending plan constructed and persisted by Runtime."""

from __future__ import annotations
import json
from copy import deepcopy
from dataclasses import dataclass
from hashlib import sha256
from typing import TYPE_CHECKING, Any, Callable, Generic, Literal, TypeVar
from uuid import uuid4
import anyio
import httpx
from pydantic import JsonValue, TypeAdapter
from .contracts.context import (
    ChangeContextSession,
    ChangeResidents,
    ChangeWorkContext,
    ContextSession,
    ContextUsage,
    WorkContext,
)
from .contracts.runtime import (
    HostEvent,
    RuntimeOpen,
    RuntimeBindingInfo,
    RuntimePrepare,
    RuntimePreparation,
    RuntimeSent,
    RuntimeComplete,
    RuntimeResult,
    RuntimeTool,
    HostFormat,
)
from .contracts.retrieval import QueryRequest
from .errors import NousError
from .operations import encoded, segment

if TYPE_CHECKING:
    from .namespace import NamespaceClient
T = TypeVar("T")
_JSON = TypeAdapter(JsonValue)


def canonical(value: Any) -> str:
    return json.dumps(
        _JSON.dump_python(value, mode="json"),
        ensure_ascii=False,
        sort_keys=True,
        separators=(",", ":"),
        allow_nan=False,
    )


@dataclass(eq=False)
class PreparedCall(Generic[T]):
    request: T
    plan: RuntimePreparation | None
    serialize: Callable[[T], dict[str, JsonValue]]
    invocation_id: str
    sync_pending: bool = False
    sent: bool = False

    @property
    def replayed_output(self) -> JsonValue:
        return self.plan.replayed_output if self.plan else None


class RuntimeResource:
    def __init__(self, namespace: NamespaceClient):
        self.namespace = namespace
        self._base = f"/api/runtime/namespaces/{segment(namespace.namespace_id)}"

    async def session(self, session_id: str) -> ContextSession:
        return ContextSession.model_validate(
            await self.namespace.request("GET", self._base + "/sessions/" + segment(session_id))
        )

    async def change_session(self, command: ChangeContextSession) -> ContextSession:
        return ContextSession.model_validate(
            await self.namespace.request(
                "POST",
                self._base + "/sessions/" + segment(command.session_id) + "/change",
                body=encoded(command),
            )
        )

    async def close_session(self, session_id: str) -> ContextSession:
        return ContextSession.model_validate(
            await self.namespace.request(
                "POST", self._base + "/sessions/" + segment(session_id) + "/close"
            )
        )

    async def change_residents(self, command: ChangeResidents) -> ContextSession:
        return ContextSession.model_validate(
            await self.namespace.request(
                "POST",
                self._base + "/sessions/" + segment(command.session_id) + "/residents",
                body=encoded(command),
            )
        )

    async def create_work(self, command: ChangeWorkContext) -> WorkContext:
        if command.action != "create":
            raise ValueError("Work creation requires the create action")
        return await self.change_work(command)

    async def change_work(self, command: ChangeWorkContext) -> WorkContext:
        return WorkContext.model_validate(
            await self.namespace.request("POST", self._base + "/work", body=encoded(command))
        )

    async def work(self, work_id: str) -> WorkContext:
        return WorkContext.model_validate(
            await self.namespace.request(
                "GET", self._base + "/work/" + segment(work_id.removeprefix("work:"))
            )
        )

    async def work_history(self, work_id: str, *, limit: int = 100) -> tuple[WorkContext, ...]:
        return TypeAdapter(tuple[WorkContext, ...]).validate_python(
            await self.namespace.request(
                "GET",
                self._base + "/work/" + segment(work_id.removeprefix("work:")) + "/history",
                params={"limit": limit},
            )
        )

    async def works(self, *, state: str | None = None, limit: int = 100) -> tuple[WorkContext, ...]:
        return TypeAdapter(tuple[WorkContext, ...]).validate_python(
            await self.namespace.request(
                "GET", self._base + "/work", params={"state": state, "limit": limit}
            )
        )

    async def open(
        self,
        *,
        session: str,
        usage: ContextUsage,
        work: str | None = None,
        instance: str = "main",
        consumer: str = "default",
        policy: Literal["optional", "strict"] = "optional",
        timeout_seconds: float = 10,
        form_long_term: bool = True,
        context_window_tokens: int | None = None,
        reserved_output_tokens: int = 0,
    ):
        command = RuntimeOpen(
            session=session,
            instance=instance,
            consumer=consumer,
            work=work,
            usage=usage,
            form_long_term=form_long_term,
            context_window_tokens=context_window_tokens,
            output_reserve=reserved_output_tokens,
        )
        binding = RuntimeBinding(
            self.namespace, command, policy=policy, timeout_seconds=timeout_seconds
        )
        await binding._open()
        return binding


class RuntimeBinding:
    def __init__(
        self,
        namespace: NamespaceClient,
        command: RuntimeOpen,
        *,
        policy: str,
        timeout_seconds: float,
    ):
        self.namespace = namespace
        self._open_command = command
        self.info = RuntimeBindingInfo(
            session=command.session,
            instance=command.instance,
            consumer=command.consumer,
            work=command.work.removeprefix("work:") if command.work else None,
            usage=command.usage,
        )
        self.policy, self.timeout_seconds = policy, timeout_seconds
        self._opened = False
        self._active: PreparedCall | None = None
        self._pending_completions: list[RuntimeComplete] = []
        self._pending_events: list[HostEvent] = []
        self._base = f"/api/runtime/namespaces/{segment(namespace.namespace_id)}"

    def _optional(self, error: BaseException) -> bool:
        return self.policy == "optional" and (
            isinstance(error, (TimeoutError, httpx.TransportError))
            or isinstance(error, NousError)
            and error.status_code == 503
        )

    async def _post(self, path: str, command):
        with anyio.fail_after(self.timeout_seconds):
            return await self.namespace.request("POST", self._base + path, body=encoded(command))

    async def _open(self):
        if self._opened:
            return
        try:
            self.info = RuntimeBindingInfo.model_validate(
                await self._post("/sessions", self._open_command)
            )
        except BaseException as error:
            if not self._optional(error):
                raise
            return
        self._opened = True

    def _path(self, suffix: str) -> str:
        return "/sessions/" + segment(self.info.session) + suffix

    async def _flush(self):
        await self._open()
        if not self._opened:
            return
        while self._pending_completions:
            command = self._pending_completions[0]
            try:
                await self._post(self._path("/model-results"), command)
            except BaseException as error:
                if not self._optional(error):
                    raise
                return
            self._pending_completions.pop(0)

    async def before_model(
        self,
        request: dict[str, JsonValue],
        *,
        format: HostFormat = "openai-chat",
        history_mode: Literal["full", "append"] = "full",
        query: QueryRequest | None = None,
        trigger: Literal[
            "ordinary", "task_change", "tool_result", "conflict", "resume", "pause", "budget"
        ] = "ordinary",
        invocation_id: str | None = None,
        events: tuple[HostEvent, ...] = (),
    ) -> PreparedCall[dict[str, JsonValue]]:
        if self._active is not None:
            raise RuntimeError("Use separate runtime instances for concurrent model requests")
        await self._flush()
        identity = invocation_id or str(uuid4())
        self._pending_events.extend(events)
        command = RuntimePrepare(
            binding=self.info,
            invocation_id=identity,
            request=request,
            format=format,
            history_mode=history_mode,
            events=tuple(self._pending_events),
            query=query,
            trigger=trigger,
        )
        plan = None
        if self._opened:
            try:
                plan = RuntimePreparation.model_validate(
                    await self._post(self._path("/model-input"), command)
                )
            except BaseException as error:
                if not self._optional(error):
                    raise
        if plan is not None:
            self._pending_events.clear()
        actual = deepcopy(plan.request if plan is not None else request)
        prepared = PreparedCall(
            actual, plan, lambda value: value, identity, sync_pending=plan is None
        )
        self._active = prepared
        return prepared

    async def mark_sent(self, prepared: PreparedCall, *, evidence="host_send_boundary"):
        if prepared is not self._active:
            raise ValueError("Prepared call belongs to a different runtime binding")
        if prepared.sent or prepared.plan is None:
            return
        digest = sha256(canonical(prepared.serialize(prepared.request)).encode()).hexdigest()
        if digest != prepared.plan.request_hash:
            raise ValueError("The constructed model request changed before sending")
        try:
            await self._post(
                self._path("/model-sent"),
                RuntimeSent(
                    call_ref=prepared.plan.call_ref, request_hash=digest, evidence=evidence
                ),
            )
        except BaseException as error:
            if not self._optional(error):
                raise
            prepared.sync_pending = True
            return
        prepared.sent = True

    async def after_model(
        self,
        prepared: PreparedCall,
        response: JsonValue,
        *,
        text: str = "",
        model: str | None = None,
        provider: str | None = None,
        response_id: str | None = None,
        usage: dict[str, JsonValue] | None = None,
        outcome: Literal["completed", "failed", "cancelled", "unknown_outcome"] = "completed",
        error: dict[str, JsonValue] | None = None,
    ) -> RuntimeResult | None:
        if prepared is not self._active:
            raise ValueError("Prepared call belongs to a different runtime binding")
        try:
            if prepared.plan is None:
                if response is not None:
                    self._pending_events.append(
                        HostEvent(
                            event_id="response:" + (response_id or prepared.invocation_id),
                            role="assistant",
                            payload=response,
                            text=text,
                            response_id=response_id,
                        )
                    )
                return None
            command = RuntimeComplete(
                call_ref=prepared.plan.call_ref,
                outcome=outcome,
                output=response,
                text=text,
                model=model,
                provider=provider,
                response_id=response_id,
                usage=usage,
                error=error,
            )
            try:
                return RuntimeResult.model_validate(
                    await self._post(self._path("/model-results"), command)
                )
            except BaseException as failure:
                if not self._optional(failure):
                    raise
                self._pending_completions.append(command)
                prepared.sync_pending = True
                return None
        finally:
            self._active = None

    async def capture_tool(
        self,
        tool_call_id: str,
        result: JsonValue,
        *,
        text: str | None = None,
        event_id: str | None = None,
    ):
        content = result if isinstance(result, str) else canonical(result)
        event = HostEvent(
            event_id=event_id or "tool:" + tool_call_id,
            role="tool",
            payload={"tool_call_id": tool_call_id, "result": result},
            text=content if text is None else text,
            tool_call_id=tool_call_id,
        )
        await self._open()
        if self._opened:
            try:
                return await self._post(
                    self._path("/events"), RuntimeTool(binding=self.info, event=event)
                )
            except BaseException as error:
                if not self._optional(error):
                    raise
        self._pending_events.append(event)
        return None

    async def read(self, ref: str, *, sources: bool = False, max_chars: int = 12000):
        from .contracts.public import ReadView

        body = {
            "binding": self.info.model_dump(mode="json"),
            "ref": ref,
            "sources": sources,
            "max_chars": max_chars,
        }
        return ReadView.model_validate(
            await self.namespace.request("POST", self._base + self._path("/read"), body=body)
        )

    def wrap_model(
        self,
        model,
        *,
        history_mode: Literal["full", "append"] = "append",
        events: tuple[HostEvent, ...] = (),
    ):
        from .pydantic_ai import RuntimeModel

        return RuntimeModel(model, self, history_mode=history_mode, events=events)
