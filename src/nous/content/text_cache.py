"""Disposable, bounded cache for source-independent Markdown structure."""

from __future__ import annotations

import hashlib
from collections import OrderedDict
from collections.abc import Callable
from pathlib import Path
from threading import RLock

from pydantic import BaseModel, ConfigDict

from nous.common.files import write_text
from nous.runtime.execution import ExecutionManager

from .configuration import TextCacheConfig
from .contracts import ArtifactRef, BlobRef, ContentSelection
from .structure_context import StructuralContextSpan
from .text import PARSER_PROFILE, BlockStructure, TextView


class CachedStructure(BaseModel):
    model_config = ConfigDict(extra="forbid", frozen=True)

    profile: str
    blocks: tuple[BlockStructure, ...]
    citations: tuple[tuple[int, int, str], ...]
    paragraphs: tuple[tuple[int, int], ...]
    contexts: tuple[StructuralContextSpan, ...]


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
        self._entries: OrderedDict[str, tuple[CachedStructure, int]] = OrderedDict()
        self._size = 0
        self._lock = RLock()

    @staticmethod
    def _serialize(payload: CachedStructure) -> str:
        checksum = hashlib.sha256(payload.model_dump_json().encode("utf-8")).hexdigest()
        return CachedText(structure=payload, checksum=checksum).model_dump_json()

    @staticmethod
    def _decode(serialized: str, text: str) -> CachedStructure:
        cached = CachedText.model_validate_json(serialized)
        payload = cached.structure
        checksum = hashlib.sha256(payload.model_dump_json().encode("utf-8")).hexdigest()
        if checksum != cached.checksum or payload.profile != PARSER_PROFILE:
            raise ValueError("Text structure is corrupt or has an unsupported profile")
        raw = text.encode("utf-8")
        for value in payload.blocks:
            if not 0 <= value.start <= value.end <= len(raw):
                raise ValueError("Text block exceeds source")
            raw[value.start : value.end].decode("utf-8")
        for start, end, _ in payload.citations:
            if not 0 <= start <= end <= len(raw):
                raise ValueError("Citation span exceeds source")
            raw[start:end].decode("utf-8")
        for start, end in payload.paragraphs:
            if not 0 <= start <= end <= len(raw):
                raise ValueError("Paragraph span exceeds source")
            raw[start:end].decode("utf-8")
        for item in payload.contexts:
            if not (
                0 <= item.scope_start <= item.scope_end <= len(raw)
                and 0 <= item.context_start <= item.context_end <= len(raw)
            ):
                raise ValueError("Structural context exceeds source")
            raw[item.scope_start : item.scope_end].decode("utf-8")
            raw[item.context_start : item.context_end].decode("utf-8")
        return payload

    def _remember(self, key: str, payload: CachedStructure, size: int, limit: int) -> None:
        with self._lock:
            self._remember_locked(key, payload, size, limit)

    def _remember_locked(self, key: str, payload: CachedStructure, size: int, limit: int) -> None:
        old = self._entries.pop(key, None)
        if old is not None:
            self._size -= old[1]
        while self._entries and self._size + size > limit:
            _, (_, evicted_size) = self._entries.popitem(last=False)
            self._size -= evicted_size
        if size <= limit:
            self._entries[key] = (payload, size)
            self._size += size

    def _view(self, artifact: ArtifactRef, blob: BlobRef, text: str) -> tuple[TextView, bool]:
        config = self.settings()
        key = hashlib.sha256(f"{PARSER_PROFILE}:{blob.digest}".encode()).hexdigest()
        with self._lock:
            entry = self._entries.get(key)
        path = self.root / (key + ".json") if self.root is not None else None
        payload, size = entry if entry is not None else (None, 0)
        if payload is None and path is not None and config.persist:
            try:
                serialized = path.read_text(encoding="utf-8")
                payload = self._decode(serialized, text)
                size = len(serialized.encode("utf-8"))
            except OSError, UnicodeError, ValueError, TypeError, KeyError, IndexError:
                try:
                    path.unlink(missing_ok=True)
                except OSError:
                    pass
        hit = payload is not None
        if payload is None:
            view = TextView.parse(ContentSelection(artifact=artifact), text)
            payload = CachedStructure(
                profile=view.parser_profile,
                blocks=view.structure,
                citations=view.citation_spans,
                paragraphs=view.paragraph_spans,
                contexts=view.context_spans,
            )
            serialized = self._serialize(payload)
            size = len(serialized.encode("utf-8"))
            if path is not None and config.persist:
                try:
                    write_text(path, serialized)
                except OSError:
                    pass
        self._remember(key, payload, size, config.max_bytes)
        return TextView.model_construct(
            body=ContentSelection(artifact=artifact),
            structure=payload.blocks,
            text=text,
            citation_spans=payload.citations,
            paragraph_spans=payload.paragraphs,
            context_spans=payload.contexts,
        ), hit

    async def view(
        self, artifact: ArtifactRef, blob: BlobRef, text: str, execution: ExecutionManager
    ) -> tuple[TextView, bool]:
        """Perform parsing, cache validation and lightweight binding in managed work."""
        return await execution.blocking(self._view, artifact, blob, text)
