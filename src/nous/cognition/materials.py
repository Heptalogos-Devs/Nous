"""Resolve exact cognitive material through its registered owner."""

from collections.abc import Awaitable, Callable, Sequence
from dataclasses import dataclass
from typing import TYPE_CHECKING, cast

import sqlalchemy as sa
from pydantic import BaseModel, JsonValue

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
    MaterialAttributes,
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

if TYPE_CHECKING:
    from nous.cognition.annotation.operations import AnnotationOperations
    from nous.cognition.writing.reading import RecordReader
    from nous.content.artifacts import ArtifactStore
    from nous.content.results import ResultStore


type MaterialCapture = Callable[
    [ReadTx, tuple[MaterialRef, ...]], Awaitable[tuple[MaterialSnapshot, ...]]
]
type MaterialCurrent = Callable[[ReadTx, NamespaceRef], Awaitable[tuple[MaterialSnapshot, ...]]]
type MaterialPrepare = Callable[[CognitiveMaterialView], Awaitable[CognitiveMaterialView]]
type MaterialKey = Callable[[MaterialRef], str]
type MaterialNamespace = Callable[[MaterialRef], NamespaceRef | None]
type IdentifierResolver = Callable[..., Awaitable[MaterialRef]]


@dataclass(frozen=True)
class _Registration:
    schema_key: str
    attributes: type[BaseModel]
    key: MaterialKey
    namespace: MaterialNamespace | None
    kinds: tuple[str, ...]
    capture: MaterialCapture
    current: MaterialCurrent | None
    prepare: MaterialPrepare | None


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
    """Registry and shared read/follow operations; owners remain authoritative."""

    def __init__(
        self,
        artifacts: ArtifactStore,
        results: ResultStore,
        records: RecordReader,
        annotations: AnnotationOperations,
    ):
        self.artifacts, self.results, self.records = artifacts, results, records
        adapters = MaterialAdapters(artifacts, results, records, annotations, self.sources_many_in)
        self._readers: dict[type, _Registration] = {}
        self._identifiers: dict[str, IdentifierResolver] = {}
        self.register(
            ArtifactRef,
            "artifact",
            ArtifactAttributes,
            key=adapters.artifact_key,
            kinds=("artifact",),
            capture_in=adapters.capture_artifacts,
        )
        self.register(
            ResultOutputRef,
            "result_output",
            ResultOutputAttributes,
            key=adapters.output_key,
            kinds=("result_output",),
            capture_in=adapters.capture_outputs,
        )
        self.register(
            RecordRevisionRef,
            "record",
            RecordAttributes,
            key=adapters.record_key,
            namespace=adapters.record_namespace,
            kinds=("note", "journal"),
            capture_in=adapters.capture_records,
            current_in=adapters.current_records,
        )

    def register[T: MaterialRef](
        self,
        ref_type: type[T],
        schema_key: str,
        attributes: type[BaseModel],
        *,
        key: Callable[[T], str],
        kinds: tuple[str, ...],
        capture_in: Callable[[ReadTx, tuple[T, ...]], Awaitable[tuple[MaterialSnapshot, ...]]],
        namespace: Callable[[T], NamespaceRef | None] | None = None,
        current_in: MaterialCurrent | None = None,
        prepare: MaterialPrepare | None = None,
    ) -> None:
        if ref_type in self._readers or any(
            item.schema_key == schema_key for item in self._readers.values()
        ):
            raise ValueError("Material reference and schema keys must be unique")
        if not kinds or len(kinds) != len(set(kinds)):
            raise ValueError("Material kinds must be declared and unique")
        self._readers[ref_type] = _Registration(
            schema_key,
            attributes,
            cast(MaterialKey, key),
            cast(MaterialNamespace | None, namespace),
            kinds,
            cast(MaterialCapture, capture_in),
            current_in,
            prepare,
        )

    def key(self, ref: MaterialRef) -> str:
        entry = self._readers.get(type(ref))
        if entry is None:
            raise NousError("material.unknown_type", "Material type is not registered")
        return entry.key(ref)

    def searchable_kinds(self) -> tuple[str, ...]:
        return tuple(
            sorted(
                {kind for entry in self._readers.values() if entry.current for kind in entry.kinds}
            )
        )

    def describe(self) -> dict[str, dict[str, JsonValue]]:
        return {
            entry.schema_key: cast(dict[str, JsonValue], entry.attributes.model_json_schema())
            for entry in self._readers.values()
        }

    def register_identifier(self, prefix: str, resolver: IdentifierResolver) -> None:
        if prefix in self._identifiers or prefix in {"note", "jrn", "art", "res"}:
            raise ValueError("Material identifier prefix is already registered")
        self._identifiers[prefix] = resolver

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
        if prefix in self._identifiers and not separator:
            return await self._identifiers[prefix](target, namespace=namespace)
        raise NousError("material.invalid_reference", "Material reference is not recognized")

    async def resolve(
        self, ref: MaterialRef, *, namespace: NamespaceSelector = None
    ) -> CognitiveMaterialView:
        return (await self.resolve_many((ref,), namespace=namespace))[0]

    def _validate_snapshot(
        self, snapshot: MaterialSnapshot, entry: _Registration
    ) -> MaterialSnapshot:
        view = snapshot.view
        owner = entry.namespace(view.ref) if entry.namespace else None
        if (
            view.attributes.schema_key != entry.schema_key
            or view.sections
            or view.kind not in entry.kinds
            or snapshot.index.key != entry.key(view.ref)
            or snapshot.index.namespace != owner
        ):
            raise NousError(
                "material.invalid_view", "Material owner returned an inconsistent snapshot"
            )
        if owner is not None:
            if owner.instance_id != self.artifacts.instance_id:
                raise NousError(
                    "cognition.instance_mismatch", "Material belongs to another instance"
                )
            if self.records.bound_namespace is not None and owner != self.records.bound_namespace:
                raise NousError(
                    "cognition.namespace_mismatch", "Material belongs to another namespace"
                )
        if (view.body is None) != (snapshot.body_digest is None):
            raise NousError(
                "material.invalid_view", "Material body and digest must be captured together"
            )
        for artifact in (
            snapshot.index.root.artifact if snapshot.index.root else None,
            snapshot.index.provenance,
        ):
            if artifact is not None and artifact.instance_id != self.artifacts.instance_id:
                raise NousError(
                    "content.instance_mismatch", "Material source belongs to another instance"
                )
        validated = entry.attributes.model_validate(view.attributes.values)
        return snapshot.model_copy(
            update={
                "view": view.model_copy(
                    update={
                        "attributes": MaterialAttributes(
                            schema_key=entry.schema_key, values=validated.model_dump(mode="json")
                        )
                    }
                )
            }
        )

    async def capture_in(
        self, tx: ReadTx, refs: Sequence[MaterialRef], *, namespace: NamespaceSelector = None
    ) -> tuple[MaterialSnapshot, ...]:
        """Capture exact materials in the caller's single consistent read view."""
        selected = (
            (await self.records.namespace_in(tx, namespace)).ref if namespace is not None else None
        )
        groups: dict[type, list[tuple[int, MaterialRef]]] = {}
        for index, ref in enumerate(refs):
            if type(ref) not in self._readers:
                raise NousError("material.unknown_type", "Material type is not registered")
            groups.setdefault(type(ref), []).append((index, ref))
        output: list[MaterialSnapshot | None] = [None] * len(refs)
        for ref_type, group in groups.items():
            entry = self._readers[ref_type]
            snapshots = await entry.capture(tx, tuple(ref for _, ref in group))
            if len(snapshots) != len(group):
                raise NousError(
                    "material.invalid_view", "Material owner returned an incomplete snapshot"
                )
            for (index, ref), snapshot in zip(group, snapshots, strict=True):
                if snapshot.view.ref != ref:
                    raise NousError(
                        "material.invalid_view", "Material owner returned a different reference"
                    )
                value = self._validate_snapshot(snapshot, entry)
                if (
                    selected is not None
                    and value.index.namespace is not None
                    and value.index.namespace != selected
                ):
                    raise NousError(
                        "cognition.namespace_mismatch", "Material belongs to another namespace"
                    )
                output[index] = value
        return tuple(cast(MaterialSnapshot, item) for item in output)

    async def current_in(self, tx: ReadTx, namespace: NamespaceRef) -> tuple[MaterialSnapshot, ...]:
        """Capture all registered current materials without applying retrieval eligibility."""
        selected = (await self.records.namespace_in(tx, namespace)).ref
        result: list[MaterialSnapshot] = []
        keys: set[str] = set()
        for ref_type, entry in self._readers.items():
            if entry.current is None:
                continue
            for snapshot in await entry.current(tx, namespace):
                if type(snapshot.view.ref) is not ref_type:
                    raise NousError(
                        "material.invalid_view", "Material owner returned a different type"
                    )
                value = self._validate_snapshot(snapshot, entry)
                if value.index.namespace != selected:
                    raise NousError(
                        "cognition.namespace_mismatch", "Material belongs to another namespace"
                    )
                if value.index.key in keys:
                    raise NousError("material.invalid_view", "Material keys must be unique")
                keys.add(value.index.key)
                result.append(value)
        return tuple(result)

    async def resolve_many(
        self, refs: Sequence[MaterialRef], *, namespace: NamespaceSelector = None
    ) -> tuple[CognitiveMaterialView, ...]:
        """Resolve owner facts together, then prepare text structure outside the transaction."""
        async with self.records.database.read() as tx:
            snapshots = await self.capture_in(tx, refs, namespace=namespace)
        views = [snapshot.view for snapshot in snapshots]
        text_indices: list[int] = []
        for index, view in enumerate(views):
            entry = self._readers[type(view.ref)]
            if entry.prepare is not None:
                views[index] = await entry.prepare(view)
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
        output_name: str,
        sources: Sequence[MaterialSource],
    ) -> None:
        """Adopt exact material identities with the producer's named output."""
        if output_name not in {item.name for item in result.outputs}:
            raise NousError("content.not_found", "Material source output does not exist")
        named = {item.key: item.sources for item in result.selections}
        if len({item.key for item in sources}) != len(sources):
            raise NousError("material.duplicate_source", "Material source keys must be unique")
        if any(named.get(item.key) != item.selections for item in sources):
            raise NousError(
                "material.invalid_source", "Material source differs from the task's named selection"
            )
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
                        "selections": [item.model_dump(mode="json") for item in source.selections],
                    }
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
        """Read material-owned source relations for completed result outputs together."""
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

    async def read_many(
        self, items: Sequence[MaterialSelection], *, namespace: NamespaceSelector = None
    ) -> tuple[MaterialRead, ...]:
        views = await self.resolve_many(tuple(item.material for item in items), namespace=namespace)
        batches: list[tuple[ContentSelection, ...]] = []
        for item, view in zip(items, views, strict=True):
            selections = item.selections or ((view.body,) if view.body is not None else ())
            self._validate_selections(view, selections)
            batches.append(selections)
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
            artifact = await self.results.resolve_output(
                ResultOutputRef(result=evidence.ref, output="evidence")
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
