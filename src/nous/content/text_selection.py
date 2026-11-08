"""Incremental exact UTF-8 text selection for local and streamed inputs."""

from __future__ import annotations

import codecs
import mmap
import os
import re

from nous.common.errors import NousError

from .contracts import SourceSelection

_LINES = re.compile(b"\r\n|\r|\n")


def read_seekable(
    descriptor: int, selection: SourceSelection, limit: int
) -> tuple[bytes, int, int]:
    """Select a regular file without copying or scanning past the requested range."""
    selected = TextSelection(selection, limit)
    _read_seekable(descriptor, selected)
    return selected.finish()


def _read_seekable(descriptor: int, selected: TextSelection) -> None:
    size = os.fstat(descriptor).st_size
    if not size or selected.done:
        return
    with mmap.mmap(descriptor, 0, access=mmap.ACCESS_READ) as data:
        if selected.by_line:
            first_start = 0
            for _ in range(selected.first - 1):
                match = _LINES.search(data, first_start)
                if match is None:
                    first_start = size
                    break
                first_start = match.end()
            boundary = min(size, first_start + selected.limit + 2)
            if selected.last is not None:
                position = first_start
                for _ in range(selected.last - selected.first + 1):
                    match = _LINES.search(data, position, boundary)
                    if match is None:
                        break
                    position = match.end()
                else:
                    boundary = position
        else:
            boundary = min(
                size,
                selected.end if selected.end is not None else selected.start + selected.limit + 1,
            )
        position = 0
        while position < boundary and not selected.done:
            end = min(boundary, position + 65536)
            selected.feed(data[position:end])
            position = end


def normalize_seekable(
    descriptor: int, selection: SourceSelection, byte_length: int
) -> SourceSelection:
    """Resolve text coordinates without retaining selected content."""
    selected = TextSelection(selection, byte_length, retain=False)
    if not selected.by_line:
        start = selected.start
        end = selected.end if selected.end is not None else byte_length
        if not 0 <= start <= end <= byte_length:
            raise NousError("content.invalid_selection", "Byte selection exceeds the artifact")
        for position in (start, end):
            if position < byte_length:
                os.lseek(descriptor, position, os.SEEK_SET)
                character = os.read(descriptor, 1)
                if not character:
                    raise NousError("content.corrupt", "Stored content is incomplete")
                if character[0] & 0xC0 == 0x80:
                    raise NousError(
                        "content.invalid_selection", "Selection splits a UTF-8 character"
                    )
        return SourceSelection(start_byte=start, end_byte=end)
    _read_seekable(descriptor, selected)
    selected.finish()
    return SourceSelection(
        start_byte=selected.selected_start,
        end_byte=selected.selected_start + selected.selected_length,
    )


class TextSelection:
    """Select bytes or lines while retaining only the requested range."""

    def __init__(self, selection: SourceSelection, limit: int, *, retain: bool = True) -> None:
        if selection.pages or any(
            value is not None
            for value in (selection.start_ms, selection.end_ms, selection.frame, selection.crop)
        ):
            raise NousError(
                "content.selection_requires_preparation",
                "Media coordinates require media preparation",
            )
        self.selection, self.limit = selection, limit
        self.start = selection.start_byte or 0
        self.end = selection.end_byte
        self.by_line = selection.start_line is not None or selection.end_line is not None
        self.first = selection.start_line or 1
        self.last = selection.end_line
        self.line: int = 1
        self.offset = 0
        self.selected_start = 0
        self.first_line = 1
        self.previous_cr = False
        self.line_data = False
        self.carry = b""
        self.output = bytearray()
        self.selected_length = 0
        self.retain = retain
        self.decoder = codecs.getincrementaldecoder("utf-8")()
        self.prefix_done = self.start == 0
        self.done = not self.by_line and self.end == 0

    def next_read_size(self) -> int:
        """Bound the next read to the remaining requested range."""
        if self.by_line:
            return 1
        if self.end is not None:
            return max(1, min(65536, self.end - self.offset))
        if self.prefix_done:
            return max(1, min(65536, self.limit + 1 - self.selected_length))
        return min(65536, self.start - self.offset)

    def _append(self, value: bytes) -> None:
        if self.selected_length + len(value) > self.limit:
            raise NousError("content.too_large", "Text selection exceeds the configured read limit")
        self.selected_length += len(value)
        if self.retain:
            self.output.extend(value)

    def _decode(self, value: bytes, *, final: bool = False, prefix: bool = False) -> str:
        try:
            return self.decoder.decode(value, final=final)
        except UnicodeDecodeError as error:
            raise NousError(
                "content.invalid_selection" if prefix else "content.not_text",
                "Selection splits a UTF-8 character" if prefix else "Input is not UTF-8 text",
            ) from error

    def feed(self, data: bytes) -> None:
        if self.done:
            return
        if self.by_line:
            self._feed_lines(data)
            return
        position = self.offset
        end = position + len(data)
        if not self.prefix_done:
            count = min(len(data), self.start - position)
            text = self._decode(data[:count], final=position + count == self.start, prefix=True)
            for char in text:
                if char == "\r" or char == "\n" and not self.previous_cr:
                    self.first_line += 1
                self.previous_cr = char == "\r"
            if position + count == self.start:
                self.prefix_done = True
                self.selected_start = self.start
                self.decoder = codecs.getincrementaldecoder("utf-8")()
        lo = max(0, self.start - position)
        hi = len(data) if self.end is None else min(len(data), max(0, self.end - position))
        if hi > lo:
            part = data[lo:hi]
            self._append(part)
            self._decode(part, prefix=not self.selection.is_whole)
        self.offset = end
        if self.end is not None and end >= self.end:
            self._decode(b"", final=True, prefix=not self.selection.is_whole)
            self.done = True

    def _feed_lines(self, data: bytes, *, final: bool = False) -> None:
        data = self.carry + data
        self.carry = b""
        if not final and data.endswith(b"\r"):
            self.carry, data = b"\r", data[:-1]
        position = 0
        for match in _LINES.finditer(data):
            part = data[position : match.end()]
            self._decode(part)
            if self.first <= self.line and (self.last is None or self.line <= self.last):
                self._append(part)
            self.offset += len(part)
            if self.last == self.line:
                self.done = True
                return
            self.line += 1
            self.line_data = False
            if self.line == self.first:
                self.selected_start = self.offset
            position = match.end()
        part = data[position:]
        if part:
            self._decode(part)
            self.line_data = True
            if self.first <= self.line and (self.last is None or self.line <= self.last):
                self._append(part)
            self.offset += len(part)

    def finish(self) -> tuple[bytes, int, int]:
        if self.by_line:
            if not self.done:
                self._feed_lines(b"", final=True)
            self._decode(b"", final=True)
            count = self.line if self.line_data else self.line - 1
            if not self.done and (
                self.first > count or self.last is not None and self.last > count
            ):
                raise NousError("content.invalid_selection", "Text selection exceeds the input")
            return bytes(self.output), self.selected_start, self.first
        if not self.done:
            if self.offset < self.start or self.end is not None and self.offset < self.end:
                raise NousError("content.invalid_selection", "Byte selection exceeds the input")
            self._decode(b"", final=True, prefix=not self.selection.is_whole)
        return bytes(self.output), self.selected_start, self.first_line
