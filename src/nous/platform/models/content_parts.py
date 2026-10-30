"""Configured video content parts through the official compatible chat SDK."""

import asyncio
import base64
from typing import TYPE_CHECKING

import httpx2
from nous_wave.contracts import CallContext, DomainError
from openai import AsyncOpenAI, omit
from pydantic import BaseModel, JsonValue, TypeAdapter

from nous.platform.configuration import ModelRoute
from nous.platform.errors import NousError, Unavailable

from .completion import require_complete_text
from .ledger import CallLedger
from .outcomes import outcome

if TYPE_CHECKING:
    from .generation import MediaInput


def content_parts(
    prompt: dict[str, object], media: tuple[MediaInput, ...]
) -> list[dict[str, JsonValue]]:
    parts: list[dict[str, JsonValue]] = [{"type": "text", "text": str(prompt["rendered"])}]
    for item in media:
        if item.media_type.startswith("text/") or item.media_type == "application/json":
            parts.append({"type": "text", "text": item.data.decode("utf-8")})
            continue
        encoded = base64.b64encode(item.data).decode("ascii")
        uri = f"data:{item.media_type};base64,{encoded}"
        if item.media_type.startswith("video/"):
            parts.append({"type": "video_url", "video_url": {"url": uri}})
        elif item.media_type.startswith("image/"):
            parts.append({"type": "image_url", "image_url": {"url": uri}})
        elif item.media_type in {"audio/wav", "audio/x-wav", "audio/mpeg", "audio/mp3"}:
            audio_format = "mp3" if item.media_type in {"audio/mpeg", "audio/mp3"} else "wav"
            parts.append(
                {"type": "input_audio", "input_audio": {"data": encoded, "format": audio_format}}
            )
        elif item.media_type == "application/pdf":
            parts.append({"type": "file", "file": {"filename": "source.pdf", "file_data": uri}})
        else:
            raise Unavailable(f"Configured video content parts do not accept {item.media_type}")
    return parts


async def complete_video[T: BaseModel | str](
    ctx: CallContext,
    call_id: str,
    route: ModelRoute,
    key: str,
    prompt: dict[str, object],
    output_type: type[T],
    media: tuple[MediaInput, ...],
    ledger: CallLedger,
    client: httpx2.AsyncClient,
    *,
    role: str,
    invocation_id: str | None,
) -> tuple[T, str]:
    raw: JsonValue = None
    try:
        parts = content_parts(prompt, media)
        sdk = AsyncOpenAI(base_url=route.base_url, api_key=key, http_client=client, max_retries=0)
        if output_type is str:
            response = await sdk.chat.completions.with_raw_response.create(
                model=route.model,
                messages=[{"role": "user", "content": str(prompt["rendered"])}],
                max_tokens=route.max_output_tokens,
                temperature=route.temperature if route.temperature is not None else omit,
                reasoning_effort=route.reasoning_effort
                if route.reasoning_effort is not None
                else omit,
                extra_body={**route.extra_body, "messages": [{"role": "user", "content": parts}]},
            )
            raw = TypeAdapter(JsonValue).validate_json(response.content)
            parsed_text = response.parse()
            if not parsed_text.choices or parsed_text.choices[0].message.content is None:
                raise ValueError("Provider did not return video description text")
            require_complete_text(
                parsed_text.choices[0].finish_reason,
                refused=parsed_text.choices[0].message.refusal is not None,
            )
            result = TypeAdapter(output_type).validate_python(
                parsed_text.choices[0].message.content
            )
        elif issubclass(output_type, BaseModel):
            response_json = await sdk.chat.completions.with_raw_response.parse(
                model=route.model,
                messages=[{"role": "user", "content": str(prompt["rendered"])}],
                response_format=output_type,
                max_tokens=route.max_output_tokens,
                temperature=route.temperature if route.temperature is not None else omit,
                reasoning_effort=route.reasoning_effort
                if route.reasoning_effort is not None
                else omit,
                extra_body={**route.extra_body, "messages": [{"role": "user", "content": parts}]},
            )
            raw = TypeAdapter(JsonValue).validate_json(response_json.content)
            parsed_json = response_json.parse()
            if not parsed_json.choices or parsed_json.choices[0].message.parsed is None:
                raise ValueError("Provider did not return the requested structured video output")
            result = TypeAdapter(output_type).validate_python(parsed_json.choices[0].message.parsed)
        else:
            raise TypeError("Generation output must be text or a Pydantic model")
        await ledger.finish(
            ctx,
            call_id,
            TypeAdapter(output_type).dump_python(result, mode="json"),
            {
                "response": raw,
                "actual_output_mode": "text" if output_type is str else "json_schema",
            },
        )
        return result, call_id
    except asyncio.CancelledError:
        await asyncio.shield(
            ledger.finish(ctx, call_id, None, {"response": raw}, "CancelledUnknownOutcome")
        )
        raise
    except Exception as error:
        code = (
            error.code
            if isinstance(error, (DomainError, NousError))
            else str(getattr(error, "status_code", type(error).__name__))
        )
        await ledger.finish(
            ctx,
            call_id,
            None,
            {"response": raw},
            code,
            outcome=outcome(error, response_received=raw is not None),
        )
        raise Unavailable(
            f"Video generation failed ({code})",
            context={
                **(
                    error.details
                    if isinstance(error, DomainError)
                    else error.context
                    if isinstance(error, NousError)
                    else {}
                ),
                "call_id": call_id,
                "invocation_id": invocation_id,
                "role": role,
                "provider_code": code,
            },
        ) from error
