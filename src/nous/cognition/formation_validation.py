"""Validate proposal-local references before any authority adoption occurs."""

from graphlib import CycleError, TopologicalSorter

from nous_sdk.contracts import DomainError, FormationCatalogEntry, ProposedConcept
from nous_sdk.contracts.cognition import FormationDraft
from pydantic import JsonValue, TypeAdapter


def validate_formation_draft(
    proposal: FormationDraft, catalog: tuple[FormationCatalogEntry, ...] = ()
) -> None:
    available = {"memory", *(entry.key for entry in catalog)}
    items = (*proposal.entities, *proposal.concepts, *proposal.schemas)
    keys = [item.key for item in items]
    if len(set(keys)) != len(keys) or set(keys).intersection(available):
        raise DomainError(
            "formation_key_conflict",
            "Proposed keys must be unique and must not shadow memory or catalog keys",
        )
    available.update(keys)
    for item in items:
        if item.target_key is not None and item.target_key not in {entry.key for entry in catalog}:
            raise DomainError(
                "formation_target_unknown",
                "An update target must be a supplied catalog key",
                key=item.target_key,
            )
    referenced = {key for item in proposal.concepts for key in item.member_keys}
    referenced.update(
        key for item in proposal.schemas for key in (*item.support_keys, *item.counterexample_keys)
    )
    referenced.update(key for item in proposal.associations for key in (item.from_key, item.to_key))
    unknown = sorted(referenced - available)
    if unknown:
        raise DomainError(
            "formation_key_unknown",
            "Every semantic endpoint must name memory, a declared local key, or a supplied source.formation_catalog key",
            unknown_keys=TypeAdapter(list[JsonValue]).validate_python(unknown),
            available_keys=TypeAdapter(list[JsonValue]).validate_python(sorted(available)),
        )
    nodes = {item.key: item for item in (*proposal.concepts, *proposal.schemas)}
    dependencies = {
        key: set(
            item.member_keys
            if isinstance(item, ProposedConcept)
            else (*item.support_keys, *item.counterexample_keys)
        ).intersection(nodes)
        for key, item in nodes.items()
    }
    try:
        tuple(TopologicalSorter(dependencies).static_order())
    except CycleError as error:
        raise DomainError(
            "formation_cycle",
            "Concept membership and Schema support must have no circular revisions",
        ) from error
