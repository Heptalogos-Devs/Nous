"""One binding implementation for read-only validation and authority adoption."""

from collections.abc import Mapping
from copy import deepcopy
from dataclasses import dataclass

import jsonpointer
from nous_wave.contracts import (
    CognitiveBody,
    CognitiveRecord,
    CognitiveRef,
    ConceptBody,
    DomainError,
    EntityBody,
    EpisodeBody,
    JournalBody,
    MemoryBody,
    MemoryLifecycle,
)
from nous_wave.contracts.growth import GrowthAdoption
from pydantic import JsonValue, TypeAdapter, ValidationError

from .projection import media_references_in, references_in

_BODY = TypeAdapter(CognitiveBody)
_JSON = TypeAdapter(dict[str, JsonValue])


@dataclass(frozen=True)
class GrowthValue:
    reference: CognitiveRef
    body: CognitiveBody | None
    commit_seq: int
    lifecycle: MemoryLifecycle | None = None
    deleted: bool = False


def bind_body(
    action: GrowthAdoption, catalog: Mapping[str, CognitiveRecord | GrowthValue], watermark: int
) -> CognitiveBody:
    body_value = deepcopy(action.body_input)

    def require_symbolic(value: JsonValue, field: str = "") -> None:
        if isinstance(value, dict):
            if field in {"metadata", "structured", "identifiers", "external_ids"}:
                return
            if "kind" in value and "id" in value:
                raise DomainError(
                    "growth_binding_required",
                    "Proposals use local-key bindings instead of formal object references",
                )
            if (
                value.get("observation_id") is not None
                or value.get("grounding_observation") is not None
            ):
                raise DomainError(
                    "growth_binding_required",
                    "Program source identities cannot be supplied by a proposal",
                )
            for key, child in value.items():
                require_symbolic(child, key)
        elif isinstance(value, list):
            for child in value:
                require_symbolic(child, field)

    require_symbolic(body_value)
    paths: set[str] = set()
    try:
        for binding in action.bindings:
            path = jsonpointer.JsonPointer(binding.path)
            if (
                binding.path in paths
                or not binding.path.startswith("/")
                or any(
                    part in {"metadata", "structured", "external_ids", "identifiers"}
                    for part in path.parts
                )
            ):
                raise DomainError(
                    "growth_binding_invalid",
                    "Bindings identify distinct typed reference fields",
                )
            paths.add(binding.path)
            if path.resolve(body_value) is not None:
                raise DomainError(
                    "growth_binding_invalid",
                    "A reference binding must replace an explicit null placeholder",
                )
            path.set(body_value, catalog[binding.key].reference.model_dump(mode="json"))
        body = _BODY.validate_python(_JSON.validate_python(body_value))
    except jsonpointer.JsonPointerException as error:
        raise DomainError(
            "growth_binding_invalid",
            "A proposal binding does not identify an existing reference placeholder",
        ) from error
    except ValidationError as error:
        raise DomainError(
            "growth_body_invalid",
            "Proposed content does not satisfy its cognitive domain contract",
            issues=TypeAdapter(list[JsonValue]).validate_python(
                [{"type": issue["type"], "loc": list(issue["loc"])} for issue in error.errors()]
            ),
        ) from error
    allowed = {record.reference.exact() for record in catalog.values()}
    if any(reference.exact() not in allowed for reference in references_in(body)):
        raise DomainError(
            "growth_reference_unknown",
            "Body references must resolve to snapshot or adopted proposal identities",
        )
    allowed_media = {
        reference.exact()
        for record in catalog.values()
        for reference in media_references_in(record.body)
    }
    if any(reference.exact() not in allowed_media for reference in media_references_in(body)):
        raise DomainError(
            "growth_media_reference_unknown",
            "Media references must come from actual snapshot content",
        )
    if isinstance(body, ConceptBody) and body.state != "active":
        raise DomainError(
            "growth_structural_action_required",
            "Concept retirement uses the merge or split action",
        )
    if isinstance(body, EpisodeBody) and body.state != "active":
        raise DomainError(
            "growth_structural_action_required", "Episode retirement uses formal resegmentation"
        )
    if isinstance(body, JournalBody):
        body = body.model_copy(update={"knowledge_cutoff": watermark})
    target = catalog.get(action.target_key) if action.target_key else None
    if target is not None and isinstance(body, MemoryBody) and isinstance(target.body, MemoryBody):
        body = body.model_copy(
            update={
                "media": target.body.media,
                "metadata": target.body.metadata,
                "external_ids": target.body.external_ids,
                "structured": target.body.structured,
            }
        )
    if target is not None and isinstance(body, EntityBody) and isinstance(target.body, EntityBody):
        body = body.model_copy(
            update={
                "identifiers": target.body.identifiers,
                "platform_identities": target.body.platform_identities,
                "purged_identity_count": target.body.purged_identity_count,
                "purged_attribute_count": target.body.purged_attribute_count,
            }
        )
    return body
