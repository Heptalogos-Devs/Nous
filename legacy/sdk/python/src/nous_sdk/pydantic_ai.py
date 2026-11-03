"""First-party Pydantic AI model wrapper; native messages remain native."""

from __future__ import annotations
from contextlib import asynccontextmanager
from typing import Any
from pydantic import TypeAdapter
from pydantic_ai.messages import ModelMessagesTypeAdapter, ModelResponse
from pydantic_ai.models import ModelRequestParameters
from pydantic_ai.models.wrapper import WrapperModel
from .runtime import RuntimeBinding

_PARAMS = TypeAdapter(ModelRequestParameters)
_JSON = TypeAdapter(Any)


class RuntimeModel(WrapperModel):
    def __init__(self, wrapped, binding: RuntimeBinding, *, history_mode="append", events=()):
        super().__init__(wrapped)
        self.binding = binding
        self.history_mode = history_mode
        self.events = tuple(events)

    async def _prepare(self, messages, settings, parameters):
        params = _PARAMS.dump_python(parameters, mode="json")
        visible_settings = {
            key: value
            for key, value in (settings or {}).items()
            if key not in {"extra_headers", "extra_query_params"}
        }
        snapshot = {
            "model": self.model_name,
            "messages": ModelMessagesTypeAdapter.dump_python(messages, mode="json"),
            "settings": _JSON.dump_python(visible_settings, mode="json"),
            "parameters": params,
            "tools": [
                {
                    "name": tool.name,
                    "description": tool.description,
                    "parameters": tool.parameters_json_schema,
                }
                for tool in parameters.function_tools
            ],
        }
        prepared = await self.binding.before_model(
            snapshot, format="pydantic-ai", history_mode=self.history_mode, events=self.events
        )
        self.events = ()
        if prepared.plan is not None:
            actual = ModelMessagesTypeAdapter.validate_python(prepared.plan.request["messages"])
            base = dict(prepared.plan.request)
            prepared.request = actual
            prepared.serialize = lambda value: {
                **base,
                "messages": ModelMessagesTypeAdapter.dump_python(value, mode="json"),
            }
        else:
            prepared.request = messages
            prepared.serialize = lambda value: {
                **snapshot,
                "messages": ModelMessagesTypeAdapter.dump_python(value, mode="json"),
            }
        # The framework persists exactly the message sequence it sent, including native replay data.
        messages[:] = prepared.request
        return prepared

    def _marker(self, response, prepared):
        if prepared.plan is not None:
            response.metadata = {
                **(response.metadata or {}),
                "nous_runtime_call": {
                    "call_ref": prepared.plan.call_ref,
                    "namespace_id": self.binding.namespace.namespace_id,
                    "session": self.binding.info.session,
                },
            }

    async def _complete(self, prepared, response, outcome="completed", error=None):
        self._marker(response, prepared)
        raw = ModelMessagesTypeAdapter.dump_python([response], mode="json")[0]
        usage = _JSON.dump_python(response.usage, mode="json")
        return await self.binding.after_model(
            prepared,
            raw,
            text=response.text or "",
            model=response.model_name,
            provider=response.provider_name,
            response_id=response.provider_response_id,
            usage=usage,
            outcome=outcome,
            error=error,
        )

    async def request(self, messages, model_settings, model_request_parameters):
        prepared = await self._prepare(messages, model_settings, model_request_parameters)
        if prepared.replayed_output is not None:
            response = ModelMessagesTypeAdapter.validate_python([prepared.replayed_output])[0]
            if not isinstance(response, ModelResponse):
                raise TypeError("Retained runtime output is not a model response")
            self.binding._active = None
            return response
        try:
            await self.binding.mark_sent(prepared, evidence="host_send_boundary")
            response = await self.wrapped.request(
                messages, model_settings, model_request_parameters
            )
        except BaseException as error:
            import asyncio

            outcome = "cancelled" if isinstance(error, asyncio.CancelledError) else "failed"
            import anyio

            with anyio.CancelScope(shield=True):
                await self.binding.after_model(
                    prepared,
                    None,
                    outcome=outcome,
                    error={"type": type(error).__name__, "message": str(error)},
                )
            raise
        await self._complete(prepared, response)
        return response

    @asynccontextmanager
    async def request_stream(
        self, messages, model_settings, model_request_parameters, run_context=None
    ):
        prepared = await self._prepare(messages, model_settings, model_request_parameters)
        stream = None
        try:
            await self.binding.mark_sent(prepared, evidence="host_send_boundary")
            async with self.wrapped.request_stream(
                messages, model_settings, model_request_parameters, run_context
            ) as actual:
                stream = actual
                if prepared.plan is not None:
                    stream.metadata = {
                        **(stream.metadata or {}),
                        "nous_runtime_call": {
                            "call_ref": prepared.plan.call_ref,
                            "namespace_id": self.binding.namespace.namespace_id,
                            "session": self.binding.info.session,
                        },
                    }
                yield stream
        except BaseException as error:
            import asyncio, anyio

            outcome = "cancelled" if isinstance(error, asyncio.CancelledError) else "failed"
            with anyio.CancelScope(shield=True):
                if stream is not None:
                    await self._complete(
                        prepared,
                        stream.get(),
                        outcome,
                        error={"type": type(error).__name__, "message": str(error)},
                    )
                else:
                    await self.binding.after_model(
                        prepared,
                        None,
                        outcome=outcome,
                        error={"type": type(error).__name__, "message": str(error)},
                    )
            raise
        else:
            response = stream.get()
            await self._complete(
                prepared,
                response,
                "completed" if response.state == "complete" else "unknown_outcome",
            )
