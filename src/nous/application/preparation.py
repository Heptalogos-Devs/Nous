"""Prepare immutable text once for all candidates in one corpus build."""

from __future__ import annotations

import hashlib
from collections.abc import Sequence
from dataclasses import dataclass

from pydantic import JsonValue

from nous.common.errors import NousError
from nous.content.artifacts import ArtifactStore
from nous.content.citations import project_citation_slice
from nous.content.contracts import (
    Artifact,
    ArtifactRef,
    BlobRef,
    ContentSelection,
    OutputContent,
    ProgramProducer,
    ResultFacts,
    SelectedInputs,
    SourceSelection,
    is_text_media_type,
)
from nous.content.projections import TextProjection, semantic_text_many
from nous.content.text import PARSER_PROFILE, TextView
from nous.media.contracts import TextMapping
from nous.media.preparation import MediaPreparer


@dataclass(frozen=True)
class PreparedText:
    """Prepared text with exact original selections retained through slicing.

    ``view`` identifies the represented body; ``origin_contents`` identifies the
    original source regions, which can differ for converted media. Citation
    projection affects ``text`` while the view retains original source bytes.
    """

    view: TextView
    origin_contents: tuple[ContentSelection, ...]
    session: TextPreparation
    mappings: tuple[TextMapping, ...] = ()

    @property
    def text(self) -> str:
        raw = self.view.text.encode("utf-8")
        return project_citation_slice(raw, self.view.citation_spans, 0, len(raw))

    def slice(self, selection: SourceSelection) -> PreparedText:
        """Select part of this view and retain overlapping original source regions.

        Returns:
            A new prepared value sharing this build session and source mappings.

        Raises:
            NousError: An invalid selection, or ``content.origin_missing`` when
                nonempty converted text has no overlapping original mapping.
        """
        view = self.view.slice(selection)
        if (
            len(self.origin_contents) == 1
            and self.origin_contents[0].artifact == view.body.artifact
        ):
            origins = (view.body,)
        else:
            origins = tuple(
                dict.fromkeys(
                    origin
                    for mapping in self.mappings
                    if mapping.content.selection.start_byte is not None
                    and mapping.content.selection.end_byte is not None
                    and mapping.content.selection.end_byte > view.byte_offset
                    and mapping.content.selection.start_byte
                    < view.byte_offset + len(view.text.encode("utf-8"))
                    for origin in mapping.origins
                )
            )
            if not origins and view.text:
                raise NousError(
                    "content.origin_missing", "Prepared text has no original source mapping"
                )
        return PreparedText(view, origins, self.session, self.mappings)

    async def semantic_content(self) -> ContentSelection:
        return (await self.session.semantic_contents((self,)))[0]


class TextPreparation:
    """Reuse immutable text preparation during one corpus build.

    Artifacts, media preparer and their resources are borrowed from the runtime.
    Caches belong to this session: equal bytes can share reading and parsing work,
    while each view retains its own artifact identity and original selections.
    """

    def __init__(
        self,
        artifacts: ArtifactStore,
        preparer: MediaPreparer,
        *,
        known_artifacts: Sequence[Artifact] = (),
    ) -> None:
        self.artifacts, self.preparer = artifacts, preparer
        self._artifacts = {item.ref.id: item for item in known_artifacts}
        self._blobs: dict[str, BlobRef] = {}
        self._views: dict[str, TextView] = {}
        self._texts: dict[str, str] = {}
        self._selected_views: dict[ContentSelection, TextView] = {}
        self._media: dict[str, PreparedText] = {}
        self.stats = {
            "bodies": 0,
            "unique_bodies": 0,
            "bytes_read": 0,
            "parsed": 0,
            "cache_hits": 0,
            "media_views": 0,
            "represented_text_bytes": 0,
        }

    async def preparation_identity(self, source: ContentSelection) -> dict[str, JsonValue]:
        """Identify text preparation without creating a derived representation."""
        artifact = self._artifacts.get(source.artifact.id)
        if artifact is None:
            artifact = await self.artifacts.get(source.artifact)
            self._artifacts[artifact.ref.id] = artifact
        elif artifact.ref != source.artifact:
            raise NousError("content.instance_mismatch", "Artifact belongs to another instance")
        identity: dict[str, JsonValue] = {"parser_profile": PARSER_PROFILE}
        if not is_text_media_type(artifact.media_type):
            info = await self.preparer.inspect(
                artifact, self.preparer.execution.snapshot.values.media
            )
            if info["kind"] != "pdf":
                raise NousError(
                    "media.text_representation_missing",
                    "This material requires explicit transcription or language interpretation to obtain text",
                )
            identity["extraction"] = self.preparer.text_view_parameters(info)
        return identity

    async def prime(self) -> None:
        """Load body bindings for constructor-supplied known artifacts in one view.

        Raises:
            NousError: ``content.missing`` if any known artifact has no body.
        """
        ids = list(self._artifacts)
        async with self.artifacts.database.read() as tx:
            blobs = await self.artifacts.content.resolve_many_in(tx, ids)
        self._blobs.update(zip(ids, blobs, strict=True))

    async def semantic_contents(
        self, values: Sequence[PreparedText]
    ) -> tuple[ContentSelection, ...]:
        """Reuse or adopt citation-projected semantic text in input order.

        Returns:
            Exact content selections for the projected values; inputs that need
            no projection may retain their existing body selection.
        """
        projections = await semantic_text_many(
            tuple(TextProjection(value.view.body, value.view.text, value.text) for value in values),
            self.preparer.results,
        )
        return tuple(item.content for item in projections)

    async def prepare_many(self, sources: Sequence[ContentSelection]) -> tuple[PreparedText, ...]:
        """Read overlapping requested text regions together, without filling unrequested gaps."""
        missing = tuple(
            dict.fromkeys(
                source.artifact for source in sources if source.artifact.id not in self._artifacts
            )
        )
        if missing:
            self._artifacts.update(
                (item.ref.id, item) for item in await self.artifacts.get_many(missing)
            )
        normalized = await self.artifacts.normalize_selections(sources)
        grouped: dict[ArtifactRef, list[tuple[int, int]]] = {}
        for source in normalized:
            selection = source.selection
            artifact = self._artifacts[source.artifact.id]
            if (
                is_text_media_type(artifact.media_type)
                and selection.start_byte is not None
                and selection.end_byte is not None
            ):
                grouped.setdefault(source.artifact, []).append(
                    (selection.start_byte, selection.end_byte)
                )
        limit = self.artifacts.text_read_max_bytes()
        for artifact, ranges in grouped.items():
            merged: list[tuple[int, int]] = []
            for start, end in sorted(set(ranges)):
                if (
                    merged
                    and start <= merged[-1][1]
                    and max(end, merged[-1][1]) - merged[-1][0] <= limit
                ):
                    merged[-1] = (merged[-1][0], max(end, merged[-1][1]))
                else:
                    merged.append((start, end))
            for start, end in merged:
                source = ContentSelection(
                    artifact=artifact, selection=SourceSelection(start_byte=start, end_byte=end)
                )
                blob = self._blobs.get(artifact.id) or await self.artifacts.blob(artifact)
                self._blobs[artifact.id] = blob
                if start == 0 and end == blob.byte_length:
                    await self.prepare(ContentSelection(artifact=artifact))
                else:
                    await self.prepare(source)
        return tuple([await self.prepare(source) for source in normalized])

    async def prepare(self, source: ContentSelection) -> PreparedText:
        """Prepare the exact artifact selection for this build session.

        Returns:
            A source-bound view and its original selections, including mapping
            back from media-derived text where applicable.

        Raises:
            NousError: ``content.instance_mismatch`` for a conflicting artifact
                identity, ``content.origin_missing`` for unmapped derived text,
                or a source-reading or media-preparation error.
        """
        artifact = self._artifacts.get(source.artifact.id)
        if artifact is None:
            artifact = await self.artifacts.get(source.artifact)
            self._artifacts[artifact.ref.id] = artifact
        elif artifact.ref != source.artifact:
            raise NousError(
                "content.instance_mismatch", "Artifact reference does not match this instance"
            )
        if is_text_media_type(artifact.media_type):
            view = self._views.get(artifact.ref.id)
            if view is None and not source.selection.is_whole:
                normalized = (await self.artifacts.normalize_selections((source,)))[0]
                view = self._selected_views.get(normalized)
                if view is None:
                    chosen = normalized.selection
                    for cached, candidate in self._selected_views.items():
                        bounds = cached.selection
                        if cached.artifact == normalized.artifact and (
                            bounds.start_byte is not None
                            and bounds.end_byte is not None
                            and chosen.start_byte is not None
                            and chosen.end_byte is not None
                            and bounds.start_byte
                            <= chosen.start_byte
                            <= chosen.end_byte
                            <= bounds.end_byte
                        ):
                            view = await self.artifacts.content.execution.blocking(
                                candidate.slice, chosen
                            )
                            break
                    if view is None:
                        view = await self.artifacts.text_view(normalized)
                    self._selected_views[normalized] = view
                return PreparedText(view, (view.body,), self)
            if view is None:
                blob = self._blobs.get(artifact.ref.id) or await self.artifacts.blob(artifact.ref)
                text = self._texts.get(blob.digest)
                if text is None:
                    text = await self.artifacts.content.read_text(
                        blob, self.artifacts.text_read_max_bytes()
                    )
                    self._texts[blob.digest] = text
                    self.stats["bytes_read"] += blob.byte_length
                    self.stats["unique_bodies"] += 1
                view, cached = await self.artifacts.text_cache.view(
                    artifact.ref, blob, text, self.artifacts.content.execution
                )
                self.stats["bodies"] += 1
                self.stats["cache_hits" if cached else "parsed"] += 1
                self._views[artifact.ref.id] = view
            prepared = PreparedText(view, (view.body,), self)
            return (
                prepared
                if source.selection.is_whole
                else await self.artifacts.content.execution.blocking(
                    prepared.slice, source.selection
                )
            )
        key = source.model_dump_json()
        prepared = self._media.get(key)
        if prepared is None:
            representation = await self.preparer.text_view(
                source, settings=self.preparer.execution.snapshot.values.media
            )
            origins = tuple(
                dict.fromkeys(
                    origin for mapping in representation.mappings for origin in mapping.origins
                )
            )
            if not origins and representation.view.text:
                raise NousError(
                    "content.origin_missing", "Prepared text has no original source mapping"
                )
            prepared = PreparedText(representation.view, origins, self, representation.mappings)
            self.stats["media_views"] += 1
            self.stats["represented_text_bytes"] += len(representation.view.text.encode("utf-8"))
            self._media[key] = prepared
        return prepared

    async def literal(self, text: str, display_name: str) -> ContentSelection:
        """Reuse an adopted deterministic text input without inventing another source."""
        key = hashlib.sha256(text.encode("utf-8")).hexdigest()
        result = await self.preparer.results.find_cached("content.literal_text", key)
        if result is None:
            result = await self.preparer.results.adopt(
                facts=ResultFacts(
                    operation="content.literal_text",
                    producer=ProgramProducer(operation="content.literal_text"),
                    selected_inputs=SelectedInputs(sources=()),
                    parameters={"cache_key": key},
                    formed_at=self.artifacts.clock.utc_now(),
                ),
                outputs={
                    "text": OutputContent(
                        data=text.encode("utf-8"),
                        media_type="text/plain",
                        display_name=display_name,
                    )
                },
            )
        return ContentSelection(
            artifact=next(item.artifact for item in result.outputs if item.name == "text")
        )
