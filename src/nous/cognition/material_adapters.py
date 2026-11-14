"""Adapt built-in domain facts to the common material capture contract."""

from __future__ import annotations

from datetime import datetime
from typing import TYPE_CHECKING

from pydantic import BaseModel, ConfigDict, JsonValue

from nous.cognition.concept_contracts import AnnotationView
from nous.cognition.contracts import NamespaceRef, RecordRevisionRef, SourceBinding
from nous.cognition.material_contracts import (
    CognitiveMaterialView,
    MaterialAttributes,
    MaterialIndex,
    MaterialRelation,
    MaterialRoot,
    MaterialSnapshot,
    MaterialSource,
)
from nous.common.errors import NousError
from nous.content.contracts import (
    ArtifactRef,
    ContentSelection,
    InputGroup,
    NamedSelection,
    OutputMapping,
    OutputRole,
    ProducerFacts,
    ResultOutput,
    ResultOutputRef,
    SelectedInputs,
)
from nous.content.selections import subtract_selection
from nous.storage.database import ReadTx
from nous.temporal.values import CognitiveTimes

if TYPE_CHECKING:
    from nous.cognition.annotation.operations import AnnotationOperations
    from nous.cognition.writing.reading import RecordReader
    from nous.content.artifacts import ArtifactStore
    from nous.content.results import ResultStore


class _Attributes(BaseModel):
    model_config = ConfigDict(extra="forbid", frozen=True)


class ArtifactAttributes(_Attributes):
    media_type: str
    source: str | None
    received_at: datetime | None
    formed_at: datetime | None
    recorded_at: datetime


class RecordAttributes(_Attributes):
    current_revision: RecordRevisionRef
    committed_generation: int
    title_source: str
    purpose: str | None
    author: str | None
    viewpoint: str | None
    archived: bool
    recall: bool
    change_kind: str
    reason: str | None
    active_annotation: AnnotationView | None
    needs_review: bool
    source_signature: str
    original_body: ArtifactRef | None


class ResultOutputAttributes(_Attributes):
    operation: str
    role: OutputRole
    representation_kind: str | None
    producer: ProducerFacts
    request: dict[str, JsonValue]
    parameters: dict[str, JsonValue]
    coverage: dict[str, JsonValue]
    selected_inputs: SelectedInputs
    inputs: tuple[InputGroup, ...]
    mappings: tuple[OutputMapping, ...]
    selections: tuple[NamedSelection, ...]
    media_type: str


_SUPPORT_RELATIONS = frozenset(
    {"reference", "current_basis", "source", "derived", "identity", "atomic"}
)


class MaterialAdapters:
    """Build material snapshots from facts captured by their domain owners."""

    def __init__(
        self,
        artifacts: ArtifactStore,
        results: ResultStore,
        records: RecordReader,
        annotations: AnnotationOperations,
    ) -> None:
        self.artifacts, self.results, self.records, self.annotations = (
            artifacts,
            results,
            records,
            annotations,
        )

    @staticmethod
    def artifact_key(ref: ArtifactRef) -> str:
        return "artifact:" + ref.id

    @staticmethod
    def output_key(ref: ResultOutputRef) -> str:
        return "result:" + ref.result.id + ":" + ref.output

    @staticmethod
    def record_key(ref: RecordRevisionRef) -> str:
        return "revision:" + ref.revision_id

    @staticmethod
    def record_namespace(ref: RecordRevisionRef) -> NamespaceRef:
        return NamespaceRef(instance_id=ref.record_ref.instance_id, id=ref.record_ref.namespace_id)

    async def current_records(
        self, tx: ReadTx, namespace: NamespaceRef
    ) -> tuple[MaterialSnapshot, ...]:
        refs = await self.records.current_refs_in(tx, namespace)
        return await self.capture_records(tx, refs)

    async def capture_records(
        self, tx: ReadTx, refs: tuple[RecordRevisionRef, ...]
    ) -> tuple[MaterialSnapshot, ...]:
        from nous.cognition.basis import needs_review_in

        captured = await self.records.capture_many_in(tx, refs)
        annotations = await self.annotations.read_many_in(tx, refs)
        revision_ids = list(dict.fromkeys(ref.revision_id for ref in refs))
        review: set[str] = set()
        for offset in range(0, len(revision_ids), 800):
            review.update(await needs_review_in(tx, revision_ids[offset : offset + 800]))
        output: list[MaterialSnapshot] = []
        for ref, item in zip(refs, captured, strict=True):
            row, revision = item.record, item.revision
            attrs = RecordAttributes(
                current_revision=self.records.revision_ref(row, item.head),
                committed_generation=revision["committed_generation"],
                title_source=revision["title_source"],
                purpose=revision["purpose"],
                author=revision["author"],
                viewpoint=revision["viewpoint"],
                archived=row["archived"],
                recall=row["recall"],
                change_kind=revision["change_kind"],
                reason=revision["reason"],
                active_annotation=annotations.get(ref.revision_id),
                needs_review=ref.revision_id in review,
                source_signature=revision["source_signature"],
                original_body=item.original_body,
            )
            formal_sources = item.material_sources
            relations = [
                MaterialRelation(
                    kind=source.kind, target=source.material, evidence=source.selections
                )
                for source in formal_sources
            ]
            for binding in item.sources:
                if any(
                    source.key == binding.key and source.selections == binding.selections
                    for source in formal_sources
                ):
                    continue
                if binding.owner_revision is not None:
                    relations.append(
                        MaterialRelation(
                            kind=binding.usage,
                            target=binding.owner_revision,
                            evidence=binding.selections,
                        )
                    )
                else:
                    relations.extend(
                        MaterialRelation(kind=binding.usage, target=part.artifact, evidence=(part,))
                        for part in binding.selections
                    )
            view = CognitiveMaterialView(
                ref=ref,
                kind=row["kind"],
                title=revision["title"],
                body=ContentSelection(artifact=item.body),
                sources=item.sources,
                material_sources=tuple(formal_sources),
                relations=tuple(relations),
                times=CognitiveTimes.model_validate(revision["cognitive_times"]),
                attributes=MaterialAttributes(
                    schema_key="record", values=attrs.model_dump(mode="json")
                ),
            )
            output.append(
                MaterialSnapshot(
                    view=view,
                    body_digest=item.body_digest,
                    index=MaterialIndex(
                        key=self.record_key(ref),
                        group_key=ref.identifier,
                        namespace=NamespaceRef(
                            instance_id=ref.record_ref.instance_id, id=ref.record_ref.namespace_id
                        ),
                        available=not row["archived"] and row["recall"],
                        purpose=attrs.purpose,
                        needs_review=attrs.needs_review,
                        annotation=attrs.active_annotation,
                        interpretation=row["kind"] == "journal",
                        root=MaterialRoot(key="record:" + row["id"], artifact=item.original_body)
                        if item.original_body
                        else None,
                        support_relations=tuple(
                            relation
                            for relation in relations
                            if relation.kind in _SUPPORT_RELATIONS
                        ),
                    ),
                )
            )
        return tuple(output)

    async def capture_artifacts(
        self, tx: ReadTx, refs: tuple[ArtifactRef, ...]
    ) -> tuple[MaterialSnapshot, ...]:
        artifacts = await self.artifacts.get_many_in(tx, refs)
        blobs = await self.artifacts.content.resolve_many_in(tx, [ref.id for ref in refs])
        produced = await self.results.outputs_for_artifacts_in(tx, refs)
        output: list[MaterialSnapshot] = []
        for artifact, blob in zip(artifacts, blobs, strict=True):
            attrs = ArtifactAttributes.model_validate(
                artifact.model_dump(include=set(ArtifactAttributes.model_fields))
            )
            relations = ()
            if artifact.ref.id in produced:
                relations = (
                    MaterialRelation(
                        kind="produced_by",
                        target=produced[artifact.ref.id],
                    ),
                )
            output.append(
                MaterialSnapshot(
                    view=CognitiveMaterialView(
                        ref=artifact.ref,
                        kind="artifact",
                        title=artifact.display_name,
                        body=ContentSelection(artifact=artifact.ref),
                        relations=relations,
                        attributes=MaterialAttributes(
                            schema_key="artifact", values=attrs.model_dump(mode="json")
                        ),
                    ),
                    body_digest=blob.digest,
                    index=MaterialIndex(
                        key=self.artifact_key(artifact.ref),
                        group_key=artifact.ref.identifier,
                        available=False,
                        provenance=artifact.ref,
                    ),
                )
            )
        return tuple(output)

    async def capture_outputs(
        self,
        tx: ReadTx,
        refs: tuple[ResultOutputRef, ...],
        formal: dict[tuple[str, str], tuple[MaterialSource, ...]],
    ) -> tuple[MaterialSnapshot, ...]:
        values = await self.results.get_many_in(tx, [ref.result for ref in refs])
        results = {value.ref.id: value for value in values}
        selected: list[ResultOutput] = []
        for ref in refs:
            result = results[ref.result.id]
            item = next((item for item in result.outputs if item.name == ref.output), None)
            if item is None:
                raise NousError("content.not_found", "Result output does not exist")
            selected.append(item)
        artifacts = await self.artifacts.get_many_in(tx, [item.artifact for item in selected])
        blobs = await self.artifacts.content.resolve_many_in(
            tx, [item.artifact.id for item in selected]
        )
        output: list[MaterialSnapshot] = []
        for ref, item, artifact, blob in zip(refs, selected, artifacts, blobs, strict=True):
            result = results[ref.result.id]
            body = ContentSelection(artifact=artifact.ref)
            attrs = ResultOutputAttributes(
                operation=result.operation,
                role=item.role,
                representation_kind=item.representation_kind,
                producer=result.producer,
                request=result.request,
                parameters=result.parameters,
                coverage=result.coverage,
                selected_inputs=result.selected_inputs,
                inputs=result.inputs,
                mappings=tuple(
                    mapping for mapping in result.mappings if mapping.output_name == ref.output
                ),
                selections=result.selections,
                media_type=artifact.media_type,
            )
            formal_sources = tuple(formal.get((ref.result.id, ref.output), ()))
            sources = (
                ()
                if formal_sources
                else tuple(
                    SourceBinding(
                        key=source.slot_key, selections=source.selections, name=source.display_name
                    )
                    for source in result.selected_inputs.sources
                )
            )
            source_relations = tuple(
                MaterialRelation(
                    kind=source.kind, target=source.material, evidence=source.selections
                )
                for source in formal_sources
            )
            mapping_relations = tuple(
                MaterialRelation(
                    kind=mapping.relation,
                    target=origin.artifact,
                    scope=(body.model_copy(update={"selection": mapping.selection}),),
                    evidence=(origin,),
                )
                for mapping in result.mappings
                if mapping.output_name == ref.output
                for origin in mapping.origins
            )
            relations = source_relations + mapping_relations
            covered = tuple(part for source in formal_sources for part in source.selections)
            support_relations = tuple(
                relation for relation in source_relations if relation.kind in _SUPPORT_RELATIONS
            ) + tuple(
                relation.model_copy(update={"evidence": (part,)})
                for relation in mapping_relations
                for origin in relation.evidence
                for part in subtract_selection(origin, covered)
            )
            output.append(
                MaterialSnapshot(
                    view=CognitiveMaterialView(
                        ref=ref,
                        kind="result_output",
                        title=artifact.display_name,
                        body=body,
                        sources=sources,
                        material_sources=formal_sources,
                        relations=relations,
                        attributes=MaterialAttributes(
                            schema_key="result_output", values=attrs.model_dump(mode="json")
                        ),
                    ),
                    body_digest=blob.digest,
                    index=MaterialIndex(
                        key=self.output_key(ref),
                        group_key=f"{ref.result.identifier}:{ref.output}",
                        available=False,
                        provenance=artifact.ref if not formal_sources else None,
                        support_relations=support_relations,
                    ),
                )
            )
        return tuple(output)
