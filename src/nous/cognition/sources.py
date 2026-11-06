"""Freeze content coordinates together with their formal cognitive ownership."""

from collections.abc import Sequence
from dataclasses import dataclass
from typing import Literal

import sqlalchemy as sa

from nous.common.errors import NousError
from nous.common.identifier import identifier_type
from nous.content.contracts import ArtifactRef, ContentSelection
from nous.media.preparation import MediaPreparer
from nous.storage.database import ReadTx

from . import schema
from .contracts import Namespace, NamespaceRef, RecordRevisionRef, RevisionSelection, SourceBinding
from .writing.contracts import SourceRequest
from .writing.reading import RecordReader, Target

SourceInput = (
    SourceRequest | SourceBinding | RevisionSelection | Target | ContentSelection | ArtifactRef
)


@dataclass(frozen=True)
class PreparedSources:
    """Formal dependencies and the precise representations selected for consumption."""

    namespace: Namespace
    bindings: tuple[SourceBinding, ...]
    actual: dict[str, tuple[ContentSelection, ...]]


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
    ) -> PreparedSources:
        """Freeze formal source ownership and the representations selected for use.

        Range normalization and mapping run outside database transactions; the final
        read view checks the immutable revision bindings used during adoption.
        """
        async with self.reader.database.read() as tx:
            ns = await self.reader.namespace_in(tx, namespace)
            result: list[SourceBinding] = []
            owners: dict[str, ArtifactRef] = {}
            index = 0
            for value in inputs:
                while True:
                    index += 1
                    key = f"{prefix}{index}"
                    if not any(old == key or old.startswith(key + ":") for old in reserved_keys):
                        break
                selected_usage = value.usage if isinstance(value, SourceRequest) else usage
                source = value.target if isinstance(value, SourceRequest) else value
                if isinstance(source, SourceBinding):
                    binding = source if preserve_keys else source.model_copy(update={"key": key})
                elif isinstance(source, RevisionSelection):
                    binding = SourceBinding(
                        key=key,
                        selections=source.selections,
                        owner_revision=source.revision,
                        usage=selected_usage,
                    )
                elif isinstance(source, ContentSelection):
                    binding = SourceBinding(key=key, selections=(source,), usage=selected_usage)
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
                    row, revision, _ = await self.reader.resolve_in(tx, source, ns.id)
                    artifact = await self.artifacts.get_in(tx, revision["body_artifact_id"])
                    binding = SourceBinding(
                        key=key,
                        selections=(ContentSelection(artifact=artifact.ref),),
                        owner_revision=self.reader.revision_ref(row, revision),
                        name=revision["title"],
                        usage=selected_usage,
                    )
                if isinstance(value, SourceRequest):
                    binding = binding.model_copy(update={"usage": selected_usage})
                if binding.owner_revision is None:
                    matches: dict[str, RecordRevisionRef] = {}
                    for selection in binding.selections:
                        rows = (
                            await tx.connection.execute(
                                sa.select(schema.records.c.identifier, schema.revisions.c.ordinal)
                                .join(
                                    schema.revisions,
                                    schema.revisions.c.record_id == schema.records.c.id,
                                )
                                .where(schema.revisions.c.body_artifact_id == selection.artifact.id)
                            )
                        ).all()
                        for identifier, ordinal in rows:
                            try:
                                row, revision, _ = await self.reader.resolve_in(
                                    tx, f"{identifier}@{ordinal}", ns.id
                                )
                            except NousError as error:
                                if error.code == "cognition.namespace_mismatch":
                                    raise NousError(
                                        "cognition.namespace_mismatch",
                                        "Source belongs to another namespace",
                                    ) from error
                                raise
                            matches[revision["id"]] = self.reader.revision_ref(row, revision)
                    if len(matches) > 1:
                        raise NousError(
                            "cognition.source_ambiguous", "Choose an exact owning revision"
                        )
                    if matches:
                        binding = binding.model_copy(
                            update={"owner_revision": next(iter(matches.values()))}
                        )
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
        return PreparedSources(ns, tuple(output), actual)

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
