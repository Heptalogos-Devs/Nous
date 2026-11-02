"""Count actual native messages with a tokenizer and the model's media estimates."""

from __future__ import annotations

import base64
import io
import json
import math
from functools import lru_cache

import tiktoken
from nous_sdk.contracts.model_measurement import ModelMeasurement
from PIL import Image
from pydantic import JsonValue
from pydantic_ai.messages import ModelMessagesTypeAdapter

from nous.platform.configuration.models import ModelResource


@lru_cache(maxsize=32)
def encoding(model: str, configured: str | None) -> tiktoken.Encoding:
    if configured:
        return tiktoken.get_encoding(configured)
    try:
        return tiktoken.encoding_for_model(model)
    except KeyError:
        return tiktoken.get_encoding("cl100k_base")


def measure(
    request: dict[str, JsonValue],
    model: ModelResource | None,
    *,
    context_window: int | None = None,
    output_reserve: int = 0,
    binary_media: tuple[tuple[str, bytes], ...] = (),
    tokenizer_name: str | None = None,
    media_limits: dict[str, int] | None = None,
) -> ModelMeasurement:
    name = str(request.get("model") or (model.actual_name if model else ""))
    tokenizer = encoding(name, tokenizer_name or (model.tokenizer if model else None))
    limits = media_limits if media_limits is not None else model.media_limits if model else {}
    texts: list[str] = []
    items: list[dict[str, JsonValue]] = []

    def media(kind: str, value: dict[str, JsonValue], data: bytes | None = None) -> None:
        tokens: int | None = limits.get(kind + "_tokens")
        method = "configured_per_item" if tokens is not None else "unmeasured"
        width, height = value.get("width"), value.get("height")
        if kind == "image" and data is not None:
            with Image.open(io.BytesIO(data)) as image:
                width, height = image.size
        divisor = limits.get("image_pixels_per_token")
        if kind == "image" and divisor and isinstance(width, int) and isinstance(height, int):
            tokens, method = math.ceil(width * height / divisor), "configured_pixels_per_token"
            maximum = limits.get("image_max_tokens")
            if maximum is not None:
                tokens = min(tokens, maximum)
        elif kind == "image" and limits.get("image_max_tokens") is not None:
            tokens, method = limits["image_max_tokens"], "configured_image_upper_bound"
        duration = value.get("duration_seconds")
        rate = limits.get(kind + "_tokens_per_second")
        if rate is not None and isinstance(duration, int | float):
            tokens, method = math.ceil(rate * duration), "configured_duration_rate"
        pages = value.get("pages")
        if kind == "document" and isinstance(pages, int) and "document_tokens_per_page" in limits:
            tokens, method = pages * limits["document_tokens_per_page"], "configured_pages"
        items.append({"kind": kind, "tokens": tokens, "method": method})

    def data_uri(value: str | None) -> bytes | None:
        if not value or not value.startswith("data:") or ";base64," not in value:
            return None
        return base64.b64decode(value.split(",", 1)[1], validate=True)

    def visit(value: JsonValue, field: str = "") -> None:
        if isinstance(value, str):
            if field not in {"signature", "encrypted_content", "data"}:
                texts.append(value)
            return
        if isinstance(value, list):
            for child in value:
                visit(child, field)
            return
        if not isinstance(value, dict):
            return
        kind = value.get("type")
        mime = value.get("media_type")
        if isinstance(mime, str) and "data" in value:
            channel = mime.split("/", 1)[0] if not mime.startswith("application/") else "document"
            content = value["data"]
            data = base64.b64decode(content, validate=True) if isinstance(content, str) else None
            media(channel, value, data if channel == "image" else None)
            return
        if kind in {"image_url", "input_image"}:
            target = value.get("image_url")
            url = target.get("url") if isinstance(target, dict) else target
            media("image", value, data_uri(url if isinstance(url, str) else None))
            return
        if kind in {"input_audio", "audio_url"}:
            media("audio", value)
            return
        if kind in {"video_url", "input_video"}:
            media("video", value)
            return
        if kind in {"file", "input_file", "document_url"}:
            media("document", value)
            return
        if value.get("encrypted_content") or value.get("previous_response_id"):
            items.append({"kind": "opaque_history", "tokens": None, "method": "provider_state"})
        if field in {"tools", "response_format", "model_request_parameters", "parameters"}:
            texts.append(json.dumps(value, ensure_ascii=False, sort_keys=True))
            return
        for key, child in value.items():
            if key not in {
                "model",
                "format",
                "metadata",
                "timestamp",
                "vendor_metadata",
                "provider_details",
                "max_tokens",
                "max_output_tokens",
                "max_completion_tokens",
                "temperature",
                "stream",
                "run_id",
                "history_digest",
                "trace",
                "generation",
                "fingerprint",
                "previous_response_id",
                "encrypted_content",
                "signature",
                "media",
            }:
                visit(child, key)

    payload = dict(request)
    if payload.get("format") == "pydantic-ai":
        native = ModelMessagesTypeAdapter.validate_python(payload.get("messages", []))
        payload["messages"] = json.loads(ModelMessagesTypeAdapter.dump_json(native))
    visit(payload)
    for mime, data in binary_media:
        channel = mime.split("/", 1)[0] if not mime.startswith("application/") else "document"
        if channel == "text" or mime == "application/json":
            texts.append(data.decode("utf-8"))
        else:
            media(channel, {}, data if channel == "image" else None)
    for field in ("max_output_tokens", "max_completion_tokens", "max_tokens"):
        candidate = request.get(field)
        if isinstance(candidate, int) and not isinstance(candidate, bool):
            output_reserve = max(output_reserve, candidate)
    if model is not None:
        context_window = (
            min(context_window, model.context_window) if context_window else model.context_window
        )
    text_tokens = sum(len(tokenizer.encode(text, disallowed_special=())) for text in texts)
    framing = len(texts) * 4
    media_tokens = sum(int(item["tokens"]) for item in items if isinstance(item["tokens"], int))
    return ModelMeasurement(
        input_tokens=text_tokens + framing + media_tokens,
        output_reserve=output_reserve,
        context_window=context_window,
        method="tiktoken:" + tokenizer.name + "+protocol_framing+configured_media",
        estimated=True,
        media_items=tuple(items),
    )
