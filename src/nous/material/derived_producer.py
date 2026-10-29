"""Bind media results to actual paid requests rather than caller assertions."""

import hashlib

from nous_wave.contracts import DomainError
from nous_wave.contracts.derivation import (
    DerivationInput,
    DerivationProducer,
    RepresentationPayload,
    StructuredRepresentation,
    TextRepresentation,
)
from pydantic import JsonValue, TypeAdapter

from nous.platform.models.ledger import PaidInvocation
from nous.platform.persistence.authority import canonical_json


def digest(value: object) -> str:
    return hashlib.sha256(canonical_json(value).encode()).hexdigest()


def producer(call: PaidInvocation) -> DerivationProducer:
    prompt = TypeAdapter(dict[str, JsonValue]).validate_python(call.request.get("prompt", {}))
    config = {key: call.request.get(key) for key in ("model", "base_url", "parameters")}
    video = call.request.get("video_content_format")
    if video is not None:
        config["video_content_format"] = video
    return DerivationProducer(
        signature=call.request_hash,
        implementation="openai-sdk" if video is not None else "pydantic-ai",
        model_call_id=call.id,
        prompt_digest=TypeAdapter(str).validate_python(prompt["hash"]),
        schema_digest=digest(call.request.get("schema")),
        config_digest=digest(config),
    )


def verify_result(
    call: PaidInvocation,
    value: DerivationProducer,
    payload: RepresentationPayload,
    inputs: tuple[DerivationInput, ...],
) -> None:
    if producer(call) != value:
        raise DomainError(
            "derivation_producer_conflict", "Producer differs from the actual model call"
        )
    if isinstance(payload, TextRepresentation):
        # Text generation selects the complete retained JSON string (root pointer "").
        if call.result != payload.text:
            raise DomainError(
                "derivation_result_conflict", "Description differs from the retained paid output"
            )
    elif isinstance(payload, StructuredRepresentation):
        if canonical_json(call.result) != canonical_json(payload.value):
            raise DomainError(
                "derivation_result_conflict", "Structure differs from the retained paid output"
            )
    else:
        raise DomainError(
            "derivation_result_conflict", "A model output cannot claim artifact bytes"
        )
    media = TypeAdapter(list[dict[str, JsonValue]]).validate_python(call.request.get("media", []))
    expected = {TypeAdapter(str).validate_python(item["source_reference"]) for item in media}
    actual = {item.source.id for item in inputs if item.role in {"source", "sampled_frame"}}
    if not expected or expected != actual:
        raise DomainError(
            "derivation_inputs_conflict", "Derivation differs from the actual supplied model media"
        )
