"""Verify that committed model-derived text matches its generated provenance."""

import hashlib

from nous.platform.errors import DomainError
from nous_sdk.contracts.common import ModelProvenance
from nous_sdk.contracts.derivation import RepresentationPayload, StructuredRepresentation, TextRepresentation

from nous.platform.persistence.authority import canonical_json


def digest(value: object) -> str:
    return hashlib.sha256(canonical_json(value).encode()).hexdigest()


def verify_result(
    producer: ModelProvenance,
    payload: RepresentationPayload,
    *,
    expected_task: str | None = None,
) -> None:
    if expected_task is not None and producer.task != expected_task:
        raise DomainError(
            "derivation_task_conflict",
            "Model provenance belongs to a different semantic task",
            expected_task=expected_task,
            actual_task=producer.task,
        )
    if isinstance(payload, TextRepresentation):
        output = payload.text
    elif isinstance(payload, StructuredRepresentation):
        output = payload.value
    else:
        raise DomainError(
            "derivation_result_conflict", "Model provenance cannot describe transformed artifact bytes"
        )
    if digest(output) != producer.output_digest:
        raise DomainError(
            "derivation_result_conflict",
            "Submitted derived content differs from the actual model output",
        )
