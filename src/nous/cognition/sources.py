"""Freeze content coordinates together with their formal cognitive ownership."""

from collections.abc import Sequence
from dataclasses import dataclass
from typing import TYPE_CHECKING, Literal, cast

from nous.common.errors import NousError
from nous.common.identifier import identifier_type
from nous.content.contracts import (
    ArtifactInput,
    ArtifactRef,
    ContentSelection,
    ExternalInput,
    InlineText,
    InputGroup,
    ResultOutputInput,
    ResultOutputRef,
    SelectedInputs,
    SelectedSource,
    SourceTarget,
)
from nous.media.preparation import MediaPreparer
from nous.storage.database import ReadTx

from .contracts import Namespace, NamespaceRef, RevisionSelection, SourceBinding
from .writing.contracts import SourceChoice
from .writing.reading import RecordReader, Target

SourceInput = (
    SourceChoice
    | SourceBinding
    | RevisionSelection
    | Target
    | ContentSelection
    | ArtifactRef
    | ResultOutputRef
    | SourceTarget
)

if TYPE_CHECKING:
    from nous.application.inputs import InputReader


@dataclass(frozen=True)
class PreparedSources:
    """Formal dependencies and the precise representations selected for consumption."""

    namespace: Namespace
    bindings: tuple[SourceBinding, ...]
    selected_inputs: SelectedInputs


class SourceResolver:
    """Resolve a source once, before models or a formal write consume it."""

    def __init__(self, reader: RecordReader, preparer: MediaPreparer):
        self.reader, self.artifacts, self.preparer = reader, reader.artifacts, preparer

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
    ) -> PreparedSources:
        """Freeze formal source ownership and the representations selected for use.

        Range normalization and mapping run outside database transactions; the final
        read view checks the immutable revision bindings used during adoption.
        """
        resolved: list[object] = []
        for value in inputs:
            source = value.target if isinstance(value, SourceChoice) else value
            if isinstance(source, (ExternalInput, InlineText, ArtifactInput, ResultOutputInput)):
                if input_reader is None:
                    raise NousError(
                        "input.missing_binding", "Source requires the current input reader"
                    )
                artifact, selection = await input_reader.accept_target(source)
                source = ContentSelection(artifact=artifact.ref, selection=selection)
            resolved.append(source)
        async with self.reader.database.read() as tx:
            ns = await self.reader.namespace_in(tx, namespace)
            result: list[SourceBinding] = []
            owners: dict[str, ArtifactRef] = {}
            index = 0
            for value, source in zip(inputs, resolved, strict=True):
                while True:
                    index += 1
                    key = f"{prefix}{index}"
                    if not any(old == key or old.startswith(key + ":") for old in reserved_keys):
                        break
                selected_usage = value.usage if isinstance(value, SourceChoice) else usage
                if isinstance(source, SourceBinding):
                    binding = source if preserve_keys else source.model_copy(update={"key": key})
                elif isinstance(source, RevisionSelection):
                    _, revision, _ = await self.reader.resolve_in(tx, source.revision, ns.id)
                    binding = SourceBinding(
                        key=key,
                        selections=source.selections,
                        owner_revision=source.revision,
                        usage=selected_usage,
                        name=revision["title"],
                    )
                elif isinstance(source, ContentSelection):
                    artifact = await self.artifacts.get_in(tx, source.artifact)
                    binding = SourceBinding(
                        key=key,
                        selections=(source,),
                        usage=selected_usage,
                        name=artifact.display_name,
                    )
                elif isinstance(source, ResultOutputRef):
                    ref = await self.preparer.results.resolve_output_in(tx, source)
                    artifact = await self.artifacts.get_in(tx, ref)
                    binding = SourceBinding(
                        key=key,
                        selections=(ContentSelection(artifact=ref),),
                        name=artifact.display_name,
                        usage=selected_usage,
                    )
                elif isinstance(source, ArtifactRef) or (
                    isinstance(source, str) and identifier_type(source) == "art"
                ):
                    artifact = await self.artifacts.get_in(tx, source)
                    binding = SourceBinding(
                        key=key,
                        selections=(ContentSelection(artifact=artifact.ref),),
                        name=artifact.display_name,
                        usage=selected_usage,
                    )
                else:
                    row, revision, _ = await self.reader.resolve_in(tx, cast(Target, source), ns.id)
                    artifact = await self.artifacts.get_in(tx, revision["body_artifact_id"])
                    binding = SourceBinding(
                        key=key,
                        selections=(ContentSelection(artifact=artifact.ref),),
                        owner_revision=self.reader.revision_ref(row, revision),
                        name=revision["title"],
                        usage=selected_usage,
                    )
                if isinstance(value, SourceChoice):
                    binding = binding.model_copy(update={"usage": selected_usage})
                await self.artifacts.get_many_in(tx, [item.artifact for item in binding.selections])
                if binding.owner_revision:
                    _, revision, _ = await self.reader.resolve_in(tx, binding.owner_revision, ns.id)
                    owners[binding.key] = (
                        await self.artifacts.get_in(tx, revision["body_artifact_id"])
                    ).ref
                result.append(binding)
            if reject_journals:
                journal = next(
                    (
                        b.owner_revision
                        for b in result
                        if b.owner_revision and b.owner_revision.record_ref.kind == "journal"
                    ),
                    None,
                )
                if journal:
                    raise NousError(
                        "cognition.writing_intent_required",
                        "Choose rewrite or reflection for an existing Journal",
                        details={
                            "journal": journal.identifier,
                            "operations": ["rewrite", "reflect"],
                        },
                    )
        normalized = await self.preparer.validate_selections(
            [selection for binding in result for selection in binding.selections],
            settings=self.reader.execution.snapshot.values.media,
        )
        output: list[SourceBinding] = []
        actual: dict[str, tuple[ContentSelection, ...]] = {}
        offset = 0
        for binding in result:
            count = len(binding.selections)
            selected = normalized[offset : offset + count]
            actual[binding.key] = selected
            if binding.key in owners:
                mapped: list[ContentSelection] = []
                for selection in selected:
                    if selection.artifact == owners[binding.key]:
                        mapped.append(selection)
                    else:
                        mapped.extend(
                            origin.origin
                            for origin in await self.artifacts.map_origins(
                                selection, owners[binding.key]
                            )
                        )
                selected = tuple(dict.fromkeys(mapped))
            output.append(binding.model_copy(update={"selections": selected}))
            offset += count
        async with self.reader.database.read() as tx:
            await self.validate_in(tx, ns.ref, output)
        return PreparedSources(
            ns,
            tuple(output),
            SelectedInputs(
                sources=tuple(
                    SelectedSource(
                        slot_key=binding.key,
                        role=binding.usage,
                        selections=actual[binding.key],
                        display_name=binding.name,
                    )
                    for binding in output
                )
            ),
        )

    async def actual_bindings(
        self, groups: Sequence[InputGroup], prepared: PreparedSources
    ) -> tuple[SourceBinding, ...]:
        """Bind consumed representations to the exact formal sources frozen for their slots."""
        sources = {source.key: source for source in prepared.bindings}
        output: list[SourceBinding] = []
        for group in groups:
            if group.role not in {"source", "reference", "current_basis", "inspiration"}:
                continue
            source = sources.get(group.slot_key)
            if source is None:
                raise NousError(
                    "cognition.source_mismatch",
                    "Successful input has no frozen source",
                    details={"key": group.key},
                )
            coordinates: list[ContentSelection] = []
            for part in group.parts:
                for target in source.selections:
                    # Preparation already records the selected original range for
                    # transformed media. Re-resolving from the whole converted
                    # artifact loses that range because image/video bodies do not
                    # expose text byte coordinates.
                    origins = tuple(
                        origin
                        for origin in part.origins
                        if origin.origin.artifact == target.artifact
                    )
                    if not origins:
                        try:
                            origins = await self.artifacts.map_origins(
                                part.content, target.artifact
                            )
                        except NousError as error:
                            if error.code == "content.origin_unmapped":
                                continue
                            raise
                    for origin in origins:
                        try:
                            selection = target.selection.restrict(origin.origin.selection)
                        except NousError:
                            continue
                        coordinate = origin.origin.model_copy(update={"selection": selection})
                        if coordinate not in coordinates:
                            coordinates.append(coordinate)
            if not coordinates:
                raise NousError(
                    "cognition.source_mismatch",
                    "Actual source has no exact mapping to its selected original",
                    details={"key": group.key},
                )
            output.append(
                source.model_copy(update={"key": group.key, "selections": tuple(coordinates)})
            )
        return tuple(output)

    async def validate_in(
        self, tx: ReadTx, namespace: NamespaceRef, sources: Sequence[SourceBinding]
    ) -> None:
        """Recheck immutable body ownership in the caller's transaction."""
        for source in sources:
            artifacts = await self.artifacts.get_many_in(
                tx, [item.artifact for item in source.selections]
            )
            if source.owner_revision is None:
                if source.usage == "current_basis":
                    raise NousError(
                        "cognition.current_basis_owner_required",
                        "Current basis requires an exact record revision",
                    )
                continue
            _, revision, _ = await self.reader.resolve_in(tx, source.owner_revision, namespace.id)
            if any(revision["body_artifact_id"] != artifact.ref.id for artifact in artifacts):
                raise NousError(
                    "cognition.source_mismatch",
                    "Source content does not match its exact owner revision",
                )
