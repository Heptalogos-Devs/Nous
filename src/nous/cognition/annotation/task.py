"""Cognitive interpretation of concept annotations for exact bodies."""

import hashlib
from collections.abc import Sequence
from typing import TYPE_CHECKING

from pydantic import JsonValue

from nous.common.errors import NousError
from nous.content.annotations import AnnotationProposal, AnnotationSet
from nous.content.contracts import Artifact, InputGroup, InputPart, OutputContent, SourceSelection
from nous.media.contracts import MediaPlan, PreparedBatch, PreparedSource
from nous.models.contracts import ReadingInput
from nous.models.tasks import InterpretedOutputs, TaskDefinition, interpreted_outputs

if TYPE_CHECKING:
    from nous.content.artifacts import ArtifactStore
    from nous.content.text import TextView


class ConceptAnnotationTask(TaskDefinition):
    """Prepare and interpret an annotation against one complete immutable Markdown body."""

    def __init__(self) -> None:
        super().__init__(
            "concept-annotation",
            AnnotationProposal,
            preserve_source_keys=True,
            text_input_mode="structured",
        )

    async def prepare_sources(
        self, originals: Sequence[tuple[Artifact, SourceSelection]], artifacts: ArtifactStore
    ) -> tuple[MediaPlan, ...]:
        if len(originals) != 1:
            raise NousError(
                "model.annotation_body", "Concept annotation requires one complete immutable body"
            )
        artifact, chosen = originals[0]
        blob = await artifacts.blob(artifact.ref)
        if chosen not in (
            SourceSelection(),
            SourceSelection(start_byte=0, end_byte=blob.byte_length),
        ):
            raise NousError(
                "model.annotation_body", "Concept annotation requires one complete immutable body"
            )
        view = await artifacts.text_view(artifact.ref)
        return await artifacts.content.execution.blocking(
            self._prepare_sources, artifact, chosen, view
        )

    @staticmethod
    def _prepare_sources(
        artifact: Artifact, chosen: SourceSelection, view: TextView
    ) -> tuple[MediaPlan, ...]:
        """Materialize structured blocks on the managed worker, not the event loop."""
        sources = tuple(
            PreparedSource(
                block.key,
                artifact.display_name,
                text=block.text,
                group=InputGroup(
                    key=block.key,
                    slot_key="S1",
                    name=artifact.display_name,
                    parts=(
                        InputPart(
                            content=block.selection,
                            representation="text",
                            media_type=artifact.media_type,
                            source_media_type=artifact.media_type,
                        ),
                    ),
                ),
            )
            for block in view.blocks
            if block.parent_key is None
        )
        batch = PreparedBatch(
            sources,
            chosen,
            {
                "body_ref": artifact.ref.model_dump(mode="json"),
                "parser_profile": view.parser_profile,
            },
        )
        return (MediaPlan(artifact, chosen, (batch,), batch.coverage),)

    def validate(self, value: JsonValue, reading: ReadingInput) -> JsonValue:
        proposal = AnnotationProposal.model_validate(value)
        proposal.validate_references({source.key for source in reading.sources})
        return proposal.model_dump(mode="json")

    async def outputs(
        self,
        value: JsonValue,
        reading: ReadingInput,
        artifacts: ArtifactStore,
        *,
        structured: bool,
        producer: dict[str, JsonValue] | None = None,
    ) -> InterpretedOutputs:
        interpreted = await super().outputs(
            value, reading, artifacts, structured=structured, producer=producer
        )
        bodies = {
            part.content.artifact.id: part.content.artifact
            for source in reading.sources
            if source.group
            for part in source.group.parts
        }
        if len(bodies) != 1:
            raise NousError("model.annotation_body", "Concept annotation requires one exact body")
        body = next(iter(bodies.values()))
        view = await artifacts.text_view(body)
        proposal = AnnotationProposal.model_validate(value)
        proposal.validate_references(
            {block.key for block in view.structure if block.parent_key is None}
        )
        annotation = AnnotationSet(
            **proposal.model_dump(),
            body_ref=body,
            parser_profile=view.parser_profile,
            annotation_revision=hashlib.sha256(interpreted.outputs["output"].data).hexdigest(),
            producer=producer or {},
        )
        return interpreted_outputs(
            {
                "output": OutputContent(
                    data=annotation.model_dump_json(indent=2).encode(),
                    media_type="application/json",
                    display_name="annotation.json",
                )
            },
            reading,
        )
