"""Query predicates select authorized facts; they never create source grants."""

from nous_wave.contracts import CognitiveRecord
from nous_wave.contracts.query_conditions import QueryAuthority, QueryFilters, QueryModality

from nous.material.service import SourceRecord

from .assets import RetrievalRecord


def authority(record: RetrievalRecord) -> QueryAuthority:
    if isinstance(record, CognitiveRecord):
        return "subject_cognition"
    return (
        "interpretation"
        if record.reference.kind in {"representation", "derived_region"}
        else "evidence"
    )


def matches(
    record: RetrievalRecord,
    filters: QueryFilters,
    sources: dict[str, SourceRecord],
    named_sources: frozenset[str] | None = None,
    modalities: dict[str, QueryModality] | None = None,
) -> bool:
    if filters.authority is not None and filters.authority != authority(record):
        return False
    constrained = (
        filters.source_ids is not None
        or filters.source_types is not None
        or filters.origin
        or named_sources is not None
        or modalities is not None
    )
    if constrained and not any(
        (named_sources is None or source["id"] in named_sources)
        and (modalities is None or modalities.get(source["id"]) in filters.modalities)
        and (
            filters.source_ids is None
            or source["id"] in filters.source_ids
            or source["material_id"] in filters.source_ids
        )
        and (filters.source_types is None or source["source_type"] in filters.source_types)
        and all(source["origin"].get(term.field) == term.value for term in filters.origin)
        for oid in record.source_roots
        if (source := sources.get(oid)) is not None
    ):
        return False
    body = record.body if isinstance(record, CognitiveRecord) else None
    if filters.tags and not set(filters.tags).issubset(getattr(body, "tags", ())):
        return False
    if (
        filters.cognitive_roles
        and getattr(body, "cognitive_role", getattr(body, "role", None))
        not in filters.cognitive_roles
    ):
        return False
    if (
        filters.formation_modes
        and getattr(body, "formation_mode", None) not in filters.formation_modes
    ):
        return False
    epistemic = getattr(body, "epistemic", None)
    return (
        not filters.epistemic_classes
        or getattr(epistemic, "classification", None) in filters.epistemic_classes
    )
