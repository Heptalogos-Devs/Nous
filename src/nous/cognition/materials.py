"""Resolve exact cognitive materials through their domain owners."""

from collections.abc import Sequence
from typing import TYPE_CHECKING

from nous.application.contracts import CallContext, define_operation
from nous.cognition.contracts import NamespaceRef, NamespaceSelector
from nous.cognition.material_adapters import MaterialAdapters
from nous.cognition.material_contracts import (
    CognitiveMaterialView,
    FollowMaterial,
    MaterialRead,
    MaterialReadBatch,
    MaterialRelation,
    MaterialSection,
    MaterialSelection,
    ReadMaterial,
    RelationRead,
)
from nous.common.errors import NousError
from nous.common.identifier import identifier_type, split_revision
from nous.common.material_refs import RecordRevisionRef, WorkRevisionRef
from nous.content.contracts import (
    ArtifactRef,
    ContentReadView,
    ContentSelection,
    DeliveryBudget,
    MaterialRef,
    MaterialSource,
    ResultOutputRef,
    is_text_media_type,
)
from nous.storage.database import ReadTx

if TYPE_CHECKING:
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
        work: WorkReader,
    ):
        self.artifacts, self.results, self.records, self.work = artifacts, results, records, work
        self.adapters = MaterialAdapters(artifacts, results, records)

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

    async def resolve_identifier(
        self, target: str | MaterialRef, *, namespace: NamespaceSelector = None
    ) -> MaterialRef:
        if not isinstance(target, str):
            return target
        base, separator, output_name = target.partition(":")
        identifier, _ = split_revision(base)
        prefix = identifier_type(identifier)
        if prefix in {"note", "jrn"} and not separator:
            selected = namespace.id if isinstance(namespace, NamespaceRef) else namespace
            async with self.records.database.read() as tx:
                record, revision, _ = await self.records.resolve_in(tx, target, selected)
            return self.records.revision_ref(record, revision)
        if prefix == "art" and not separator and base == identifier:
            return (await self.artifacts.get(identifier)).ref
        if prefix == "res" and base == identifier:
            if separator and not output_name:
                raise NousError("material.output_required", "Choose a named result output")
            result, _ = await self.results.resolve_output(
                identifier, output_name if separator else None
            )
            return result
        if prefix == "work" and not separator:
            return await self.work.resolve_identifier(target, namespace=namespace)
        raise NousError("material.invalid_reference", "Material reference is not recognized")

    async def resolve(
        self, ref: MaterialRef, *, namespace: NamespaceSelector = None
    ) -> CognitiveMaterialView:
        return (await self.resolve_many((ref,), namespace=namespace))[0]

    async def capture_in(
        self,
        tx: ReadTx,
        refs: Sequence[MaterialRef],
        *,
        namespace: NamespaceSelector = None,
        structure: bool = False,
    ) -> tuple[CognitiveMaterialView, ...]:
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
        captured = (
            *(await self.adapters.capture_records(tx, records) if records else ()),
            *(
                await self.work.capture_materials_in(tx, works, structure=structure)
                if works
                else ()
            ),
            *(await self.adapters.capture_artifacts(tx, artifacts) if artifacts else ()),
            *(await self.adapters.capture_outputs(tx, outputs) if outputs else ()),
        )
        by_ref = {item.ref: item for item in captured}
        return tuple(by_ref[ref] for ref in refs)

    async def current_in(
        self, tx: ReadTx, namespace: NamespaceRef
    ) -> tuple[CognitiveMaterialView, ...]:
        """Capture current records and Work in one consistent namespace view."""
        selected = (await self.records.namespace_in(tx, namespace)).ref
        return (
            *await self.adapters.current_records(tx, selected),
            *await self.work.current_materials_in(tx, selected),
        )

    async def resolve_many(
        self, refs: Sequence[MaterialRef], *, namespace: NamespaceSelector = None
    ) -> tuple[CognitiveMaterialView, ...]:
        """Resolve material facts without reading their content."""
        async with self.records.database.read() as tx:
            return await self.capture_in(tx, refs, namespace=namespace)

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
        self.validate_captured_sources(sources, snapshots)

    @classmethod
    def validate_captured_sources(
        cls, sources: Sequence[MaterialSource], views: Sequence[CognitiveMaterialView]
    ) -> None:
        """Check selections against material identities already captured by this owner."""
        for source, view in zip(sources, views, strict=True):
            if source.material != view.ref:
                raise NousError("material.invalid_selection", "Source material does not match")
            cls._validate_selections(view, source.selections)

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
        return tuple(zip(snapshots, self._selections(items, snapshots), strict=True))

    async def read_many(
        self,
        items: Sequence[MaterialSelection],
        *,
        namespace: NamespaceSelector = None,
        include_sources: bool = False,
        structure: bool = False,
        blocks: tuple[str, ...] = (),
    ) -> tuple[MaterialRead, ...]:
        return (
            await self._read_many(
                items,
                namespace=namespace,
                include_sources=include_sources,
                structure=structure,
                blocks=blocks,
            )
        ).readings

    async def read_budgeted(
        self,
        targets: Sequence[str | MaterialRef | MaterialSelection],
        budget: DeliveryBudget | None,
        *,
        namespace: NamespaceSelector = None,
        return_types: tuple[str, ...] = (),
        limit: int | None = None,
    ) -> MaterialReadBatch:
        """Read explicit materials with a shared budget and exact unread ranges."""
        items: list[MaterialSelection] = []
        for target in targets:
            if isinstance(target, MaterialSelection):
                item = target
            else:
                item = MaterialSelection(
                    material=await self.resolve_identifier(target, namespace=namespace)
                )
            if item not in items:
                items.append(item)
        return await self._read_many(
            items,
            namespace=namespace,
            budget=budget,
            return_types=return_types,
            limit=limit,
            count_usage=True,
        )

    async def _read_many(
        self,
        items: Sequence[MaterialSelection],
        *,
        namespace: NamespaceSelector = None,
        budget: DeliveryBudget | None = None,
        return_types: tuple[str, ...] = (),
        limit: int | None = None,
        include_sources: bool = False,
        count_usage: bool = False,
        structure: bool = False,
        blocks: tuple[str, ...] = (),
    ) -> MaterialReadBatch:
        async with self.records.database.read() as tx:
            views = await self.capture_in(
                tx, tuple(item.material for item in items), namespace=namespace, structure=structure
            )
        block_view = None
        if blocks:
            if len(items) != 1 or views[0].body is None:
                raise NousError("material.no_content", "Material has no text body")
            block_view = await self.artifacts.text_view(views[0].body)
            items = (
                MaterialSelection(
                    material=items[0].material,
                    selections=tuple(block_view.block(key).selection for key in blocks),
                ),
            )
        selected = [
            (item, view)
            for item, view in zip(items, views, strict=True)
            if not return_types
            or view.kind in return_types
            or ("cognition" in return_types and view.kind in self.searchable_kinds())
        ]
        if limit is not None:
            selected = selected[:limit]
        ranges = self._selections(
            tuple(item for item, _ in selected), tuple(view for _, view in selected)
        )
        body_selections = tuple(item for group in ranges for item in group)
        sources = (
            tuple(source for _, view in selected for source in view.material_sources)
            if include_sources
            else ()
        )
        source_selections = tuple(item for source in sources for item in source.selections)
        counter = await self.artifacts.selection_budget(budget, ()) if budget is not None else None
        used = 0
        if counter is None:
            content = await self.artifacts.read_view_many(
                (*body_selections, *source_selections),
                representations="metadata" if count_usage else "existing",
            )
            body_views = content[: len(body_selections)]
            source_views = content[len(body_selections) :]
            if count_usage:
                deliveries, used = await self.artifacts.deduplicate_views(body_views)
            else:
                deliveries = ()
        else:
            deliveries = await self.artifacts.read_budgeted_views(body_selections, counter)
            body_views = ()
            source_views = (
                await self.artifacts.read_view_many(source_selections) if source_selections else ()
            )
        readings: list[MaterialRead] = []
        remaining: list[MaterialSelection] = []
        offset = source_offset = 0
        for (_, view), group in zip(selected, ranges, strict=True):
            original_parts = body_views[offset : offset + len(group)] if counter is None else ()
            if counter is None and not count_usage:
                parts = body_views[offset : offset + len(group)]
                shared: tuple[ContentSelection, ...] = ()
                unread: tuple[ContentSelection, ...] = ()
            else:
                chosen = deliveries[offset : offset + len(group)]
                parts = tuple(part for item in chosen for part in item.views)
                shared = tuple(part for item in chosen for part in item.shared)
                unread = tuple(part for item in chosen for part in item.remaining)
            offset += len(group)
            if unread:
                remaining.append(MaterialSelection(material=view.ref, selections=unread))
            source_content: dict[str, tuple[ContentReadView, ...]] = {}
            if include_sources:
                for source in view.material_sources:
                    count = len(source.selections)
                    source_content[source.key] = source_views[source_offset : source_offset + count]
                    source_offset += count
            if (
                structure
                and not isinstance(view.ref, WorkRevisionRef)
                and view.body is not None
                and view.media_type is not None
                and is_text_media_type(view.media_type)
            ):
                text_view = block_view or (
                    counter.views.get(view.body.artifact.id) if counter is not None else None
                )
                full_part = (
                    original_parts[0]
                    if group == (view.body,) and not unread and len(original_parts) == 1
                    else None
                )
                if text_view is None and full_part is not None and full_part.body_text is not None:
                    blob = await self.artifacts.blob(view.body.artifact)
                    text_view, _ = await self.artifacts.text_cache.view(
                        view.body.artifact,
                        blob,
                        full_part.body_text,
                        self.artifacts.content.execution,
                    )
                if text_view is None:
                    text_view = await self.artifacts.text_view(view.body)
                view = view.model_copy(
                    update={
                        "sections": tuple(
                            MaterialSection(
                                key=block.key,
                                heading_path=block.heading_path,
                                selection=block.selection,
                                structural_context=text_view.structural_context(block.selection),
                            )
                            for block in text_view.blocks
                            if block.parent_key is None
                        )
                    }
                )
            if structure and view.sections is None:
                view = view.model_copy(update={"sections": ()})
            readings.append(
                MaterialRead(material=view, selections=parts, shared=shared, sources=source_content)
            )
        if counter is not None:
            used = counter.used
        return MaterialReadBatch(
            readings=tuple(readings),
            remaining=tuple(remaining),
            used=used,
            unit=budget.unit if budget else "characters",
        )

    async def follow(
        self,
        ref: MaterialRef,
        relation: str | None = None,
        limit: int = 100,
        *,
        namespace: NamespaceSelector = None,
    ) -> tuple[MaterialRelation, ...]:
        view = await self.resolve(ref, namespace=namespace)
        return await self.follow_view(view, relation=relation, limit=limit)

    async def follow_view(
        self, view: CognitiveMaterialView, *, relation: str | None = None, limit: int = 100
    ) -> tuple[MaterialRelation, ...]:
        """Expand formal relations from an already captured material."""
        if limit < 1:
            raise NousError("material.invalid_limit", "Relation limit must be positive")
        ref = view.ref
        relations = list(view.relations)
        relations.extend(
            MaterialRelation(kind=source.kind, target=source.material, evidence=source.selections)
            for source in view.material_sources
        )
        if isinstance(ref, ResultOutputRef):
            async with self.results.database.read() as tx:
                mappings = (await self.results.output_mappings_in(tx, (ref,))).get(
                    (ref.result.id, ref.output), ()
                )
            assert view.body is not None
            relations.extend(
                MaterialRelation(
                    kind=mapping.relation,
                    target=origin.artifact,
                    scope=(view.body.model_copy(update={"selection": mapping.selection}),),
                    evidence=(origin,),
                )
                for mapping in mappings
                for origin in mapping.origins
            )
        return tuple(item for item in relations if relation is None or item.kind == relation)[
            :limit
        ]

    async def read(self, request: ReadMaterial, call: CallContext) -> MaterialRead:
        if request.ref is not None:
            ref = await self.resolve_identifier(request.ref, namespace=request.namespace)
            result = (
                await self.read_many(
                    (MaterialSelection(material=ref),),
                    namespace=request.namespace,
                    include_sources=request.sources,
                    structure=request.structure,
                    blocks=request.blocks,
                )
            )[0]
        elif request.selection is not None:
            result = (
                await self.read_many(
                    (request.selection,),
                    namespace=request.namespace,
                    include_sources=request.sources,
                    structure=request.structure,
                )
            )[0]
        else:
            from nous.cognition.retrieval.adoption import read_saved_evidence

            assert request.delivery is not None and request.key is not None
            evidence = await read_saved_evidence(self.results, request.delivery)
            delivery_namespace = evidence.namespace
            async with self.records.database.read() as tx:
                selected_namespace = await self.records.namespace_in(
                    tx, request.namespace if request.namespace is not None else delivery_namespace
                )
            if selected_namespace.ref != delivery_namespace:
                raise NousError(
                    "cognition.namespace_mismatch", "Evidence belongs to another namespace"
                )
            source = evidence.source(request.key)
            result = (
                await self.read_many(
                    (MaterialSelection(material=source.material, selections=source.selections),),
                    namespace=delivery_namespace,
                    include_sources=request.sources,
                    structure=request.structure,
                )
            )[0]
        return result

    async def follow_request(self, request: FollowMaterial, call: CallContext) -> RelationRead:
        ref = await self.resolve_identifier(request.ref, namespace=request.namespace)
        return RelationRead(
            ref=ref,
            relations=await self.follow(
                ref, request.relation, request.limit, namespace=request.namespace
            ),
        )
