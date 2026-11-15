"""Resolve formal source identities and their exact selected representations."""

from collections.abc import Sequence
from typing import TYPE_CHECKING, Literal, cast

from pydantic import TypeAdapter

from nous.common.errors import NousError
from nous.common.identifier import identifier_type
from nous.common.material_refs import RecordRevisionRef, WorkRevisionRef
from nous.content.contracts import (
    ArtifactInput,
    ArtifactRef,
    ContentSelection,
    ExternalInput,
    InlineText,
    InputPart,
    MaterialRef,
    MaterialSource,
    ResultOutputInput,
    ResultOutputRef,
    SelectedInputs,
    SelectedSource,
    SourceRole,
    SourceTarget,
)
from nous.media.preparation import MediaPreparer
from nous.storage.database import ReadTx

from .contracts import NamespaceRef, RevisionSelection
from .material_contracts import CognitiveMaterialView, MaterialSelection
from .writing.contracts import SourceChoice
from .writing.reading import RecordReader, Target

if TYPE_CHECKING:
    from nous.application.inputs import InputReader
    from .materials import Materials

SourceInput = (
    SourceChoice
    | MaterialSource
    | RevisionSelection
    | Target
    | ContentSelection
    | ArtifactRef
    | ResultOutputRef
    | WorkRevisionRef
    | MaterialSelection
    | SourceTarget
)


class SourceResolver:
    """Freeze each formal identity and map consumed representations back to it."""

    def __init__(self, reader: RecordReader, preparer: MediaPreparer, materials: Materials):
        self.reader, self.artifacts, self.preparer = reader, reader.artifacts, preparer
        self.materials = materials

    async def prepare(
        self,
        namespace: NamespaceRef,
        inputs: Sequence[SourceInput],
        *,
        prefix: str = "S",
        reserved_keys: Sequence[str] = (),
        reject_journals: bool = False,
        usage: Literal["reference", "current_basis", "inspiration"] = "reference",
        preserve_keys: bool = False,
        input_reader: InputReader | None = None,
    ) -> SelectedInputs:
        resolved: list[tuple[object, ResultOutputRef | None, str]] = []
        for value in inputs:
            source = value.target if isinstance(value, SourceChoice) else value
            role = value.usage if isinstance(value, SourceChoice) else usage
            hint = None
            if isinstance(source, (ExternalInput, InlineText, ArtifactInput, ResultOutputInput)):
                if input_reader is None:
                    raise NousError(
                        "input.missing_binding", "Source requires the current input reader"
                    )
                if isinstance(source, ResultOutputInput):
                    hint, output = await self.preparer.results.resolve_output(
                        source.result, source.output
                    )
                    artifact = await self.artifacts.get(output)
                    selection = source.selection
                else:
                    artifact, selection = await input_reader.accept_target(source)
                source = ContentSelection(artifact=artifact.ref, selection=selection)
            elif isinstance(source, str) and (
                source.startswith("work(") or source.startswith("res(")
            ):
                source = await self.materials.resolve_identifier(source, namespace=namespace)
            resolved.append((source, hint, role))
        async with self.reader.database.read() as tx:
            ns = await self.reader.namespace_in(tx, namespace)
            bindings: list[MaterialSource] = []
            owners: dict[str, ArtifactRef] = {}
            views: dict[MaterialRef, CognitiveMaterialView] = {}
            index = 0
            for source, hint, role in resolved:
                owner: ArtifactRef | None = None
                while True:
                    index += 1
                    key = f"{prefix}{index}"
                    if not any(old == key or old.startswith(key + ":") for old in reserved_keys):
                        break
                if isinstance(source, MaterialSource):
                    binding = source if preserve_keys else source.model_copy(update={"key": key})
                elif isinstance(source, (MaterialSelection, WorkRevisionRef)):
                    material = source.material if isinstance(source, MaterialSelection) else source
                    view = (await self.materials.capture_in(tx, (material,), namespace=ns.ref))[0]
                    views[material] = view
                    if isinstance(material, RecordRevisionRef) and view.body is not None:
                        owner = view.body.artifact
                    selections = source.selections if isinstance(source, MaterialSelection) else ()
                    if not selections and view.body is not None:
                        selections = (view.body,)
                    if not selections:
                        raise NousError("material.no_content", "Material has no content to cite")
                    binding = MaterialSource(
                        key=key,
                        material=material,
                        selections=selections,
                        kind=role,
                        name=view.title,
                    )
                elif isinstance(source, RevisionSelection):
                    _, revision, _ = await self.reader.resolve_in(tx, source.revision, ns.id)
                    owner = (await self.artifacts.get_in(tx, revision["body_artifact_id"])).ref
                    binding = MaterialSource(
                        key=key,
                        material=source.revision,
                        selections=source.selections,
                        kind=role,
                        name=revision["title"],
                    )
                elif isinstance(source, ContentSelection):
                    artifact = await self.artifacts.get_in(tx, source.artifact)
                    binding = MaterialSource(
                        key=key,
                        material=hint or artifact.ref,
                        selections=(source,),
                        kind=role,
                        name=artifact.display_name,
                    )
                elif isinstance(source, ResultOutputRef):
                    _, ref = (
                        await self.preparer.results.resolve_outputs_in(
                            tx, ((source.result, source.output),)
                        )
                    )[0]
                    artifact = await self.artifacts.get_in(tx, ref)
                    binding = MaterialSource(
                        key=key,
                        material=source,
                        selections=(ContentSelection(artifact=ref),),
                        kind=role,
                        name=artifact.display_name,
                    )
                elif isinstance(source, ArtifactRef) or (
                    isinstance(source, str) and identifier_type(source) == "art"
                ):
                    artifact = await self.artifacts.get_in(tx, source)
                    binding = MaterialSource(
                        key=key,
                        material=artifact.ref,
                        selections=(ContentSelection(artifact=artifact.ref),),
                        kind=role,
                        name=artifact.display_name,
                    )
                else:
                    row, revision, _ = await self.reader.resolve_in(tx, cast(Target, source), ns.id)
                    artifact = await self.artifacts.get_in(tx, revision["body_artifact_id"])
                    owner = artifact.ref
                    binding = MaterialSource(
                        key=key,
                        material=self.reader.revision_ref(row, revision),
                        selections=(ContentSelection(artifact=artifact.ref),),
                        kind=role,
                        name=revision["title"],
                    )
                if isinstance(binding.material, RecordRevisionRef):
                    if owner is not None:
                        owners[binding.key] = owner
                    if reject_journals and binding.material.record_ref.kind == "journal":
                        raise NousError(
                            "cognition.writing_intent_required",
                            "Choose rewrite or reflection for an existing Journal",
                            details={
                                "journal": binding.material.identifier,
                                "operations": ["rewrite", "reflect"],
                            },
                        )
                bindings.append(binding)
            missing = tuple(
                dict.fromkeys(
                    binding.material for binding in bindings if binding.material not in views
                )
            )
            captured = await self.materials.capture_in(tx, missing, namespace=ns.ref)
            views.update(zip(missing, captured, strict=True))
            for binding in bindings:
                if isinstance(binding.material, RecordRevisionRef) and binding.key not in owners:
                    body = views[binding.material].body
                    if body is None:
                        raise NousError("material.no_content", "Material has no content to cite")
                    owners[binding.key] = body.artifact
        normalized, facts = await self.preparer.validate_selections(
            [selection for binding in bindings for selection in binding.selections],
            settings=self.reader.execution.snapshot.values.media,
        )
        selected: list[SelectedSource] = []
        offset = 0
        for binding in bindings:
            count = len(binding.selections)
            actual = normalized[offset : offset + count]
            origins = actual
            representations: list[InputPart] = []
            if binding.key in owners:
                mapped: list[ContentSelection] = []
                for selection in actual:
                    if selection.artifact == owners[binding.key]:
                        mapped.append(selection)
                        representations.append(InputPart(content=selection))
                    else:
                        source_origins = await self.artifacts.map_origins(
                            selection, owners[binding.key]
                        )
                        mapped.extend(origin.origin for origin in source_origins)
                        representations.append(InputPart(content=selection, origins=source_origins))
                origins = tuple(dict.fromkeys(mapped))
            selected.append(
                SelectedSource(
                    key=binding.key,
                    material=binding.material,
                    kind=TypeAdapter(SourceRole).validate_python(binding.kind),
                    name=binding.name,
                    selections=origins,
                    representations=tuple(representations) if tuple(actual) != origins else (),
                )
            )
            offset += count
        self._validate_roles(selected)
        self.materials.validate_captured_sources(
            selected, tuple(views[item.material] for item in selected)
        )
        return SelectedInputs(sources=tuple(selected), facts=facts)

    @staticmethod
    def _validate_roles(sources: Sequence[MaterialSource]) -> None:
        if len({source.key for source in sources}) != len(sources):
            raise NousError("input.duplicate_source", "Source keys must be unique")
        for source in sources:
            if source.kind == "current_basis" and not isinstance(
                source.material, RecordRevisionRef
            ):
                raise NousError(
                    "cognition.current_basis_owner_required",
                    "Current basis requires an exact record revision",
                )

    async def validate_in(
        self, tx: ReadTx, namespace: NamespaceRef, sources: Sequence[MaterialSource]
    ) -> None:
        self._validate_roles(sources)
        await self.materials.validate_sources_in(tx, sources, namespace=namespace)
