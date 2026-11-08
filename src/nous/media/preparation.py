"""Prepare actual CAS-backed text and media, preserving every selected batch."""

from __future__ import annotations

import codecs
import shutil
from collections.abc import AsyncGenerator, Sequence
from contextlib import asynccontextmanager
from dataclasses import replace
from pathlib import Path
from typing import cast

from PIL import Image, UnidentifiedImageError
from pydantic import JsonValue
from pydantic_ai.messages import BinaryContent

from nous.common.errors import NousError
from nous.content.artifacts import ArtifactStore
from nous.content.contracts import (
    Artifact,
    ArtifactRef,
    ContentSelection,
    InputGroup,
    OutputContent,
    OutputMapping,
    SourceSelection,
    is_text_media_type,
)
from nous.content.results import ResultStore
from nous.content.text import TextView
from nous.runtime.execution import ExecutionManager

from .audiovisual import AudioVisual
from .audiovisual_preparation import AudioVisualPreparation
from .configuration import MediaConfig
from .contracts import (
    MediaPlan,
    PlannedConversion,
    PlannedPart,
    PreparedBatch,
    PreparedSource,
    TextMapping,
    TextRepresentation,
)
from .derivatives import MediaDerivatives, preparation_results, track_completed
from .documents import (
    inspect_image,
    inspect_pdf,
)
from .planning import (
    audio_format,
    image_format,
    make_batch,
    make_group,
    media_interval,
    split_sources,
    validate_selection_bounds,
    with_source_context,
)
from .visual_preparation import VisualPreparation


class MediaPreparer:
    """Materialize precise CAS-backed representations and retain completed conversion stages."""

    def __init__(
        self,
        artifacts: ArtifactStore,
        results: ResultStore,
        execution: ExecutionManager,
    ) -> None:
        self.artifacts, self.results, self.execution = artifacts, results, execution
        self.derivatives = MediaDerivatives(artifacts, results)
        self.visual = VisualPreparation(artifacts, results, execution, self.derivatives)
        self.av = AudioVisualPreparation(artifacts, execution, self.derivatives)

    @staticmethod
    def capabilities(settings: MediaConfig) -> dict[str, JsonValue]:
        Image.init()
        formats = {"image/png": "PNG", "image/jpeg": "JPEG", "image/webp": "WEBP"}
        executables: dict[str, JsonValue] = {}
        for name, configured in (
            ("ffmpeg", settings.ffmpeg_path),
            ("ffprobe", settings.ffprobe_path),
        ):
            resolved = shutil.which(configured or name)
            executables[name] = {"path": resolved, "available": resolved is not None}
        return {
            "image_formats": [
                mime for mime in settings.image.formats if formats.get(mime) in Image.SAVE
            ],
            "executables": executables,
            "jpeg_background": settings.image.jpeg_background,
        }

    @asynccontextmanager
    async def batches(
        self, plan: MediaPlan, context: object = None
    ) -> AsyncGenerator[AsyncGenerator[PreparedBatch]]:
        """Own a sequential conversion iterator and close it when the consumer leaves."""

        async def iterate() -> AsyncGenerator[PreparedBatch]:
            for batch in plan.batches:
                yield await self.materialize(batch)

        iterator = iterate()
        try:
            yield iterator
        finally:
            await iterator.aclose()

    @staticmethod
    def text_view_parameters(info: dict[str, JsonValue]) -> dict[str, JsonValue]:
        """Describe the extraction semantics used to cache a PDF text view."""
        return {
            "source_media_type": "application/pdf",
            "tool": info["tool"],
            "version": info["version"],
            "origin_mapping": "selected_originals",
        }

    async def text_view(
        self, content: ContentSelection | ArtifactRef | str, *, settings: MediaConfig
    ) -> TextRepresentation:
        source = (
            content
            if isinstance(content, ContentSelection)
            else ContentSelection(artifact=(await self.artifacts.get(content)).ref)
        )
        original = await self.artifacts.get(source.artifact)
        info = await self.inspect(original, settings)
        validate_selection_bounds(info, source.selection)
        if info["kind"] == "text":
            view = await self.artifacts.text_view(source)
            return TextRepresentation(
                original.ref,
                view,
                (make_group("S1", original.display_name, original, source.selection),),
                {"mode": "text", "selection": source.selection.model_dump(mode="json")},
                mappings=(TextMapping(source, (source,)),),
            )
        if info["kind"] != "pdf":
            raise NousError(
                "media.text_representation_missing",
                "This material requires explicit transcription or language interpretation to obtain text",
            )
        parameters = self.text_view_parameters(info)
        key = self.derivatives.key(original, source.selection, "media.text-view", parameters)
        cached = await self.results.find_cached("media.text-view", key)
        if cached is None:
            plan = await self.prepare(
                original,
                source.selection,
                requirements=("text/plain",),
                settings=settings,
                document_mode="text",
            )
            texts: list[str] = []
            inputs: list[InputGroup] = []
            mappings: list[OutputMapping] = []
            offset = 0
            async with self.batches(plan) as batches:
                async for batch in batches:
                    for item in batch.sources:
                        text = item.text or ""
                        start = offset + (2 if texts else 0)
                        offset = start + len(text.encode())
                        origins = (
                            tuple(
                                dict.fromkeys(
                                    selected
                                    for part in item.group.parts
                                    for selected in (
                                        tuple(value.origin for value in part.origins)
                                        or (part.content,)
                                    )
                                )
                            )
                            if item.group
                            else ()
                        )
                        if start < offset and origins:
                            mappings.append(
                                OutputMapping(
                                    output_name="text",
                                    selection=SourceSelection(start_byte=start, end_byte=offset),
                                    relation="atomic",
                                    origins=origins,
                                )
                            )
                        texts.append(text)
                    inputs.extend(batch.inputs)
            cached = await self.results.adopt(
                operation="media.text-view",
                producer={"kind": "deterministic", **parameters},
                inputs=tuple(inputs),
                outputs={
                    "text": OutputContent(
                        data="\n\n".join(texts).encode(),
                        media_type="text/plain",
                        display_name="document.txt",
                    )
                },
                parameters={
                    **parameters,
                    "cache_key": key,
                    "representations": {"text": {"kind": "text"}},
                },
                coverage=plan.coverage,
                mappings=mappings,
            )
        track_completed(cached.ref.identifier)
        body = cached.outputs[0].artifact
        resolved_mappings = tuple(
            TextMapping(
                ContentSelection(
                    artifact=body,
                    selection=SourceSelection(
                        start_byte=mapping.selection.start_byte,
                        end_byte=mapping.selection.end_byte,
                    ),
                ),
                mapping.origins,
            )
            for mapping in cached.mappings
            if mapping.output_name == "text"
        )
        return TextRepresentation(
            body,
            await self.artifacts.text_view(body),
            cached.inputs,
            cached.coverage,
            (cached.ref.identifier,),
            resolved_mappings,
        )

    async def inspect(
        self, artifact: Artifact | ArtifactRef | str, settings: MediaConfig
    ) -> dict[str, JsonValue]:
        original = (
            artifact if isinstance(artifact, Artifact) else await self.artifacts.get(artifact)
        )
        blob = await self.artifacts.blob(original.ref)
        async with self.artifacts.borrow_local(original.ref) as path:
            return await self._inspect_local(original, blob.byte_length, path, settings)

    async def validate_selections(
        self,
        values: Sequence[ContentSelection],
        *,
        settings: MediaConfig,
        facts: dict[str, dict[str, JsonValue]] | None = None,
    ) -> tuple[ContentSelection, ...]:
        """Normalize text and inspect media coordinates without creating derivatives."""
        originals = await self.artifacts.get_many([value.artifact for value in values])
        inspected = dict(facts or {})
        for artifact in originals:
            if artifact.ref.id not in inspected:
                inspected[artifact.ref.id] = await self.inspect(artifact, settings)
        normalized = await self.artifacts.normalize_selections(
            values, media_types={key: str(value["media_type"]) for key, value in inspected.items()}
        )
        for value, artifact in zip(normalized, originals, strict=True):
            selection = value.selection
            validate_selection_bounds(inspected[artifact.ref.id], selection)
        return normalized

    async def _inspect_local(
        self, original: Artifact, byte_length: int, path: Path, settings: MediaConfig
    ) -> dict[str, JsonValue]:
        with path.open("rb") as source:
            header = source.read(4096)
        if header.startswith(b"%PDF-"):
            info = await self.execution.blocking(inspect_pdf, path)
        else:
            try:
                info = await self.execution.blocking(inspect_image, path)
            except UnidentifiedImageError, OSError:
                try:
                    decoded = codecs.getincrementaldecoder("utf-8")().decode(header, final=False)
                    controls = sum(ord(char) < 32 and char not in "\r\n\t" for char in decoded)
                    text = "\x00" not in decoded and controls <= len(decoded) // 100
                except UnicodeDecodeError:
                    text = False
                if not text:
                    info = await AudioVisual(settings, self.execution).inspect(path)
                else:
                    mime = (
                        original.media_type
                        if is_text_media_type(original.media_type)
                        else "text/plain"
                    )
                    info = {"kind": "text", "encoding": "utf-8", "media_type": mime}
        return {
            **info,
            "artifact": original.ref.model_dump(mode="json"),
            "byte_length": byte_length,
            "media_type": info.get("media_type", original.media_type),
        }

    async def materialize(self, batch: PreparedBatch) -> PreparedBatch:
        """Materialize one planned batch's text and media for consumption.

        Deferred deterministic conversions may publish and adopt derivatives;
        their completed result references are returned with the batch or attached
        to a ``NousError`` if conversion fails. Materialize bounded batches as
        needed instead of loading all planned media at once.

        Returns:
            The batch with source representations loaded and completed
            conversion result references retained.

        Raises:
            NousError: ``media.invalid_plan`` if a source lacks content
                references, or a conversion or content-reading error.
        """
        if batch.source_role != "source":
            role_sources = tuple(
                replace(source, group=source.group.model_copy(update={"role": batch.source_role}))
                if source.group
                else source
                for source in batch.sources
            )
            batch = replace(
                batch,
                sources=role_sources,
            )
        if batch.conversion is None:
            bound_sources = tuple(
                source.bind_slot(batch.source_slot_key, batch.source_prefix, batch.source_role)
                if source.group and source.group.slot_key != batch.source_slot_key
                else source
                for source in batch.sources
            )
            batch = replace(
                batch,
                sources=bound_sources,
            )
        if batch.conversion is not None:
            recipe = batch.conversion
            completed: list[str] = []
            token = preparation_results.set(completed)
            try:
                converted = await self._prepare(
                    recipe.artifact,
                    recipe.selection,
                    requirements=recipe.requirements,
                    settings=recipe.settings,
                    document_mode=recipe.document_mode,
                    without_audio=recipe.without_audio,
                    facts=recipe.parts[0].known_shape if recipe.parts else None,
                )
                materialized = [await self.materialize(item) for item in converted.batches]
                actual_sources = tuple(source for item in materialized for source in item.sources)
                if (
                    batch.source_prefix != "S1"
                    or batch.source_role != "source"
                    or batch.source_slot_key != "S1"
                ):
                    actual_sources = tuple(
                        source.bind_slot(
                            batch.source_slot_key, batch.source_prefix, batch.source_role
                        )
                        for source in actual_sources
                    )
                return replace(
                    batch,
                    sources=actual_sources,
                    conversion=None,
                    coverage=converted.coverage,
                    completed_result_refs=tuple(dict.fromkeys(completed)),
                )
            except NousError as error:
                error.details["completed_result_refs"] = list(dict.fromkeys(completed))
                raise
            finally:
                preparation_results.reset(token)
        sources: list[PreparedSource] = []
        for source in batch.sources:
            texts: list[str] = []
            media: list[BinaryContent] = []
            if source.group is None:
                raise NousError("media.invalid_plan", "Prepared media has no content references")
            for part in source.group.parts:
                artifact = await self.artifacts.get(part.content.artifact)
                mime = part.media_type or artifact.media_type
                if is_text_media_type(mime):
                    texts.append(await self.artifacts.read_selection(part.content))
                else:
                    media.append(
                        BinaryContent(
                            data=await self.artifacts.read_bytes(artifact.ref),
                            media_type=mime,
                        )
                    )
            sources.append(
                replace(source, text="\n".join(texts) if texts else None, media=tuple(media))
            )
        return replace(batch, sources=tuple(sources))

    async def prepare(
        self,
        artifact: Artifact | ArtifactRef | str,
        selection: SourceSelection | None = None,
        *,
        requirements: Sequence[str] = (),
        settings: MediaConfig,
        document_mode: str = "auto",
        without_audio: bool = False,
        facts: dict[str, JsonValue] | None = None,
    ) -> MediaPlan:
        """Plan bounded batches of deterministic representations for an artifact.

        Args:
            artifact: Accepted artifact or its instance-bound reference.
            selection: Requested source region; None selects the whole source.
            requirements: Accepted MIME types used to choose representations.
            settings: Media limits and conversion settings for this operation.
            document_mode: Document representation policy.
            without_audio: Exclude audio when preparing video.

        Returns:
            A reference-based plan. Derivative conversion and byte loading occur
            when a batch is materialized, rather than during planning.

        Raises:
            NousError: The source, selection or requested representation is invalid.
        """
        original = (
            artifact if isinstance(artifact, Artifact) else await self.artifacts.get(artifact)
        )
        selected = selection or SourceSelection()
        info = facts if facts is not None else await self.inspect(original, settings)
        kind = str(info["kind"])
        validate_selection_bounds(info, selected)
        if kind == "text":
            return with_source_context(await self._text(original, selected, settings), info)
        mime = str(info["media_type"])
        # Actual identified MIME follows identity references through materialization.
        original = original.model_copy(update={"media_type": mime})
        if kind == "video" and any(value.startswith("video/") for value in requirements):
            return with_source_context(
                self.av.plan_native_video(
                    original, selected, settings, info, requirements, without_audio
                ),
                info,
            )
        selections: list[SourceSelection] = []
        if kind == "pdf":
            pages = selected.pages or tuple(range(1, int(cast(int, info["pages"])) + 1))
            native = (
                document_mode == "auto"
                and "application/pdf" in requirements
                and not selected.pages
                and int(cast(int, info["byte_length"])) <= settings.max_inline_bytes
            )
            selections = (
                [selected]
                if native
                else [
                    SourceSelection(
                        pages=pages[offset : offset + settings.pdf.max_pages_per_request]
                    )
                    for offset in range(0, len(pages), settings.pdf.max_pages_per_request)
                ]
            )
            target = (
                "application/pdf"
                if document_mode == "auto" and "application/pdf" in requirements
                else image_format(requirements, settings)
                if document_mode != "text"
                else "text/plain"
            )
        elif kind in {"audio", "video"}:
            start, end = media_interval(info, selected)
            span = max(1, round(settings.audio.max_seconds_per_request * 1000))
            if kind == "video":
                span = min(
                    span, settings.video.sample_interval_ms * settings.video.max_frames_per_request
                )
            selections = [
                SourceSelection(start_ms=lower, end_ms=min(lower + span, end))
                for lower in range(start, end, span)
            ]
            target = image_format(requirements, settings) if kind == "video" else "audio/wav"
        else:
            selections = [selected]
            if int(cast(int, info["frames"])) > 1 and selected.frame is None:
                next_sample = selected.start_ms or 0
                selections = []
                for frame in cast(list[dict[str, int]], info["frame_times"]):
                    position, span = frame["start_ms"], max(1, frame["duration_ms"])
                    if position + span > next_sample and (
                        selected.end_ms is None or position < selected.end_ms
                    ):
                        selections.append(selected.model_copy(update={"frame": frame["frame"]}))
                        next_sample = max(
                            next_sample + settings.video.sample_interval_ms, position + span
                        )
                if not selections:
                    raise NousError(
                        "media.empty_selection", "No animation frames fall in the selected interval"
                    )
            target = (
                mime
                if mime in (requirements or ("image/png", "image/jpeg", "image/webp"))
                else image_format(requirements, settings)
            )
            if (
                target == "image/jpeg"
                and bool(info["has_alpha"])
                and settings.image.jpeg_background is None
            ):
                raise NousError(
                    "media.jpeg_background_required",
                    "Transparent images require an explicit JPEG background",
                )
        batches: list[PreparedBatch] = []
        for part_selection in selections:
            identity = (
                kind == "image"
                and mime == target
                and info["frames"] == 1
                and info["orientation"] == 1
                and selected.crop is None
                and max(int(cast(int, info["width"])), int(cast(int, info["height"])))
                <= settings.image.max_side
                or kind == "pdf"
                and target == "application/pdf"
                and not part_selection.pages
            )
            part = PlannedPart(
                ContentSelection(artifact=original.ref, selection=part_selection),
                mime,
                "original" if identity else kind,
                target,
                {
                    "operation": "identity" if identity else "convert",
                    "target_media_type": target,
                    "max_side": settings.image.max_side,
                    "quality": settings.image.quality,
                    "jpeg_background": settings.image.jpeg_background,
                    "render_dpi": settings.pdf.render_dpi,
                },
                info,
            )
            parts = (part,)
            if kind == "video" and bool(info.get("has_audio")) and not without_audio:
                parts += (
                    PlannedPart(
                        part.source,
                        mime,
                        "audio",
                        audio_format(requirements, settings),
                        {"operation": "audio", "sample_rate": 16000, "channels": 1},
                    ),
                )
            recipe = PlannedConversion(
                original,
                part_selection,
                tuple(requirements),
                settings,
                document_mode,
                without_audio,
                parts,
            )
            coverage: dict[str, JsonValue] = {
                "kind": kind,
                "selection": part_selection.model_dump(mode="json"),
            }
            batches.append(PreparedBatch((), part_selection, coverage, conversion=recipe))
        return with_source_context(
            MediaPlan(
                original,
                selected,
                tuple(batches),
                {
                    "kind": kind,
                    "selection": selected.model_dump(mode="json"),
                    "batches": len(batches),
                },
            ),
            info,
        )

    async def _prepare(
        self,
        artifact: Artifact | ArtifactRef | str,
        selection: SourceSelection | None = None,
        *,
        requirements: Sequence[str] = (),
        settings: MediaConfig,
        document_mode: str = "auto",
        without_audio: bool = False,
        facts: dict[str, JsonValue] | None = None,
    ) -> MediaPlan:
        original = (
            artifact if isinstance(artifact, Artifact) else await self.artifacts.get(artifact)
        )
        selected = selection or SourceSelection()
        info = facts if facts is not None else await self.inspect(original, settings)
        original = original.model_copy(update={"media_type": str(info["media_type"])})
        async with self.artifacts.borrow_local(original.ref) as path:
            kind = cast(str, info["kind"])
            validate_selection_bounds(info, selected)
            if kind == "text":
                plan = await self._text(original, selected, settings)
            elif kind == "image":
                plan = await self.visual.images(
                    original, selected, settings, path, info, requirements
                )
            elif kind == "pdf":
                plan = await self.visual.pdf(
                    original, selected, settings, path, info, requirements, document_mode
                )
            else:
                plan = await self.av.prepare(
                    original,
                    selected,
                    settings,
                    path,
                    info,
                    requirements,
                    without_audio,
                )
            return with_source_context(plan, info)

    async def _text(
        self, original: Artifact, selection: SourceSelection, settings: MediaConfig
    ) -> MediaPlan:
        view = await self.artifacts.text_view(
            ContentSelection(artifact=original.ref, selection=selection)
        )
        return await self.execution.blocking(self._text_plan, original, selection, settings, view)

    def _text_plan(
        self, original: Artifact, selection: SourceSelection, settings: MediaConfig, view: TextView
    ) -> MediaPlan:
        chunks = view.chunks(max_bytes=settings.max_inline_bytes)
        sources = tuple(
            PreparedSource(
                key=f"S1:C{index}",
                name=original.display_name,
                text=chunk.text,
                group=make_group(
                    f"S1:C{index}",
                    original.display_name,
                    original,
                    chunk.selection.selection,
                    representation="text",
                ),
                metadata={
                    "representation": "text",
                    "parser_profile": view.parser_profile,
                    "blocks": list[JsonValue](
                        block.model_dump(mode="json") for block in chunk.blocks
                    ),
                    "byte_length": len(chunk.text.encode("utf-8")),
                },
            )
            for index, chunk in enumerate(chunks, 1)
        )
        if not sources:
            raise NousError("media.empty_selection", "The selected text has no readable body")
        groups = split_sources(sources, len(sources), settings.max_inline_bytes)
        batches: list[PreparedBatch] = []
        for group in groups:
            inputs = tuple(source.group for source in group if source.group is not None)
            actual = SourceSelection(
                start_byte=inputs[0].parts[0].content.selection.start_byte,
                end_byte=inputs[-1].parts[0].content.selection.end_byte,
            )
            batches.append(
                make_batch(
                    original,
                    group,
                    actual,
                    {
                        "mode": "text",
                        "selection": actual.model_dump(mode="json"),
                        "parser_profile": view.parser_profile,
                    },
                )
            )
        return MediaPlan(
            original,
            selection,
            tuple(batches),
            {
                "mode": "text",
                "selection": selection.model_dump(mode="json"),
                "parser_profile": view.parser_profile,
                "batches": len(batches),
            },
        )
