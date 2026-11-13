"""Streaming model response lifecycle, retry and visible-delta ownership."""

from __future__ import annotations

import asyncio
from collections.abc import AsyncGenerator, Awaitable, Callable
from dataclasses import replace
from types import TracebackType
from typing import TYPE_CHECKING, cast

from openai import APIConnectionError, APIStatusError
from pydantic_ai.direct import model_request_stream
from pydantic_ai.exceptions import ModelAPIError, UnexpectedModelBehavior
from pydantic_ai.messages import PartDeltaEvent, PartStartEvent, TextPart, TextPartDelta

from nous.common.errors import NousError
from nous.models.configuration import ModelRoute

from .contracts import GenerationResult
from .policy import (
    attempt_reports,
    attempt_scope,
    endpoint_path,
    error_report,
    model_error,
    retry_delay,
    retryable,
    stage_deadline,
)
from .requests import PreparedGeneration

if TYPE_CHECKING:
    from nous.runtime.execution import OperationContext

    from .executor import ModelExecutor


class GenerationStream:
    """Close the active SDK stream on completion, cancellation or abandonment."""

    def __init__(
        self,
        executor: ModelExecutor,
        prepared: PreparedGeneration,
        context: OperationContext | None,
        prepare_route: Callable[[ModelRoute], Awaitable[PreparedGeneration]] | None,
    ) -> None:
        self.executor = executor
        self.prepared = prepared
        self.result: GenerationResult | None = None
        self.deltas = self._deltas(prepared, context, prepare_route)

    async def __aenter__(self) -> GenerationStream:
        if self.prepared.compiled.schema is not None:
            raise NousError(
                "model.stream_structured", "Structured output is returned after complete validation"
            )
        return self

    async def __aexit__(
        self,
        exc_type: type[BaseException] | None,
        exc: BaseException | None,
        traceback: TracebackType | None,
    ) -> None:
        await self.deltas.aclose()

    def __aiter__(self) -> AsyncGenerator[str]:
        return self.deltas

    async def _deltas(
        self,
        prepared: PreparedGeneration,
        context: OperationContext | None,
        prepare_route: Callable[[ModelRoute], Awaitable[PreparedGeneration]] | None = None,
    ) -> AsyncGenerator[str]:
        visible = False
        report_start = len(attempt_reports())
        active = prepared
        route_index = 0
        unavailable = dict(prepared.ineligible_routes)
        async with asyncio.timeout_at(stage_deadline(prepared.timeout_seconds, context)):
            for attempt in range(1, prepared.max_attempts + 1):
                route = prepared.routes[min(route_index, len(prepared.routes) - 1)]
                if route != active.routes[0]:
                    active, route_index = await self.executor.prepare_route(
                        prepared, route_index, prepare_route, unavailable
                    )
                    route = active.routes[0]
                if "stream" not in route.model.capabilities:
                    raise NousError(
                        "model.capability_missing",
                        f"Model {route.model_id} does not declare streaming",
                    )
                submission = self.executor.prepare_attempt(active, route, attempt)
                retry_error: BaseException | None = None
                result: GenerationResult | None = None
                with attempt_scope(
                    attempt,
                    {
                        "model_id": route.model_id,
                        "provider": route.model.provider,
                        "protocol": "openai-chat-completions",
                        "endpoint_path": endpoint_path(
                            active.snapshot, route.model.provider, "chat/completions"
                        ),
                    },
                    prepared.task,
                ) as scope:
                    try:
                        async with (
                            asyncio.timeout(route.preset.timeout_seconds),
                            model_request_stream(
                                self.executor.model_for_route(route),
                                list(submission.sdk_messages),
                                model_settings=self.executor.model_settings(active, route),
                                model_request_parameters=self.executor.request_parameters(active),
                            ) as response,
                        ):
                            async for event in response:
                                delta = (
                                    event.part.content
                                    if isinstance(event, PartStartEvent)
                                    and isinstance(event.part, TextPart)
                                    else event.delta.content_delta
                                    if isinstance(event, PartDeltaEvent)
                                    and isinstance(event.delta, TextPartDelta)
                                    else ""
                                )
                                if delta:
                                    visible = True
                                    yield delta
                            result = self.executor.response_result(
                                active, route, response.get(), attempt
                            )
                            scope.complete(
                                outcome="success",
                                provider_request_id=cast(
                                    str | None, result.producer.get("request_id")
                                ),
                                reported_usage=result.usage,
                            )
                    except asyncio.CancelledError, GeneratorExit:
                        scope.complete(outcome="cancelled", next_action="close_stream")
                        raise
                    except (
                        APIConnectionError,
                        APIStatusError,
                        TimeoutError,
                        ModelAPIError,
                        UnexpectedModelBehavior,
                        NousError,
                    ) as error:
                        retry = not visible and attempt < prepared.max_attempts and retryable(error)
                        scope.complete(
                            **error_report(
                                error,
                                next_action=(
                                    "next_route"
                                    if route_index + 1 < len(prepared.routes)
                                    else "retry_same_route"
                                )
                                if retry
                                else "raise",
                            )
                        )
                        if not retry:
                            failure = model_error(error, attempt)
                            failure.details["partial_output_delivered"] = visible
                            raise failure from error
                        route_index += 1
                        retry_error = error
                if retry_error is not None:
                    await retry_delay(retry_error)
                    continue
                assert result is not None
                self.result = replace(
                    result,
                    request={
                        **result.request,
                        "prepared_attempt": submission.to_dict(),
                        "attempt_reports": attempt_reports(report_start),
                        "ineligible_routes": unavailable,
                    },
                )
                return
