"""Adapt built-in domain facts to the common material capture contract."""

from __future__ import annotations

from typing import TYPE_CHECKING

from nous.cognition.contracts import NamespaceRef
from nous.cognition.material_contracts import (
    CognitiveMaterialView,
    MaterialRelation,
)
from nous.common.material_refs import RecordRevisionRef
from nous.content.contracts import (
    ArtifactRef,
    ContentSelection,
    ResultOutputRef,
)
from nous.storage.database import ReadTx
from nous.temporal.values import CognitiveTimes

if TYPE_CHECKING:
    from nous.cognition.writing.reading import RecordReader
    from nous.content.artifacts import ArtifactStore
    from nous.content.results import ResultStore


class MaterialAdapters:
    """Build material snapshots from facts captured by their domain owners."""

    def __init__(
        self,
        artifacts: ArtifactStore,
        results: ResultStore,
        records: RecordReader,
    ) -> None:
        self.artifacts, self.results, self.records = (
            artifacts,
            results,
            records,
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
    ) -> tuple[CognitiveMaterialView, ...]:
        refs = await self.records.current_refs_in(tx, namespace)
        return await self.capture_records(tx, refs)

    async def capture_records(
        self, tx: ReadTx, refs: tuple[RecordRevisionRef, ...]
    ) -> tuple[CognitiveMaterialView, ...]:
        captured = await self.records.capture_many_in(tx, refs)
        output: list[CognitiveMaterialView] = []
        for ref, item in zip(refs, captured, strict=True):
            revision = item.revision
            formal_sources = item.sources
            view = CognitiveMaterialView(
                ref=ref,
                title=revision["title"],
                media_type="text/markdown",
                body=ContentSelection(artifact=item.body),
                material_sources=tuple(formal_sources),
                times=CognitiveTimes.model_validate(revision["cognitive_times"]),
            )
            output.append(view)
        return tuple(output)

    async def capture_artifacts(
        self, tx: ReadTx, refs: tuple[ArtifactRef, ...]
    ) -> tuple[CognitiveMaterialView, ...]:
        artifacts = await self.artifacts.get_many_in(tx, refs)
        produced = await self.results.outputs_for_artifacts_in(tx, refs)
        output: list[CognitiveMaterialView] = []
        for artifact in artifacts:
            relations = ()
            if artifact.ref.id in produced:
                relations = (
                    MaterialRelation(
                        kind="produced_by",
                        target=produced[artifact.ref.id],
                    ),
                )
            output.append(
                CognitiveMaterialView(
                    ref=artifact.ref,
                    title=artifact.display_name,
                    media_type=artifact.media_type,
                    body=ContentSelection(artifact=artifact.ref),
                    relations=relations,
                )
            )
        return tuple(output)

    async def capture_outputs(
        self,
        tx: ReadTx,
        refs: tuple[ResultOutputRef, ...],
    ) -> tuple[CognitiveMaterialView, ...]:
        resolved = await self.results.resolve_outputs_in(
            tx, tuple((ref.result, ref.output) for ref in refs)
        )
        sources = await self.results.output_sources_in(tx, refs)
        artifacts = await self.artifacts.get_many_in(tx, [item[1] for item in resolved])
        output: list[CognitiveMaterialView] = []
        for ref, artifact in zip(refs, artifacts, strict=True):
            body = ContentSelection(artifact=artifact.ref)
            formal_sources = sources.get((ref.result.id, ref.output), ())
            output.append(
                CognitiveMaterialView(
                    ref=ref,
                    title=artifact.display_name,
                    media_type=artifact.media_type,
                    body=body,
                    material_sources=formal_sources,
                )
            )
        return tuple(output)
