"""Resolve exact cognitive materials through their domain owners."""

from collections.abc import Sequence
from typing import TYPE_CHECKING, cast

import sqlalchemy as sa
from pydantic import JsonValue

from nous.application.contracts import CallContext, define_operation
from nous.cognition.contracts import (
    NamespaceRef,
    NamespaceSelector,
    RecordRevisionRef,
)
from nous.cognition.material_adapters import (
    ArtifactAttributes,
    MaterialAdapters,
    RecordAttributes,
    ResultOutputAttributes,
)
from nous.cognition.material_contracts import (
    CognitiveMaterialView,
    FollowMaterial,
    MaterialRead,
    MaterialRef,
    MaterialRelation,
    MaterialSection,
    MaterialSelection,
    MaterialSnapshot,
    MaterialSource,
    MaterialSourceRead,
    ReadMaterial,
    RelationRead,
)
from nous.cognition.schema import source_materials
from nous.cognition.work.contracts import WorkAttributes
from nous.cognition.work.refs import WorkRevisionRef
from nous.common.errors import NousError
from nous.common.identifier import identifier_type, split_revision
from nous.content.contracts import (
    ArtifactRef,
    ContentSelection,
    ProcessingResult,
    ResultOutputRef,
    is_text_media_type,
)
from nous.content.schema import material_sources
from nous.storage.database import ReadTx, WriteTx


async def upgrade_material_source_refs_in(tx: WriteTx) -> None:
    """Protect existing explicitly saved result-output identities with foreign keys."""

    def columns_for(connection: sa.Connection, name: str) -> set[str]:
        return {item["name"] for item in sa.inspect(connection).get_columns(name)}

    for table in (material_sources, source_materials):
        columns = await tx.connection.run_sync(columns_for, table.name)
        if "source_result_id" in columns:
            continue
        await tx.connection.exec_driver_sql(
            f'ALTER TABLE "{table.name}" ADD COLUMN source_result_id VARCHAR '
            "REFERENCES processing_results(id)"
        )
        await tx.connection.exec_driver_sql(
            f"UPDATE \"{table.name}\" SET source_result_id = json_extract(material, '$.result.id') "
            "WHERE json_extract(material, '$.result.kind') = 'result' "
            "AND json_extract(material, '$.output') IS NOT NULL"
        )


if TYPE_CHECKING:
    from nous.cognition.annotation.operations import AnnotationOperations
    from nous.cognition.work.reading import WorkReader
    from nous.cognition.writing.reading import RecordReader
    from nous.content.artifacts import ArtifactStore
    from nous.content.results import ResultStore


READ_MATERIAL = define_operation(
    "materials.read",
    request=ReadMaterial,
    result=MaterialRead,
    description="Read a material selection or a key in a saved evidence delivery.",
)
FOLLOW_MATERIAL = define_operation(
    "materials.follow",
    request=FollowMaterial,
    result=RelationRead,
    description="Follow formal relations from exact material.",
)


class Materials:
    """Shared material reads and captures backed by the domain owners."""

    def __init__(
        self,
        artifacts: ArtifactStore,
        results: ResultStore,
        records: RecordReader,
        annotations: AnnotationOperations,
        work: WorkReader,
    ):
        self.artifacts, self.results, self.records, self.work = artifacts, results, records, work
        self.adapters = MaterialAdapters(artifacts, results, records, annotations)

    def key(self, ref: MaterialRef) -> str:
        if isinstance(ref, ArtifactRef):
            return self.adapters.artifact_key(ref)
        if isinstance(ref, ResultOutputRef):
            return self.adapters.output_key(ref)
        if isinstance(ref, RecordRevisionRef):
            return self.adapters.record_key(ref)
        return self.work.material_key(ref)

    def searchable_kinds(self) -> tuple[str, ...]:
        return ("journal", "note", "work")

    def describe(self) -> dict[str, dict[str, JsonValue]]:
        return {
            key: cast(dict[str, JsonValue], attributes.model_json_schema())
            for key, attributes in (
                ("artifact", ArtifactAttributes),
                ("result_output", ResultOutputAttributes),
                ("record", RecordAttributes),
                ("work", WorkAttributes),
            )
        }

    async def resolve_identifier(
        self, target: str | MaterialRef, *, namespace: NamespaceSelector = None
    ) -> MaterialRef:
        if not isinstance(target, str):
            return target
        base, separator, output_name = target.partition(":")
        identifier, _ = split_revision(base)
        prefix = identifier_type(identifier)
        if prefix in {"note", "jrn"} and not separator:
            record = await self.records.read(
                target, namespace=namespace.id if isinstance(namespace, NamespaceRef) else namespace
            )
            return record.revision_ref
        if prefix == "art" and not separator and base == identifier:
            return (await self.artifacts.get(identifier)).ref
        if prefix == "res" and base == identifier:
            result = await self.results.get(identifier)
            if not separator:
                evidence = [item for item in result.outputs if item.name == "evidence"]
                primary = [item for item in result.outputs if item.role == "primary"]
                choices = evidence or primary
                if len(choices) != 1:
                    raise NousError("material.output_required", "Choose a named result output")
                output_name = choices[0].name
            if not output_name:
                raise NousError("material.output_required", "Choose a named result output")
            return ResultOutputRef(result=result.ref, output=output_name)
        if prefix == "work" and not separator:
            return await self.work.resolve_identifier(target, namespace=namespace)
        raise NousError("material.invalid_reference", "Material reference is not recognized")

    async def resolve(
        self, ref: MaterialRef, *, namespace: NamespaceSelector = None
    ) -> CognitiveMaterialView:
        return (await self.resolve_many((ref,), namespace=namespace))[0]

    async def capture_in(
        self, tx: ReadTx, refs: Sequence[MaterialRef], *, namespace: NamespaceSelector = None
    ) -> tuple[MaterialSnapshot, ...]:
        """Capture exact domain facts together in the caller's read transaction."""
        selected = (
            (await self.records.namespace_in(tx, namespace)).ref
            if namespace is not None
            else self.records.bound_namespace
        )
        records = tuple(ref for ref in refs if isinstance(ref, RecordRevisionRef))
        works = tuple(ref for ref in refs if isinstance(ref, WorkRevisionRef))
        artifacts = tuple(ref for ref in refs if isinstance(ref, ArtifactRef))
        outputs = tuple(ref for ref in refs if isinstance(ref, ResultOutputRef))
        if len(records) + len(works) + len(artifacts) + len(outputs) != len(refs):
            raise NousError("material.unknown_type", "Material type is not supported")
        for ref in (*records, *works):
            owner = (
                self.adapters.record_namespace(ref)
                if isinstance(ref, RecordRevisionRef)
                else self.work.material_namespace(ref)
            )
            if owner.instance_id != self.artifacts.instance_id:
                raise NousError(
                    "cognition.instance_mismatch", "Material belongs to another instance"
                )
            if selected is not None and owner != selected:
                raise NousError(
                    "cognition.namespace_mismatch", "Material belongs to another namespace"
                )
        formal = await self.sources_many_in(tx, tuple(ref.result.id for ref in outputs))
        captured = (
            *(await self.adapters.capture_records(tx, records) if records else ()),
            *(await self.work.capture_materials_in(tx, works) if works else ()),
            *(await self.adapters.capture_artifacts(tx, artifacts) if artifacts else ()),
            *(await self.adapters.capture_outputs(tx, outputs, formal) if outputs else ()),
        )
        by_ref = {item.view.ref: item for item in captured}
        return tuple(by_ref[ref] for ref in refs)

    async def current_in(self, tx: ReadTx, namespace: NamespaceRef) -> tuple[MaterialSnapshot, ...]:
        """Capture current records and Work in one consistent namespace view."""
        selected = (await self.records.namespace_in(tx, namespace)).ref
        return (
            *await self.adapters.current_records(tx, selected),
            *await self.work.current_materials_in(tx, selected),
        )

    async def resolve_many(
        self, refs: Sequence[MaterialRef], *, namespace: NamespaceSelector = None
    ) -> tuple[CognitiveMaterialView, ...]:
        """Resolve owner facts together, then prepare text structure outside the transaction."""
        async with self.records.database.read() as tx:
            snapshots = await self.capture_in(tx, refs, namespace=namespace)
        views = [snapshot.view for snapshot in snapshots]
        text_indices: list[int] = []
        for index, view in enumerate(views):
            if isinstance(view.ref, WorkRevisionRef):
                views[index] = await self.work.prepare_material(view)
            elif view.body is not None:
                media_type = view.attributes.values.get("media_type", "text/markdown")
                if isinstance(media_type, str) and is_text_media_type(media_type):
                    text_indices.append(index)
        text_views = await self.artifacts.text_views(
            tuple(cast(ContentSelection, views[index].body) for index in text_indices)
        )
        for index, text_view in zip(text_indices, text_views, strict=True):
            sections = tuple(
                MaterialSection(
                    key=block.key,
                    heading_path=block.heading_path,
                    selection=block.selection,
                    structural_context=text_view.structural_context(block.selection),
                )
                for block in text_view.blocks
                if block.parent_key is None
            )
            views[index] = views[index].model_copy(update={"sections": sections})
        return tuple(views)

    async def adopt_sources_in(
        self,
        tx: WriteTx,
        result: ProcessingResult,
        output_names: tuple[str, ...],
        sources: Sequence[MaterialSource],
        *,
        namespace: NamespaceRef,
    ) -> None:
        """Adopt task selections and their formal identities with the named outputs."""
        if not set(output_names).issubset(item.name for item in result.outputs):
            raise NousError("content.not_found", "Material source output does not exist")
        named = {item.key: item.sources for item in result.selections}
        if len({item.key for item in sources}) != len(sources):
            raise NousError("material.duplicate_source", "Material source keys must be unique")
        if any(named.get(item.key) != item.selections for item in sources):
            raise NousError(
                "material.invalid_source", "Material source differs from the task's named selection"
            )
        await self.validate_sources_in(tx, sources, namespace=namespace)
        if sources:
            await tx.connection.execute(
                material_sources.insert(),
                [
                    {
                        "result_id": result.ref.id,
                        "output_name": output_name,
                        "ordinal": ordinal,
                        "key": source.key,
                        "kind": source.kind,
                        "name": source.name,
                        "material": source.material.model_dump(mode="json"),
                        "source_result_id": source.material.result.id
                        if isinstance(source.material, ResultOutputRef)
                        else None,
                        "selections": [item.model_dump(mode="json") for item in source.selections],
                    }
                    for output_name in output_names
                    for ordinal, source in enumerate(sources)
                ],
            )

    async def sources_in(self, tx: ReadTx, ref: ResultOutputRef) -> tuple[MaterialSource, ...]:
        await self.results.resolve_output_in(tx, ref)
        return (await self.sources_many_in(tx, (ref.result.id,))).get(
            (ref.result.id, ref.output), ()
        )

    async def sources_many_in(
        self, tx: ReadTx, result_ids: Sequence[str]
    ) -> dict[tuple[str, str], tuple[MaterialSource, ...]]:
        """Read the formal material sources of completed result outputs."""
        values: dict[tuple[str, str], list[MaterialSource]] = {}
        identifiers = list(dict.fromkeys(result_ids))
        for offset in range(0, len(identifiers), 800):
            rows = (
                await tx.connection.execute(
                    sa.select(material_sources)
                    .where(material_sources.c.result_id.in_(identifiers[offset : offset + 800]))
                    .order_by(
                        material_sources.c.result_id,
                        material_sources.c.output_name,
                        material_sources.c.ordinal,
                    )
                )
            ).mappings()
            for row in rows:
                key = (str(row["result_id"]), str(row["output_name"]))
                values.setdefault(key, []).append(
                    MaterialSource.model_validate(
                        {
                            name: row[name]
                            for name in ("key", "kind", "name", "material", "selections")
                        }
                    )
                )
        return {key: tuple(sources) for key, sources in values.items()}

    async def sources_for(self, ref: ResultOutputRef) -> tuple[MaterialSource, ...]:
        async with self.results.database.read() as tx:
            return await self.sources_in(tx, ref)

    @staticmethod
    def _validate_selections(
        view: CognitiveMaterialView, selections: Sequence[ContentSelection]
    ) -> None:
        allowed = ((view.body,) if view.body is not None else ()) + tuple(
            selection
            for relation in view.relations
            if relation.kind == "representation"
            for selection in relation.evidence
        )
        for selection in selections:
            for candidate in allowed:
                if selection.artifact == candidate.artifact:
                    try:
                        if candidate.selection.restrict(selection.selection) == selection.selection:
                            break
                    except NousError:
                        continue
            else:
                raise NousError("material.invalid_selection", "Selection is outside the material")

    async def validate_sources_in(
        self, tx: ReadTx, sources: Sequence[MaterialSource], *, namespace: NamespaceRef
    ) -> None:
        """Validate formal source identities and ranges inside the adopting transaction."""
        snapshots = await self.capture_in(
            tx, tuple(item.material for item in sources), namespace=namespace
        )
        for source, snapshot in zip(sources, snapshots, strict=True):
            self._validate_selections(snapshot.view, source.selections)

    @classmethod
    def _selections(
        cls, items: Sequence[MaterialSelection], views: Sequence[CognitiveMaterialView]
    ) -> tuple[tuple[ContentSelection, ...], ...]:
        selected: list[tuple[ContentSelection, ...]] = []
        for item, view in zip(items, views, strict=True):
            ranges = item.selections or ((view.body,) if view.body is not None else ())
            cls._validate_selections(view, ranges)
            selected.append(ranges)
        return tuple(selected)

    async def selections(
        self, items: Sequence[MaterialSelection], *, namespace: NamespaceSelector = None
    ) -> tuple[tuple[CognitiveMaterialView, tuple[ContentSelection, ...]], ...]:
        """Resolve material identities and ranges without reading their bodies."""
        async with self.records.database.read() as tx:
            snapshots = await self.capture_in(
                tx, tuple(item.material for item in items), namespace=namespace
            )
        views = tuple(item.view for item in snapshots)
        return tuple(zip(views, self._selections(items, views), strict=True))

    async def read_many(
        self, items: Sequence[MaterialSelection], *, namespace: NamespaceSelector = None
    ) -> tuple[MaterialRead, ...]:
        views = await self.resolve_many(tuple(item.material for item in items), namespace=namespace)
        batches = self._selections(items, views)
        contents = await self.artifacts.read_view_many(
            tuple(value for batch in batches for value in batch)
        )
        offset = 0
        result: list[MaterialRead] = []
        for view, batch in zip(views, batches, strict=True):
            result.append(
                MaterialRead(material=view, selections=contents[offset : offset + len(batch)])
            )
            offset += len(batch)
        return tuple(result)

    async def follow(
        self,
        ref: MaterialRef,
        relation: str | None = None,
        limit: int = 100,
        *,
        namespace: NamespaceSelector = None,
    ) -> tuple[MaterialRelation, ...]:
        if limit < 1:
            raise NousError("material.invalid_limit", "Relation limit must be positive")
        view = await self.resolve(ref, namespace=namespace)
        return tuple(item for item in view.relations if relation is None or item.kind == relation)[
            :limit
        ]

    async def read(self, request: ReadMaterial, call: CallContext) -> MaterialRead:
        if request.ref is not None:
            ref = await self.resolve_identifier(request.ref, namespace=request.namespace)
            result = (
                await self.read_many(
                    (MaterialSelection(material=ref),), namespace=request.namespace
                )
            )[0]
        elif request.selection is not None:
            result = (await self.read_many((request.selection,), namespace=request.namespace))[0]
        else:
            from nous.cognition.retrieval.contracts import EvidenceDelivery

            assert request.delivery is not None and request.key is not None
            evidence = await self.results.get(request.delivery)
            artifact = next(
                (item.artifact for item in evidence.outputs if item.name == "evidence"), None
            )
            if artifact is None:
                raise NousError(
                    "content.not_found",
                    "Result output does not exist",
                    details={"result": evidence.ref.identifier, "output": "evidence"},
                )
            delivery = EvidenceDelivery.model_validate_json(
                await self.artifacts.read_text(artifact)
            )
            delivery_namespace = delivery.intent.constraints.namespace
            async with self.records.database.read() as tx:
                selected_namespace = await self.records.namespace_in(
                    tx, request.namespace if request.namespace is not None else delivery_namespace
                )
            if selected_namespace.ref != delivery_namespace:
                raise NousError(
                    "cognition.namespace_mismatch", "Evidence belongs to another namespace"
                )
            matches = [item for item in delivery.passages if item.key == request.key]
            if len(matches) != 1:
                raise NousError("material.evidence_unknown", "Evidence key is not in this delivery")
            passage = matches[0]
            view = await self.resolve(passage.material, namespace=delivery_namespace)
            selected = passage.selections
            result = MaterialRead(
                material=view, selections=await self.artifacts.read_view_many(selected)
            )
        if request.blocks:
            if result.material.body is None:
                raise NousError("material.no_content", "Material has no text body")
            text_view = await self.artifacts.text_view(result.material.body)
            selected = tuple(text_view.block(key).selection for key in request.blocks)
            result = result.model_copy(
                update={"selections": await self.artifacts.read_view_many(selected)}
            )
        if request.sources:
            sources: list[MaterialSource] = list(result.material.material_sources)
            for binding in result.material.sources:
                if binding.owner_revision is not None:
                    candidate = MaterialSource(
                        key=binding.key,
                        material=binding.owner_revision,
                        selections=binding.selections,
                        kind=binding.usage,
                        name=binding.name,
                    )
                    if not any(
                        item.key == candidate.key and item.selections == candidate.selections
                        for item in sources
                    ):
                        sources.append(candidate)
                else:
                    for part in binding.selections:
                        if not any(
                            item.key == binding.key and part in item.selections for item in sources
                        ):
                            sources.append(
                                MaterialSource(
                                    key=binding.key,
                                    material=part.artifact,
                                    selections=(part,),
                                    kind=binding.usage,
                                    name=binding.name,
                                )
                            )
            known = {(item.material, item.selections) for item in sources}
            for index, relation in enumerate(result.material.relations, 1):
                if relation.evidence and (relation.target, relation.evidence) not in known:
                    sources.append(
                        MaterialSource(
                            key=f"R{index}",
                            material=relation.target,
                            selections=relation.evidence,
                            kind=relation.kind,
                        )
                    )
                    known.add((relation.target, relation.evidence))
            values = await self.artifacts.read_view_many(
                tuple(selection for source in sources for selection in source.selections)
            )
            offset = 0
            details: list[MaterialSourceRead] = []
            for source in sources:
                details.append(
                    MaterialSourceRead(
                        source=source, views=values[offset : offset + len(source.selections)]
                    )
                )
                offset += len(source.selections)
            result = result.model_copy(update={"sources": tuple(details)})
        return result

    async def follow_request(self, request: FollowMaterial, call: CallContext) -> RelationRead:
        ref = await self.resolve_identifier(request.ref, namespace=request.namespace)
        return RelationRead(
            ref=ref,
            relations=await self.follow(
                ref, request.relation, request.limit, namespace=request.namespace
            ),
        )
