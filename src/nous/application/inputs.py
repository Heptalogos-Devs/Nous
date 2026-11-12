"""Resolve declared command inputs within the owning application request."""

from __future__ import annotations

import asyncio
import mimetypes
from collections.abc import AsyncGenerator, AsyncIterable, Callable, Iterable, Mapping
from dataclasses import dataclass
from pathlib import Path
from types import MappingProxyType
from typing import TYPE_CHECKING, cast

from nous.common.errors import NousError
from nous.content.contracts import (
    Artifact,
    ArtifactInput,
    BodySpec,
    ContentSelection,
    ExternalInput,
    InlineText,
    ResultOutputInput,
    ResultOutputRef,
    SourceSelection,
    SourceTarget,
    is_text_media_type,
)
from nous.runtime.execution import complete_cleanup
from nous.storage.file_input import FileReaderPool

if TYPE_CHECKING:
    from nous.content.artifacts import ArtifactStore
    from nous.content.results import ResultStore
    from nous.runtime.execution import ExecutionManager


@dataclass(frozen=True)
class FileInput:
    """A caller-owned file, with optional original identity for staged delivery."""

    path: Path
    name: str | None = None
    source: str | None = None


@dataclass(frozen=True)
class StreamInput:
    """A single-use stream supplied by the caller."""

    stream: AsyncIterable[bytes] | Iterable[bytes]
    name: str = "input"
    source: str | None = None


type InputProvider = FileInput | StreamInput


async def input_chunks(
    value: InputProvider,
    readers: FileReaderPool,
    chunk_size: int,
    execution: ExecutionManager | None = None,
) -> AsyncGenerator[bytes]:
    """Read one caller-owned input with the same semantics in SDK and host delivery."""

    async def blocking[T](action: Callable[..., T], *args: object) -> T:
        if execution is not None:
            return await execution.blocking(action, *args)
        return await complete_cleanup(asyncio.to_thread(action, *args))

    if isinstance(value, FileInput):
        chunks = readers.chunks(value.path, chunk_size)
        try:
            async for chunk in chunks:
                yield chunk
        except TimeoutError:
            raise
        except OSError as error:
            raise NousError(
                "input.unreadable",
                "Cannot read external input",
                details={"source": value.source or str(value.path)},
                cause=error,
            ) from error
        finally:
            await chunks.aclose()
        return
    if isinstance(value.stream, AsyncIterable):
        iterator = aiter(value.stream)
        try:
            async for chunk in iterator:
                if not isinstance(cast(object, chunk), bytes):
                    raise NousError("input.invalid_stream", "Input stream must yield bytes")
                yield chunk
        finally:
            close = getattr(iterator, "aclose", None)
            if close is not None:
                await close()
        return
    iterator = iter(value.stream)
    sentinel = object()
    try:
        while True:
            chunk = await blocking(next, iterator, sentinel)
            if chunk is sentinel:
                break
            if not isinstance(chunk, bytes):
                raise NousError("input.invalid_stream", "Input stream must yield bytes")
            yield chunk
    finally:
        close = getattr(iterator, "close", None)
        if callable(close):
            await blocking(close)


@dataclass(frozen=True)
class SelectedExternalText:
    text: str
    data: bytes
    selection: SourceSelection
    name: str
    source: str | None


class InputReader:
    """Resolve only the external bindings supplied for this application call."""

    def __init__(
        self,
        bindings: Mapping[str, InputProvider],
        *,
        artifacts: ArtifactStore | None,
        results: ResultStore | None = None,
        readers: FileReaderPool,
        execution: ExecutionManager,
        chunk_size: int = 1024 * 1024,
    ) -> None:
        if any(not isinstance(cast(object, key), str) or not key for key in bindings):
            raise NousError("input.invalid_binding", "Input keys must be nonempty strings")
        if any(
            not isinstance(cast(object, value), (FileInput, StreamInput))
            for value in bindings.values()
        ):
            raise NousError("input.invalid_binding", "Unsupported external input binding")
        self._bindings = MappingProxyType(dict(bindings))
        self._artifacts = artifacts
        self._results = results
        self._readers = readers
        self._execution = execution
        self._chunk_size = chunk_size
        self._consumed: set[str] = set()

    def _get(self, key: str) -> InputProvider:
        try:
            value = self._bindings[key]
        except KeyError as error:
            raise NousError(
                "input.missing_binding", "External input is not bound", details={"key": key}
            ) from error
        if isinstance(value, StreamInput):
            if key in self._consumed:
                raise NousError(
                    "input.consumed",
                    "External input stream was already consumed",
                    details={"key": key},
                )
            self._consumed.add(key)
        return value

    @staticmethod
    def _identity(value: InputProvider) -> tuple[str, str | None]:
        if isinstance(value, FileInput):
            return value.name or value.path.name, value.source or str(value.path)
        return value.name, value.source

    async def _chunks(self, value: InputProvider) -> AsyncGenerator[bytes]:
        chunks = input_chunks(value, self._readers, self._chunk_size, self._execution)
        try:
            async for chunk in chunks:
                yield chunk
        finally:
            await chunks.aclose()

    async def accept(self, key: str, *, name: str | None = None) -> Artifact:
        """Accept the complete external input as an immutable original."""
        artifacts = self._artifacts
        if artifacts is None:
            raise NousError("input.unavailable", "This operation cannot accept an artifact")
        value = self._get(key)
        input_name, source = self._identity(value)
        chunks = self._chunks(value)
        try:
            published = await artifacts.content.publish(await artifacts.content.prepare(chunks))
        finally:
            await chunks.aclose()
        display_name = name or input_name
        media_type = mimetypes.guess_type(display_name)[0] or "application/octet-stream"
        async with artifacts.database.write() as tx:
            artifact = await artifacts.register_in(
                tx,
                published,
                media_type=media_type,
                display_name=display_name,
                source=source,
                received_at=artifacts.clock.utc_now(),
                formed_at=None,
            )
        return artifact

    async def read_text(
        self, key: str, selection: SourceSelection, limit: int
    ) -> SelectedExternalText:
        """Read a bounded selected text range without accepting an extra artifact."""
        from nous.content.text_selection import TextSelection

        value = self._get(key)
        name, source = self._identity(value)
        if isinstance(value, FileInput):
            chunks = self._readers.chunks(
                value.path, self._chunk_size, selection=selection, limit=limit
            )
            try:
                data = b"".join([chunk async for chunk in chunks])
            except TimeoutError:
                raise
            except OSError as error:
                raise NousError(
                    "input.unreadable",
                    "Cannot read external input",
                    details={"source": value.source or str(value.path)},
                    cause=error,
                ) from error
            finally:
                await chunks.aclose()
        else:
            selected = TextSelection(selection, limit)
            chunks = self._chunks(value)
            try:
                async for chunk in chunks:
                    selected.feed(chunk)
                    if selected.done:
                        break
                data, _, _ = selected.finish()
            finally:
                await chunks.aclose()
        try:
            text = data.decode("utf-8")
        except UnicodeDecodeError as error:
            raise NousError(
                "content.not_text", "Selection does not contain complete UTF-8 text"
            ) from error
        return SelectedExternalText(
            text=text, data=data, selection=selection, name=name, source=source
        )

    async def accept_target(self, target: SourceTarget) -> tuple[Artifact, SourceSelection]:
        """Resolve any declared source and retain the caller's exact selection."""
        if isinstance(target, ExternalInput):
            return await self.accept(target.key), target.selection
        artifacts = self._artifacts
        if artifacts is None:
            raise NousError("input.unavailable", "This operation cannot accept an artifact")
        if isinstance(target, InlineText):
            return await artifacts.ingest(target.text, display_name=target.name), SourceSelection()
        if isinstance(target, ArtifactInput):
            return await artifacts.get(target.artifact), target.selection
        if self._results is None:
            raise NousError("input.unavailable", "Stored results are unavailable")
        result = await self._results.get(target.result)
        ref = await self._results.resolve_output(
            ResultOutputRef(result=result.ref, output=target.output)
        )
        return await artifacts.get(ref), target.selection

    async def body(self, value: BodySpec) -> str:
        """Resolve one text-use input, preserving the same range and size rules."""
        from nous.content.text_selection import TextSelection

        limit = self._execution.snapshot.values.content.text_read_max_bytes
        if isinstance(value, ExternalInput):
            return (await self.read_text(value.key, value.selection, limit)).text
        if isinstance(value, InlineText):
            selected = TextSelection(SourceSelection(), limit)
            selected.feed(value.text.encode("utf-8"))
            data, _, _ = selected.finish()
            return data.decode("utf-8")
        artifacts = self._artifacts
        if artifacts is None:
            raise NousError("input.unavailable", "Stored content is unavailable")
        if isinstance(value, ArtifactInput):
            artifact = await artifacts.get(value.artifact)
        else:
            assert isinstance(value, ResultOutputInput)
            if self._results is None:
                raise NousError("input.unavailable", "Stored results are unavailable")
            result = await self._results.get(value.result)
            ref = await self._results.resolve_output(
                ResultOutputRef(result=result.ref, output=value.output)
            )
            artifact = await artifacts.get(ref)
        if (
            not is_text_media_type(artifact.media_type)
            and artifact.media_type != "application/octet-stream"
        ):
            raise NousError("content.not_text", "The selected content is not text")
        return await artifacts.read_selection(
            ContentSelection(artifact=artifact.ref, selection=value.selection)
        )
