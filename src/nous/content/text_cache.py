"""Disposable, bounded cache for source-independent Markdown structure."""

from __future__ import annotations

import hashlib
from collections import OrderedDict
from collections.abc import Callable
from pathlib import Path

from pydantic import BaseModel, ConfigDict

from nous.common.files import write_text
from nous.runtime.execution import ExecutionManager

from .configuration import TextCacheConfig
from .contracts import ArtifactRef, BlobRef, ContentSelection, SourceSelection
from .text import PARSER_PROFILE, BlockRef, TextBlock, TextView


class CachedBlock(BaseModel):
    model_config = ConfigDict(extra="forbid", frozen=True)

    key: str
    kind: str
    start: int
    end: int
    start_line: int
    end_line: int
    heading_path: tuple[str, ...]
    parent_key: str | None


class CachedStructure(BaseModel):
    model_config = ConfigDict(extra="forbid", frozen=True)

    profile: str
    blocks: tuple[CachedBlock, ...]
    citations: tuple[tuple[int, int, str], ...]


class CachedText(BaseModel):
    model_config = ConfigDict(extra="forbid", frozen=True)

    structure: CachedStructure
    checksum: str


class TextStructureCache:
    """Cache parse structure by bytes; bind each result to its own artifact identity."""

    def __init__(
        self,
        root: Path | None,
        settings: Callable[[], TextCacheConfig] | None,
    ) -> None:
        self.root = root / "text-structure" if root is not None else None
        self.settings = settings or TextCacheConfig
        self._entries: OrderedDict[str, str] = OrderedDict()
        self._size = 0

    @staticmethod
    def _serialize(view: TextView) -> str:
        blocks: list[CachedBlock] = []
        for block in view.blocks:
            selection = block.selection.selection
            assert selection.start_byte is not None and selection.end_byte is not None
            blocks.append(CachedBlock(
                key=block.key,
                kind=block.kind,
                start=selection.start_byte,
                end=selection.end_byte,
                start_line=block.start_line,
                end_line=block.end_line,
                heading_path=block.heading_path,
                parent_key=block.parent_key,
            ))
        structure = CachedStructure(
            profile=view.parser_profile, blocks=tuple(blocks), citations=view.citation_spans
        )
        checksum = hashlib.sha256(structure.model_dump_json().encode("utf-8")).hexdigest()
        return CachedText(structure=structure, checksum=checksum).model_dump_json()

    @staticmethod
    def _bind(serialized: str, artifact: ArtifactRef, text: str) -> TextView:
        cached = CachedText.model_validate_json(serialized)
        payload = cached.structure
        checksum = hashlib.sha256(payload.model_dump_json().encode("utf-8")).hexdigest()
        if checksum != cached.checksum:
            raise ValueError("Text structure is corrupt")
        if payload.profile != PARSER_PROFILE:
            raise ValueError("Text structure profile does not match")
        raw = text.encode("utf-8")
        body = ContentSelection(artifact=artifact)
        blocks: list[TextBlock] = []
        for value in payload.blocks:
            start, end = value.start, value.end
            if not 0 <= start <= end <= len(raw):
                raise ValueError("Text block exceeds source")
            selected = ContentSelection(
                artifact=artifact, selection=SourceSelection(start_byte=start, end_byte=end)
            )
            key = value.key
            blocks.append(TextBlock(
                key=key,
                kind=value.kind,
                selection=selected,
                ref=BlockRef(body=artifact, key=key, content=selected, view=body),
                text=raw[start:end].decode("utf-8"),
                start_line=value.start_line,
                end_line=value.end_line,
                heading_path=value.heading_path,
                parent_key=value.parent_key,
            ))
        spans: list[tuple[int, int, str]] = []
        for start, end, label in payload.citations:
            if not 0 <= start <= end <= len(raw):
                raise ValueError("Citation span exceeds source")
            raw[start:end].decode("utf-8")
            spans.append((start, end, label))
        return TextView(body=body, blocks=tuple(blocks), text=text, citation_spans=tuple(spans))

    def _remember(self, key: str, value: str, limit: int) -> None:
        old = self._entries.pop(key, None)
        if old is not None:
            self._size -= len(old.encode("utf-8"))
        size = len(value.encode("utf-8"))
        while self._entries and self._size + size > limit:
            _, evicted = self._entries.popitem(last=False)
            self._size -= len(evicted.encode("utf-8"))
        if size <= limit:
            self._entries[key] = value
            self._size += size

    async def view(
        self, artifact: ArtifactRef, blob: BlobRef, text: str, execution: ExecutionManager
    ) -> tuple[TextView, bool]:
        config = self.settings()
        key = hashlib.sha256(f"{PARSER_PROFILE}:{blob.digest}".encode()).hexdigest()
        cached = self._entries.get(key)
        path = self.root / (key + ".json") if self.root is not None else None
        if cached is None and path is not None and config.persist:
            try:
                cached = await execution.blocking(path.read_text, encoding="utf-8")
            except (OSError, UnicodeError):
                cached = None
        if cached is not None:
            try:
                view = self._bind(cached, artifact, text)
            except (ValueError, TypeError, KeyError, IndexError, UnicodeError):
                cached = None
            else:
                self._remember(key, cached, config.max_bytes)
                return view, True
        view = await execution.blocking(TextView.parse, ContentSelection(artifact=artifact), text)
        serialized = self._serialize(view)
        self._remember(key, serialized, config.max_bytes)
        if path is not None and config.persist:
            try:
                await execution.blocking(write_text, path, serialized)
            except OSError:
                pass
        return view, False
