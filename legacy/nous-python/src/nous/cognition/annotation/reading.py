"""Materialize exact annotation views within a caller-owned read transaction."""

from collections import defaultdict
from collections.abc import Sequence

import sqlalchemy as sa
from sqlalchemy.engine import RowMapping

from nous.common.errors import NousError
from nous.content.contracts import ContentSelection, ResultRef
from nous.content.schema import results as processing_results
from nous.storage.database import ReadTx

from .. import concept_schema as schema
from .. import schema as written
from ..concept_contracts import (
    AnnotationRef,
    AnnotationView,
    BoundOccurrence,
    BoundPath,
    ConceptView,
)
from ..concepts import ConceptOperations
from ..contracts import RecordRef, RecordRevisionRef


async def read_views_in(
    tx: ReadTx, annotation_ids: Sequence[str], instance_id: str, concepts: ConceptOperations
) -> dict[str, AnnotationView]:
    """One materializer serves single reads and corpus capture; no content IO occurs here."""
    output: dict[str, AnnotationView] = {}
    object_ids = list(dict.fromkeys(annotation_ids))
    for start in range(0, len(object_ids), 400):
        chunk = object_ids[start : start + 400]
        annotations = (
            (
                await tx.connection.execute(
                    sa.select(
                        schema.annotations,
                        written.records.c.id.label("record_id"),
                        written.records.c.identifier.label("record_identifier"),
                        written.records.c.kind.label("record_kind"),
                        written.records.c.namespace_id,
                        written.revisions.c.ordinal.label("record_ordinal"),
                        written.namespaces.c.content_generation,
                        processing_results.c.identifier.label("proposal_identifier"),
                        schema.annotation_heads.c.annotation_id.label("active_annotation"),
                    )
                    .join(
                        written.revisions,
                        written.revisions.c.id == schema.annotations.c.record_revision_id,
                    )
                    .join(written.records, written.records.c.id == written.revisions.c.record_id)
                    .join(
                        written.namespaces,
                        written.namespaces.c.id == written.records.c.namespace_id,
                    )
                    .join(
                        processing_results,
                        processing_results.c.id == schema.annotations.c.proposal_result_id,
                    )
                    .outerjoin(
                        schema.annotation_heads,
                        schema.annotation_heads.c.record_revision_id == written.revisions.c.id,
                    )
                    .where(schema.annotations.c.id.in_(chunk))
                )
            )
            .mappings()
            .all()
        )
        data: dict[str, list[RowMapping]] = {}
        for table in (
            schema.bindings,
            schema.occurrences,
            schema.blocks,
            schema.paths,
            schema.path_steps,
        ):
            query = sa.select(table).where(table.c.annotation_id.in_(chunk))
            if "ordinal" in table.c:
                query = query.order_by(table.c.ordinal)
            data[table.name] = list((await tx.connection.execute(query)).mappings())
        meanings = await concepts.read_revisions_in(
            tx, [row["concept_revision_id"] for row in data[schema.bindings.name]]
        )
        bindings: dict[str, dict[str, ConceptView]] = defaultdict(dict)
        for row in data[schema.bindings.name]:
            bindings[row["annotation_id"]][row["local_key"]] = meanings[row["concept_revision_id"]]
        blocks: dict[tuple[str, str], list[RowMapping]] = defaultdict(list)
        for row in data[schema.blocks.name]:
            blocks[row["annotation_id"], row["occurrence_key"]].append(row)
        occurrences: dict[str, list[BoundOccurrence]] = defaultdict(list)
        for row in data[schema.occurrences.name]:
            parts = blocks[row["annotation_id"], row["key"]]
            selections = tuple(ContentSelection.model_validate(part["selection"]) for part in parts)
            if not selections or any(
                part.selection.start_byte is None or part.selection.end_byte is None
                for part in selections
            ):
                raise NousError(
                    "cognition.annotation_invalid", "Annotation has no exact block selection"
                )
            occurrences[row["annotation_id"]].append(
                BoundOccurrence(
                    key=row["key"],
                    concept_key=row["concept_key"],
                    blocks=tuple(part["block_key"] for part in parts),
                    selections=selections,
                )
            )
        steps: dict[tuple[str, str], list[str]] = defaultdict(list)
        for row in data[schema.path_steps.name]:
            steps[row["annotation_id"], row["path_key"]].append(row["occurrence_key"])
        paths: dict[str, list[BoundPath]] = defaultdict(list)
        for row in data[schema.paths.name]:
            paths[row["annotation_id"]].append(
                BoundPath(
                    key=row["key"],
                    label=row["label"],
                    occurrences=tuple(steps[row["annotation_id"], row["key"]]),
                )
            )
        for row in annotations:
            record = RecordRef(
                instance_id=instance_id,
                namespace_id=row["namespace_id"],
                kind=row["record_kind"],
                id=row["record_id"],
                identifier=row["record_identifier"],
            )
            bound = bindings[row["id"]]
            if any(
                meaning.ref.concept_ref.namespace_id != record.namespace_id
                for meaning in bound.values()
            ):
                raise NousError(
                    "cognition.namespace_mismatch",
                    "Annotation meaning belongs to another namespace",
                )
            if any(
                part.artifact.id != row["body_artifact_id"]
                or part.artifact.instance_id != instance_id
                for occurrence in occurrences[row["id"]]
                for part in occurrence.selections
            ):
                raise NousError(
                    "cognition.annotation_body", "Annotation selection belongs to another body"
                )
            output[row["id"]] = AnnotationView(
                ref=AnnotationRef(
                    instance_id=instance_id,
                    namespace_id=record.namespace_id,
                    id=row["id"],
                    identifier=row["identifier"],
                    body_revision=RecordRevisionRef(
                        record_ref=record,
                        revision_id=row["record_revision_id"],
                        ordinal=row["record_ordinal"],
                    ),
                ),
                proposal_result=ResultRef(
                    instance_id=instance_id,
                    id=row["proposal_result_id"],
                    identifier=row["proposal_identifier"],
                ),
                parser_profile=row["parser_profile"],
                bindings=bound,
                occurrences=tuple(occurrences[row["id"]]),
                paths=tuple(paths[row["id"]]),
                active=row["active_annotation"] == row["id"],
                generation=row["content_generation"],
            )
    missing = set(object_ids) - output.keys()
    if missing:
        raise NousError(
            "cognition.annotation_not_found",
            "Annotation does not exist",
            details={"annotation_ids": sorted(missing)},
        )
    return output
