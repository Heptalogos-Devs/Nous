"""Cross-domain reference closure is a DAG, including output artifacts and media."""

from graphlib import CycleError, TopologicalSorter

from nous_sdk.contracts import (
    CognitiveRef,
    EntityBody,
    MediaReference,
    MemoryBody,
    NamespaceBundle,
)
from nous.platform.errors import DomainError
from nous_sdk.contracts.derivation import ArtifactRepresentation
from nous_sdk.contracts.fact_body import FactClaimBody, PersonFactScope

from nous.cognition.projection import evidence_in, media_references_in, references_in


def validate_graph(bundle: NamespaceBundle) -> None:
    from nous.cognition.fact_identity import conflict_group, identity

    exacts = {record.reference.exact(): record for record in bundle.cognition}
    fact_identities: dict[str, tuple[str, str, str, str, str]] = {}
    for record in bundle.cognition:
        body = record.body
        if not isinstance(body, FactClaimBody):
            continue
        key = identity(body)
        if fact_identities.setdefault(record.reference.id, key) != key:
            raise DomainError(
                "bundle_fact_identity", "A retained claim identity changes its semantic identity"
            )
        if isinstance(body.scope, PersonFactScope):
            person = exacts.get(body.scope.person.exact())
            if person is None or (
                not person.purged
                and (not isinstance(person.body, EntityBody) or person.body.entity_type != "person")
            ):
                raise DomainError(
                    "bundle_fact_scope",
                    "A person claim requires its actual retained Person identity",
                )
        for reference in body.supersedes:
            old = exacts.get(reference.exact())
            if (
                old is None
                or reference.kind != "fact_claim"
                or (
                    not old.purged
                    and (
                        not isinstance(old.body, FactClaimBody)
                        or conflict_group(old.body) != conflict_group(body)
                    )
                )
            ):
                raise DomainError(
                    "bundle_fact_supersedes",
                    "Supersession must retain an actual claim in the same conflict group",
                )
    graph: dict[str, set[str]] = {}
    def media(reference: MediaReference) -> str:
        return reference.exact()

    for material in bundle.materials:
        observation_dependencies = material.record.get("required_observation_ids", [])
        source_dependencies = material.record.get("required_source_ids", [])
        if not isinstance(observation_dependencies, list) or not all(
            isinstance(identity, str) for identity in observation_dependencies
        ) or not isinstance(source_dependencies, list) or not all(
            isinstance(source, str) for source in source_dependencies
        ):
            raise DomainError("bundle_source_missing", "Material dependencies must be typed references")
        dependencies: set[str] = {
            f"observation:{identity}" for identity in observation_dependencies
        }
        for source in source_dependencies:
            kind, separator, identity = source.partition(":")
            if not separator or not identity:
                raise DomainError(
                    "bundle_source_missing", "Material dependency must be a typed source reference"
                )
            if kind == "material":
                dependencies.add(f"material:{identity}")
            elif kind in {"source-region", "source_region"}:
                dependencies.add(f"source-region:{identity}")
            elif kind == "representation":
                dependencies.add(f"representation:{identity}")
            elif kind in {"derived-region", "derived_region"}:
                dependencies.add(f"derived-region:{identity}")
            else:
                raise DomainError(
                    "bundle_source_missing", "Material dependency has an unsupported source kind"
                )
        graph[f"material:{material.id}"] = dependencies
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
            *(link.reference.exact() for link in evidence_in(record) if link.reference),
            *(
                f"observation:{link.observation_id}"
                for link in evidence_in(record)
                if link.observation_id
            ),
            *(f"observation:{root}" for root in record.source_roots),
        }
        if isinstance(record.body, MemoryBody) and record.body.contact_observation_id:
            graph[record.reference.exact()].add(f"observation:{record.body.contact_observation_id}")
    catalog_rows = {row.id: row for row in bundle.catalogs}
    input_rows = {row.id: row for row in bundle.inputs}
    unit_rows = {row.id: row for row in bundle.units}
    output_rows = {row.id: row for row in bundle.outputs}
    observation_ids = {row.id for row in bundle.observations}
    material_ids = {row.id for row in bundle.materials}
    for catalog_id, catalog in catalog_rows.items():
        unit_ids = catalog.record.get("unit_ids")
        input_ids = catalog.record.get("input_source_ids")
        if not isinstance(unit_ids, list) or not isinstance(input_ids, list):
            raise DomainError("bundle_import_catalog", "Catalog header is incomplete")
        if any(identifier not in unit_rows for identifier in unit_ids) or any(
            identifier not in input_rows for identifier in input_ids
        ):
            raise DomainError("bundle_import_catalog", "Catalog refers to missing import data")
        if any(
            CognitiveRef.model_validate(reference).exact() not in exacts
            for reference in catalog.record.get("formation_references", [])
        ):
            raise DomainError("bundle_reference_missing", "Catalog formation reference is absent")
        if any(unit_rows[identifier].catalog_id != catalog_id for identifier in unit_ids):
            raise DomainError("bundle_import_catalog", "Catalog unit belongs to another catalog")
    for source in bundle.inputs:
        if source.catalog_id not in catalog_rows or source.material_id not in material_ids:
            raise DomainError("bundle_import_input", "Import input source is incomplete")
        document = source.record.get("document")
        if isinstance(document, dict) and document.get("material_id") != source.material_id:
            raise DomainError("bundle_import_input", "Import document material identity differs")
    for unit in bundle.units:
        record = unit.record
        if (
            unit.catalog_id not in catalog_rows
            or record.get("input_source_id") not in input_rows
            or record.get("material_id") not in material_ids
            or record.get("observation_id") not in observation_ids
        ):
            raise DomainError("bundle_import_unit", "Import unit source is incomplete")
        if input_rows[record["input_source_id"]].catalog_id != unit.catalog_id:
            raise DomainError("bundle_import_unit", "Import unit input belongs to another catalog")
        if record.get("source_region_id") and f"source-region:{record['source_region_id']}" not in graph:
            raise DomainError("bundle_reference_missing", "Import unit source region is absent")
        raw_source = record.get("source")
        if raw_source is not None:
            source = MediaReference.model_validate(raw_source)
            if source.exact() not in graph:
                raise DomainError("bundle_reference_missing", "Import unit media source is absent")
            if source.observation_id and f"observation:{source.observation_id}" not in graph:
                raise DomainError("bundle_reference_missing", "Import unit observation is absent")
    for output in bundle.outputs:
        if (
            output.unit_id not in unit_rows
            or output.observation_id not in observation_ids
            or any(reference.exact() not in exacts for reference in output.cognition)
        ):
            raise DomainError("bundle_import_output", "Import output points outside the bundle")
    for success in bundle.file_successes:
        if (
            success.record.get("catalog_id") not in catalog_rows
            or success.input_source_id not in input_rows
            or any(
                unit_id not in unit_rows or output_id not in output_rows
                for unit_id, output_id in success.record["unit_outputs"].items()
            )
        ):
            raise DomainError("bundle_import_success", "File completion references are incomplete")
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
