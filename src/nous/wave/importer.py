"""Accept collection manifests and freeze document identities."""

from __future__ import annotations

from pathlib import Path

from nous.content.artifacts import ArtifactStore
from nous.content.contracts import ArtifactRef, InputSpec
from nous.content.input_access import current_input_access
from nous.content.results import ResultStore
from nous.runtime.execution import ExecutionManager

from .contracts import (
    CollectionManifest,
    CorpusView,
    Document,
    DocumentInput,
)
from .projection import content_evidence


class ManifestImporter:
    """Resolve manifest files and collect their source evidence."""

    def __init__(
        self, artifacts: ArtifactStore, results: ResultStore, execution: ExecutionManager
    ) -> None:
        self.artifacts, self.results, self.execution = artifacts, results, execution

    async def import_manifest(self, manifest_path: Path) -> CorpusView:
        """Accept a file-based collection and freeze its document identities."""
        access = current_input_access.get()
        path = (
            await access.resolve(manifest_path)
            if access is not None
            else await self.execution.blocking(manifest_path.resolve)
        )
        manifest_artifact = await self.artifacts.ingest(path)
        manifest = CollectionManifest.model_validate_json(
            await self.artifacts.read_text(manifest_artifact.ref)
        )
        documents: list[Document] = []
        accepted: dict[str, ArtifactRef] = {}
        for document in manifest.documents:
            source = document.input
            if source.file is not None:
                file = Path(source.file)
                file = file if file.is_absolute() else path.parent / file
                resolved = (
                    await access.resolve(file)
                    if access is not None
                    else await self.execution.blocking(file.resolve)
                )
                spec = InputSpec(kind="file", path=str(resolved))
            elif source.text is not None:
                spec = InputSpec(kind="text", text=source.text, display_name=document.key)
            else:
                assert source.artifact is not None
                spec = InputSpec(kind="artifact", artifact=source.artifact)
            artifact = await self.artifacts.accept(spec)
            accepted[document.key] = artifact.ref
            documents.append(
                document.model_copy(
                    update={
                        "input": DocumentInput(
                            artifact=artifact.ref.identifier, selector=source.selector
                        )
                    }
                )
            )
        return CorpusView(
            **manifest.model_dump(exclude={"documents"}),
            documents=tuple(documents),
            manifest_artifact=manifest_artifact.ref,
            evidence=await content_evidence(documents, accepted, self.results),
        )
