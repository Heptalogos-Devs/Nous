"""Bind authorized directory heads before applying return-candidate predicates."""

from __future__ import annotations

from dataclasses import dataclass
from typing import TYPE_CHECKING

from nous_wave.contracts import CognitiveRecord, ConceptBody, DomainError, EntityBody, SchemaBody

from nous.cognition.projection import media_references_in, references_in
from nous.material.service import SourceRecord

from .assets import RetrievalRecord
from .planning import BoundPlan

if TYPE_CHECKING:
    from .service import RetrievalService


def names(record: CognitiveRecord) -> tuple[str, ...]:
    body = record.body
    labels = (
        (body.label, *body.aliases)
        if isinstance(body, (EntityBody, ConceptBody))
        else (body.title,)
        if isinstance(body, SchemaBody)
        else ()
    )
    addresses = record.addresses
    return (
        *labels,
        record.reference.exact(),
        f"{record.reference.kind}:{record.reference.id}",
        *((value for value in (addresses.object, addresses.exact) if value) if addresses else ()),
    )


def resolve_name(
    catalog: list[RetrievalRecord],
    selector: str,
    locator: str,
) -> CognitiveRecord:
    kind = {"tag": "concept", "entity": "entity", "schema": "schema"}.get(selector)
    bound = [
        record
        for record in catalog
        if isinstance(record, CognitiveRecord)
        and record.reference.kind == kind
        and locator.casefold() in {name.casefold() for name in names(record)}
    ]
    if not bound:
        raise DomainError("selector_not_found", f"No accessible object matches {locator}")
    if len(bound) > 1:
        raise DomainError(
            "ambiguous_name",
            "Selector matches multiple cognitive objects",
            candidates=[value.reference.model_dump(mode="json") for value in bound],
        )
    return bound[0]


def named_targets(records: list[RetrievalRecord], target: CognitiveRecord) -> list[RetrievalRecord]:
    target_id = target.reference.id
    # Outbound members/support retain their exact published revision. A newer
    # revision does not become evidence or a member merely by sharing identity.
    outbound = {ref.exact() for ref in references_in(target.body)} | {
        ref.exact() for ref in media_references_in(target.body)
    }
    return [
        record
        for record in records
        if record.reference.id == target_id
        or record.reference.exact() in outbound
        or isinstance(record, CognitiveRecord)
        and (
            any(ref.id == target_id for ref in references_in(record.body))
            or any(link.reference and link.reference.id == target_id for link in record.evidence)
        )
    ]


@dataclass(frozen=True)
class BoundSelection:
    records: list[RetrievalRecord]
    direct: bool
    terms: tuple[str, ...]
    source_ids: tuple[str, ...]


def selectors(
    self: RetrievalService,
    records: list[RetrievalRecord],
    catalog: list[RetrievalRecord],
    plan: BoundPlan,
    sources: dict[str, SourceRecord],
) -> BoundSelection:
    candidates, direct = records, False
    terms: list[str] = []
    roots: set[str] = set()
    for cue in plan["selectors"]:
        if cue["kind"] != "selector":
            continue
        selector, locators = cue.get("selector", ""), cue.get("locators", [])
        matched: list[set[str]] = []
        participants: set[str] = set()
        for locator in locators:
            if selector == "ref":
                # The shared ReferenceDirectory has already resolved and authorized
                # this exact address. Candidate filters can legitimately remove it.
                values = [record for record in records if record.reference.exact() == locator]
                direct = True
            elif selector == "object":
                values = [
                    record
                    for record in records
                    if any(
                        oid in sources and locator in sources[oid]["origin"].values()
                        for oid in record.source_roots
                    )
                ]
            else:
                target = resolve_name(catalog, selector, locator)
                if target.reference.id in participants:
                    raise DomainError(
                        "duplicate_identity", "Selector repeats the same cognitive participant"
                    )
                participants.add(target.reference.id)
                terms.append(names(target)[0])
                roots.update(target.source_roots)
                values = named_targets(records, target)
            matched.append({value.reference.exact() for value in values})
        selected = matched[0].intersection(*matched[1:]) if matched else set[str]()
        candidates = [record for record in candidates if record.reference.exact() in selected]
    return BoundSelection(candidates, direct, tuple(dict.fromkeys(terms)), tuple(sorted(roots)))
