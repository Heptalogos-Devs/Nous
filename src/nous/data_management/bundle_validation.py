"""Cross-domain reference closure is a DAG, including output artifacts and media."""

from graphlib import CycleError, TopologicalSorter

from nous_wave.contracts import DomainError, MediaReference, SubjectBundle
from nous_wave.contracts.derivation import ArtifactRepresentation

from nous.cognition.projection import media_references_in, references_in


def validate_graph(bundle: SubjectBundle) -> None:
    graph: dict[str, set[str]] = {}
    materials = {row.id for row in bundle.materials}
    observations = {row.id for row in bundle.observations}
    regions = {str(row["id"]) for row in bundle.regions}

    def media(reference: MediaReference) -> str:
        return reference.exact()

    def locator(value: str) -> str:
        if ":" in value:
            return value.replace("source_region:", "source-region:", 1).replace(
                "derived_region:", "derived-region:", 1
            )
        if value in materials:
            return f"material:{value}"
        if value in observations:
            return f"observation:{value}"
        if value in regions:
            return f"source-region:{value}"
        raise DomainError(
            "bundle_reference_missing", "Artifact dependency is outside the archived closure"
        )

    for material in bundle.materials:
        required = material.record.get("required_source_ids", [])
        pins = material.record.get("required_observation_ids", [])
        if not isinstance(required, list) or not isinstance(pins, list):
            raise DomainError(
                "bundle_source_invalid", "Artifact dependency collections must be arrays"
            )
        graph[f"material:{material.id}"] = {
            locator(value) for value in required if isinstance(value, str)
        } | {f"observation:{pin}" for pin in pins if isinstance(pin, str)}
    for event in bundle.observations:
        graph[f"observation:{event.id}"] = {f"material:{event.material_id}"}
    for region in bundle.regions:
        graph[f"source-region:{region['id']}"] = {f"material:{region['material_id']}"}
    for representation in bundle.derivations.representations:
        dependencies = {media(item.source) for item in representation.inputs}
        if representation.previous and any(
            value.reference == representation.previous
            for value in bundle.derivations.representations
        ):
            dependencies.add(media(representation.previous.media()))
        if isinstance(representation.payload, ArtifactRepresentation):
            dependencies.add(f"material:{representation.payload.material_id}")
        graph[media(representation.reference.media())] = dependencies
    for region in bundle.derivations.regions:
        dependencies = {media(region.representation.media())}
        if region.parent_id:
            dependencies.add(f"derived-region:{region.parent_id}")
        graph[f"derived-region:{region.id}"] = dependencies
    for record in bundle.cognition:
        graph[record.reference.exact()] = {
            *(reference.exact() for reference in references_in(record.body)),
            *(media(reference) for reference in media_references_in(record.body)),
            *(link.reference.exact() for link in record.evidence if link.reference),
            *(
                f"observation:{link.observation_id}"
                for link in record.evidence
                if link.observation_id
            ),
            *(f"observation:{root}" for root in record.source_roots),
        }
    if any(target not in graph for dependencies in graph.values() for target in dependencies):
        raise DomainError(
            "bundle_reference_missing", "Bundle contains an unresolved cross-domain dependency"
        )
    try:
        tuple(TopologicalSorter(graph).static_order())
    except CycleError as error:
        raise DomainError(
            "bundle_reference_cycle", "Bundle content dependencies contain a cycle"
        ) from error
