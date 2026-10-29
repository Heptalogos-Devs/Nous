"""Configured video content parts through the official compatible chat SDK."""

import asyncio
import base64
from typing import TYPE_CHECKING

import httpx2
from nous_wave.contracts import CallContext
from openai import AsyncOpenAI, omit
from pydantic import BaseModel, JsonValue, TypeAdapter

from nous.platform.configuration import ModelRoute
from nous.platform.errors import Unavailable

from .ledger import CallLedger

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


async def complete_video[T: BaseModel](
    ctx: CallContext,
    call_id: str,
    route: ModelRoute,
    key: str,
    prompt: dict[str, object],
    output_type: type[T],
    media: tuple[MediaInput, ...],
    ledger: CallLedger,
    client: httpx2.AsyncClient,
) -> tuple[T, str]:
    raw: JsonValue = None
    try:
        parts = content_parts(prompt, media)
        sdk = AsyncOpenAI(base_url=route.base_url, api_key=key, http_client=client, max_retries=0)
        # The SDK owns strict response schemas and parsing. extra_body supplies the
        # explicitly configured content-part extension absent from its standard video union.
        response = await sdk.chat.completions.with_raw_response.parse(
            model=route.model,
            messages=[{"role": "user", "content": str(prompt["rendered"])}],
            response_format=output_type,
            max_tokens=route.max_output_tokens,
            temperature=route.temperature if route.temperature is not None else omit,
            reasoning_effort=route.reasoning_effort if route.reasoning_effort is not None else omit,
            extra_body={"messages": [{"role": "user", "content": parts}]},
        )
        raw = TypeAdapter(JsonValue).validate_json(response.content)
        parsed = response.parse()
        if not parsed.choices or parsed.choices[0].message.parsed is None:
            raise ValueError("Provider did not return the requested structured video output")
        result = parsed.choices[0].message.parsed
        await ledger.finish(ctx, call_id, result.model_dump(mode="json"), {"response": raw})
        return result, call_id
    except asyncio.CancelledError:
        await asyncio.shield(
            ledger.finish(ctx, call_id, None, {"response": raw}, "CancelledUnknownOutcome")
        )
        raise
    except Exception as error:
        code = str(getattr(error, "status_code", type(error).__name__))
        await ledger.finish(ctx, call_id, None, {"response": raw}, code)
        raise Unavailable(f"Video generation failed ({code}); call {call_id}") from error
