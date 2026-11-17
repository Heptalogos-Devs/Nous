"""Accept collection manifests and freeze document identities."""

from __future__ import annotations

from nous.application.contracts import CallContext
from nous.common.errors import NousError
from nous.content.artifacts import ArtifactStore
from nous.content.contracts import ArtifactRef
from nous.content.results import ResultStore
from nous.runtime.execution import ExecutionManager

from .contracts import (
    CollectionManifest,
    CorpusView,
    Document,
    DocumentInput,
    WaveBuild,
)
from .projection import content_evidence


class ManifestImporter:
    """Resolve manifest files and collect their source evidence."""

    def __init__(
        self, artifacts: ArtifactStore, results: ResultStore, execution: ExecutionManager
    ) -> None:
        self.artifacts, self.results, self.execution = artifacts, results, execution

    async def import_manifest(self, request: WaveBuild, call: CallContext) -> CorpusView:
        """Accept a manifest and its explicitly bound documents for this request."""
        manifest_artifact = await call.inputs.accept(request.manifest.key)
        manifest = CollectionManifest.model_validate_json(
            await self.artifacts.read_text(manifest_artifact.ref)
        )
        documents: list[Document] = []
        accepted: dict[str, ArtifactRef] = {}
        for document in manifest.documents:
            source = document.input
            if source.file is not None:
                external = request.documents.get(document.key)
                if external is None:
                    raise NousError(
                        "input.missing_binding",
                        "Manifest document has no input binding",
                        details={"document": document.key},
                    )
                artifact = await call.inputs.accept(external.key)
            elif source.text is not None:
                artifact = await self.artifacts.ingest(source.text, display_name=document.key)
            else:
                assert source.artifact is not None
                artifact = await self.artifacts.get(source.artifact)
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
