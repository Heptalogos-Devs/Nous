"""CommonMark source blocks over one immutable UTF-8 body."""

from __future__ import annotations

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


class TextView(ContentValue):
    """A CommonMark view whose blocks refer to exact immutable source bytes.

    Block keys are meaningful within the body and parser profile carried by
    their references. Byte selections use absolute UTF-8 offsets with exclusive
    ends; line selections use one-based inclusive line numbers. Rendered block
    text is for presentation and does not replace source selections.
    """

    body: ContentSelection
    parser_profile: str = PARSER_PROFILE
    blocks: tuple[TextBlock, ...]
    text: str = Field(repr=False)
    byte_offset: int = 0
    first_line: int = 1
    citation_spans: tuple[tuple[int, int, str], ...] = ()

    @classmethod
    def parse(
        cls, body: ContentSelection, text: str, *, byte_offset: int = 0, first_line: int = 1
    ) -> TextView:
        lines = text.splitlines(keepends=True)
        byte_offsets = [0]
        for line in lines:
            byte_offsets.append(byte_offsets[-1] + len(line.encode("utf-8")))
        raw = text.encode("utf-8")
        environment: dict[str, object] = {}
        tokens = MarkdownIt("commonmark").parse(text, environment)
        heading_path: list[str] = []
        blocks: list[TextBlock] = []
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
            selected = ContentSelection(
                artifact=body.artifact,
                selection=SourceSelection(
                    start_byte=byte_offset + start, end_byte=byte_offset + end
                ),
            )
            blocks.append(
                TextBlock(
                    key=key,
                    kind=token.type.removesuffix("_open"),
                    selection=selected,
                    ref=BlockRef(body=body.artifact, key=key, content=selected, view=body),
                    text=raw[start:end].decode("utf-8"),
                    start_line=first_line + start_line,
                    end_line=first_line + end_line - 1,
                    heading_path=tuple(heading_path),
                )
            )
            previous_end = end_line
        return cls(
            body=body,
            blocks=tuple(blocks),
            text=text,
            byte_offset=byte_offset,
            first_line=first_line,
            citation_spans=citation_spans(text, tokens, environment),
        )

    def block(self, key: str) -> TextBlock:
        selected = next((block for block in self.blocks if block.key == key), None)
        if selected is None:
            raise NousError(
                "content.block_unknown",
                "Block is not part of this text view",
                details={"block": key},
            )
        return selected

    def render(self) -> str:
        return "\n\n".join(f"[{block.key}]\n{block.text}" for block in self.blocks)

    def selection_bounds(self, selection: ContentSelection) -> tuple[int, int]:
        """Resolve a selection to absolute, half-open UTF-8 byte boundaries.

        The selection must name this view's artifact, stay within the view and
        land on character boundaries. Line selections are one-based and include
        both endpoints. Invalid selections raise ``content.invalid_selection``.
        """
        if selection.artifact != self.body.artifact:
            raise NousError("content.invalid_selection", "Selection belongs to a different body")
        selected = selection.selection
        raw = self.text.encode("utf-8")
        offset = self.byte_offset
        if selected.pages or any(
            value is not None
            for value in (selected.start_ms, selected.end_ms, selected.frame, selected.crop)
        ):
            raise NousError("content.invalid_selection", "A text view requires a text selection")
        if selected.start_line is not None or selected.end_line is not None:
            lines = self.text.splitlines(keepends=True)
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
            raise NousError("content.invalid_selection", "Selection splits a UTF-8 character") from error
        return start, end

    def slice(self, selection: SourceSelection) -> TextView:
        """Select original bytes and rebind existing structure without parsing again."""
        start, end = self.selection_bounds(ContentSelection(artifact=self.body.artifact, selection=selection))
        low, high = start - self.byte_offset, end - self.byte_offset
        raw = self.text.encode("utf-8")
        text = raw[low:high].decode("utf-8")
        body = ContentSelection(
            artifact=self.body.artifact,
            selection=SourceSelection(start_byte=start, end_byte=end),
        )
        preceding = raw[:low].decode("utf-8")
        return self.model_copy(
            update={
                "body": body,
                "text": text,
                "byte_offset": start,
                "first_line": self.first_line + preceding.count("\n") + preceding.count("\r") - preceding.count("\r\n"),
                "blocks": tuple(
                    block for block in self.blocks
                    if block.selection.selection.start_byte is not None
                    and block.selection.selection.end_byte is not None
                    and start <= block.selection.selection.start_byte
                    and block.selection.selection.end_byte <= end
                ),
                "citation_spans": tuple(
                    (left - low, right - low, label)
                    for left, right, label in self.citation_spans
                    if low <= left and right <= high
                ),
            }
        )

    def complete_chunks(self, target_bytes: int) -> tuple[TextChunk, ...]:
        """Group root blocks without splitting an oversized block."""
        if target_bytes < 1:
            raise ValueError("Text chunk target must be positive")
        raw = self.text.encode("utf-8")
        chunks: list[TextChunk] = []
        pending: list[TextBlock] = []

        def emit() -> None:
            if not pending:
                return
            start = pending[0].selection.selection.start_byte
            end = pending[-1].selection.selection.end_byte
            assert start is not None and end is not None
            chunks.append(
                TextChunk(
                    text=raw[start - self.byte_offset : end - self.byte_offset].decode("utf-8"),
                    selection=ContentSelection(
                        artifact=self.body.artifact,
                        selection=SourceSelection(start_byte=start, end_byte=end),
                    ),
                    blocks=tuple(block.ref for block in pending),
                    heading_path=pending[0].heading_path,
                )
            )
            pending.clear()

        for block in self.blocks:
            start = pending[0].selection.selection.start_byte if pending else None
            end = block.selection.selection.end_byte
            if start is not None and end is not None and end - start > target_bytes:
                emit()
            pending.append(block)
        emit()
        return tuple(chunks)

    def chunks(
        self, max_chars: int | None = None, *, max_bytes: int | None = None
    ) -> tuple[TextChunk, ...]:
        """Group complete root blocks; oversized blocks retain a parent reference when sliced."""
        if max_chars is None and max_bytes is None:
            raise ValueError("Provide a character or byte limit")
        if max_chars is not None and max_chars < 1 or max_bytes is not None and max_bytes < 1:
            raise ValueError("Text chunk limits must be positive")
        raw = self.text.encode("utf-8")
        result: list[TextChunk] = []
        pending: list[TextBlock] = []

        def fits(text: str) -> bool:
            return (max_chars is None or len(text) <= max_chars) and (
                max_bytes is None or len(text.encode("utf-8")) <= max_bytes
            )

        def slices(text: str) -> list[str]:
            start = count = size = 0
            output: list[str] = []
            for index, character in enumerate(text):
                length = len(character.encode("utf-8"))
                if max_bytes is not None and length > max_bytes:
                    raise NousError(
                        "content.chunk_limit", "One UTF-8 character exceeds the chunk byte limit"
                    )
                if count and (
                    (max_chars is not None and count == max_chars)
                    or (max_bytes is not None and size + length > max_bytes)
                ):
                    output.append(text[start:index])
                    start, count, size = index, 0, 0
                count += 1
                size += length
            if count:
                output.append(text[start:])
            return output

        def emit() -> None:
            if not pending:
                return
            start = pending[0].selection.selection.start_byte
            end = pending[-1].selection.selection.end_byte
            assert start is not None and end is not None
            result.append(
                TextChunk(
                    text=raw[start - self.byte_offset : end - self.byte_offset].decode("utf-8"),
                    selection=ContentSelection(
                        artifact=self.body.artifact,
                        selection=SourceSelection(start_byte=start, end_byte=end),
                    ),
                    blocks=tuple(block.ref for block in pending),
                    heading_path=pending[0].heading_path,
                )
            )
            pending.clear()

        for block in self.blocks:
            if not fits(block.text):
                emit()
                offset = block.selection.selection.start_byte
                assert offset is not None
                for part in slices(block.text):
                    end = offset + len(part.encode("utf-8"))
                    result.append(
                        TextChunk(
                            text=part,
                            selection=ContentSelection(
                                artifact=self.body.artifact,
                                selection=SourceSelection(start_byte=offset, end_byte=end),
                            ),
                            blocks=(block.ref,),
                            heading_path=block.heading_path,
                        )
                    )
                    offset = end
                continue
            if pending:
                start = pending[0].selection.selection.start_byte
                end = block.selection.selection.end_byte
                assert start is not None and end is not None
                if not fits(raw[start - self.byte_offset : end - self.byte_offset].decode("utf-8")):
                    emit()
            pending.append(block)
        emit()
        return tuple(result)
