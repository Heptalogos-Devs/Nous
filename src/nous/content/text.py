"""CommonMark source blocks over one immutable UTF-8 body."""

from __future__ import annotations

import io
from bisect import bisect_left, bisect_right
from functools import cached_property

from markdown_it import MarkdownIt
from pydantic import Field

from nous.common.errors import NousError

from .citations import citation_spans
from .contracts import ArtifactRef, ContentSelection, ContentValue, SourceSelection

PARSER_PROFILE = "commonmark-source-v1"


class BlockRef(ContentValue):
    body: ArtifactRef
    parser_profile: str = PARSER_PROFILE
    key: str
    content: ContentSelection
    view: ContentSelection | None = None


class TextBlock(ContentValue):
    key: str
    kind: str
    selection: ContentSelection
    ref: BlockRef
    text: str
    start_line: int
    end_line: int
    heading_path: tuple[str, ...] = ()
    parent_key: str | None = None


class TextChunk(ContentValue):
    text: str
    selection: ContentSelection
    blocks: tuple[BlockRef, ...]
    heading_path: tuple[str, ...] = ()


class BlockStructure(ContentValue):
    """Source-independent compact block boundaries in UTF-8 bytes."""

    key: str
    kind: str
    start: int
    end: int
    start_line: int
    end_line: int
    heading_path: tuple[str, ...] = ()
    parent_key: str | None = None


class TextView(ContentValue):
    """A CommonMark view whose blocks refer to exact immutable source bytes.

    Block keys are meaningful within the body and parser profile carried by
    their references. Byte selections use absolute UTF-8 offsets with exclusive
    ends; line selections use one-based inclusive line numbers. Rendered block
    text is for presentation and does not replace source selections.
    """

    body: ContentSelection
    parser_profile: str = PARSER_PROFILE
    structure: tuple[BlockStructure, ...] = ()
    text: str = Field(repr=False)
    byte_offset: int = 0
    first_line: int = 1
    citation_spans: tuple[tuple[int, int, str], ...] = ()

    @classmethod
    def parse(
        cls, body: ContentSelection, text: str, *, byte_offset: int = 0, first_line: int = 1
    ) -> TextView:
        lines = list(io.StringIO(text, newline=""))
        byte_offsets = [0]
        for line in lines:
            byte_offsets.append(byte_offsets[-1] + len(line.encode("utf-8")))
        environment: dict[str, object] = {}
        tokens = MarkdownIt("commonmark").parse(text, environment)
        heading_path: list[str] = []
        blocks: list[BlockStructure] = []
        previous_end = 0
        for index, token in enumerate(tokens):
            if token.level != 0 or token.nesting < 0 or token.map is None:
                continue
            start_line, end_line = token.map
            if start_line < previous_end or end_line <= start_line:
                continue
            if token.type == "heading_open":
                level = int(token.tag[1:])
                title = tokens[index + 1].content
                heading_path = heading_path[: level - 1]
                heading_path.append(title)
            start, end = byte_offsets[start_line], byte_offsets[min(end_line, len(lines))]
            key = f"B{len(blocks) + 1}"
            blocks.append(
                BlockStructure(
                    key=key,
                    kind=token.type.removesuffix("_open"),
                    start=byte_offset + start,
                    end=byte_offset + end,
                    start_line=first_line + start_line,
                    end_line=first_line + end_line - 1,
                    heading_path=tuple(heading_path),
                )
            )
            previous_end = end_line
        # CommonMark omits definition lines and whitespace from its root tokens.
        # Keep those byte ranges addressable alongside the parsed blocks.
        complete: list[BlockStructure] = []
        cursor = 0
        gaps = 0
        for block in blocks:
            start = block.start - byte_offset
            if cursor < start:
                gaps += 1
                complete.append(
                    BlockStructure(
                        key=f"G{gaps}",
                        kind="source_gap",
                        start=byte_offset + cursor,
                        end=byte_offset + start,
                        start_line=first_line + bisect_right(byte_offsets, cursor) - 1,
                        end_line=first_line + bisect_left(byte_offsets, start) - 1,
                    )
                )
            complete.append(block)
            cursor = block.end - byte_offset
        if cursor < byte_offsets[-1]:
            gaps += 1
            complete.append(
                BlockStructure(
                    key=f"G{gaps}",
                    kind="source_gap",
                    start=byte_offset + cursor,
                    end=byte_offset + byte_offsets[-1],
                    start_line=first_line + bisect_right(byte_offsets, cursor) - 1,
                    end_line=first_line + bisect_left(byte_offsets, byte_offsets[-1]) - 1,
                )
            )
        return cls(
            body=body,
            structure=tuple(complete),
            text=text,
            byte_offset=byte_offset,
            first_line=first_line,
            citation_spans=citation_spans(text, tokens, environment),
        )

    @cached_property
    def raw(self) -> bytes:
        return self.text.encode("utf-8")

    def _bind(self, value: BlockStructure) -> TextBlock:
        selected = ContentSelection(
            artifact=self.body.artifact,
            selection=SourceSelection(start_byte=value.start, end_byte=value.end),
        )
        raw = self.raw
        return TextBlock(
            key=value.key,
            kind=value.kind,
            selection=selected,
            ref=BlockRef(body=self.body.artifact, key=value.key, content=selected, view=self.body),
            text=raw[value.start - self.byte_offset : value.end - self.byte_offset].decode("utf-8"),
            start_line=value.start_line,
            end_line=value.end_line,
            heading_path=value.heading_path,
            parent_key=value.parent_key,
        )

    @property
    def blocks(self) -> tuple[TextBlock, ...]:
        return tuple(self._bind(value) for value in self.structure)

    def block(self, key: str) -> TextBlock:
        for value in self.structure:
            if value.key == key:
                return self._bind(value)
        raise NousError(
            "content.block_unknown", "Block is not part of this text view", details={"block": key}
        )

    def render(self) -> str:
        parts: list[str] = []
        previous = 0
        for block in self.structure:
            start = block.start - self.byte_offset
            parts.append(self.raw[previous:start].decode("utf-8"))
            parts.append(f"[{block.key}]\n")
            previous = start
        parts.append(self.raw[previous:].decode("utf-8"))
        return "".join(parts)

    def selection_bounds(self, selection: ContentSelection) -> tuple[int, int]:
        """Resolve a selection to absolute, half-open UTF-8 byte boundaries.

        The selection must name this view's artifact, stay within the view and
        land on character boundaries. Line selections are one-based and include
        both endpoints. Invalid selections raise ``content.invalid_selection``.
        """
        if selection.artifact != self.body.artifact:
            raise NousError("content.invalid_selection", "Selection belongs to a different body")
        selected = selection.selection
        raw = self.raw
        offset = self.byte_offset
        if selected.pages or any(
            value is not None
            for value in (selected.start_ms, selected.end_ms, selected.frame, selected.crop)
        ):
            raise NousError("content.invalid_selection", "A text view requires a text selection")
        if selected.start_line is not None or selected.end_line is not None:
            lines = list(io.StringIO(self.text, newline=""))
            first = selected.start_line or self.first_line
            last = selected.end_line or self.first_line + len(lines) - 1
            if first < self.first_line or last >= self.first_line + len(lines):
                raise NousError("content.invalid_selection", "Text selection exceeds the view")
            start = offset + len("".join(lines[: first - self.first_line]).encode("utf-8"))
            end = offset + len("".join(lines[: last - self.first_line + 1]).encode("utf-8"))
        else:
            start = selected.start_byte if selected.start_byte is not None else offset
            end = selected.end_byte if selected.end_byte is not None else offset + len(raw)
        if not offset <= start <= end <= offset + len(raw):
            raise NousError("content.invalid_selection", "Text selection exceeds the view")
        try:
            raw[: start - offset].decode("utf-8")
            raw[start - offset : end - offset].decode("utf-8")
        except UnicodeDecodeError as error:
            raise NousError(
                "content.invalid_selection", "Selection splits a UTF-8 character"
            ) from error
        return start, end

    def slice(self, selection: SourceSelection) -> TextView:
        """Select original bytes and rebind existing structure without parsing again."""
        start, end = self.selection_bounds(
            ContentSelection(artifact=self.body.artifact, selection=selection)
        )
        low, high = start - self.byte_offset, end - self.byte_offset
        raw = self.raw
        if low == 0 and high == len(raw):
            return self
        text = raw[low:high].decode("utf-8")
        body = ContentSelection(
            artifact=self.body.artifact,
            selection=SourceSelection(start_byte=start, end_byte=end),
        )
        preceding = raw[:low].decode("utf-8")
        return TextView.model_construct(
            body=body,
            parser_profile=self.parser_profile,
            text=text,
            byte_offset=start,
            first_line=self.first_line
            + preceding.count("\n")
            + preceding.count("\r")
            - preceding.count("\r\n"),
            structure=tuple(
                block for block in self.structure if start <= block.start and block.end <= end
            ),
            citation_spans=tuple(
                (left - low, right - low, label)
                for left, right, label in self.citation_spans
                if low <= left and right <= high
            ),
        )

    def complete_chunks(self, target_bytes: int) -> tuple[TextChunk, ...]:
        """Partition the entire raw view, retaining complete root blocks and gaps."""
        if target_bytes < 1:
            raise ValueError("Text chunk target must be positive")
        raw = self.raw
        boundaries = [0]
        for block in self.structure:
            end = block.end - self.byte_offset
            if end - boundaries[-1] >= target_bytes:
                boundaries.append(end)
        if boundaries[-1] != len(raw):
            boundaries.append(len(raw))
        return tuple(
            self._chunk(raw, start, end)
            for start, end in zip(boundaries, boundaries[1:], strict=False)
        )

    def _chunk(self, raw: bytes, start: int, end: int) -> TextChunk:
        selected = ContentSelection(
            artifact=self.body.artifact,
            selection=SourceSelection(
                start_byte=self.byte_offset + start, end_byte=self.byte_offset + end
            ),
        )
        overlapping = tuple(
            value
            for value in self.structure
            if value.end > self.byte_offset + start and value.start < self.byte_offset + end
        )
        refs = tuple(
            BlockRef(
                body=self.body.artifact,
                key=value.key,
                content=ContentSelection(
                    artifact=self.body.artifact,
                    selection=SourceSelection(start_byte=value.start, end_byte=value.end),
                ),
                view=self.body,
            )
            for value in overlapping
        )
        return TextChunk(
            text=raw[start:end].decode("utf-8"),
            selection=selected,
            blocks=refs,
            heading_path=overlapping[0].heading_path if overlapping else (),
        )

    def chunks(
        self, max_chars: int | None = None, *, max_bytes: int | None = None
    ) -> tuple[TextChunk, ...]:
        """Partition every source byte in order without splitting UTF-8 characters."""
        if max_chars is None and max_bytes is None:
            raise ValueError("Provide a character or byte limit")
        if max_chars is not None and max_chars < 1 or max_bytes is not None and max_bytes < 1:
            raise ValueError("Text chunk limits must be positive")
        raw = self.raw
        result: list[TextChunk] = []
        start = position = count = 0
        for character in self.text:
            size = len(character.encode("utf-8"))
            if max_bytes is not None and size > max_bytes:
                raise NousError(
                    "content.chunk_limit", "One UTF-8 character exceeds the chunk byte limit"
                )
            if count and (
                (max_chars is not None and count == max_chars)
                or (max_bytes is not None and position + size - start > max_bytes)
            ):
                result.append(self._chunk(raw, start, position))
                start, count = position, 0
            position += size
            count += 1
        if position > start:
            result.append(self._chunk(raw, start, position))
        return tuple(result)
