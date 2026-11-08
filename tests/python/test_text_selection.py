"""Exact text selections must survive arbitrary input chunk boundaries."""

import asyncio
import os
import sys
from pathlib import Path

import pytest

from nous.common.errors import NousError
from nous.content.contracts import SourceSelection
from nous.content.text_selection import TextSelection, normalize_seekable, read_seekable
from nous.storage.file_input import FileReaderPool


def test_streamed_text_selection_preserves_coordinates_and_bounds(tmp_path: Path) -> None:
    raw = "甲\r\n乙\r丙\n丁".encode()
    cases = (
        (SourceSelection(), raw, 0, 1),
        (SourceSelection(start_line=2, end_line=3), "乙\r丙\n".encode(), 5, 2),
        (SourceSelection(start_byte=5, end_byte=9), "乙\r".encode(), 5, 2),
        (SourceSelection(start_byte=len(raw), end_byte=len(raw)), b"", len(raw), 4),
    )
    path = tmp_path / "input.txt"
    path.write_bytes(raw)
    for size in range(1, len(raw) + 1):
        for selection, expected, start, line in cases:
            selected = TextSelection(selection, 100)
            for index in range(0, len(raw), size):
                selected.feed(raw[index : index + size])
                if selected.done:
                    break
            assert selected.finish() == (expected, start, line)
            with path.open("rb") as stream:
                normalized = normalize_seekable(stream.fileno(), selection, len(raw))
                assert normalized == SourceSelection(
                    start_byte=start, end_byte=start + len(expected)
                )
                assert read_seekable(stream.fileno(), selection, 100) == (expected, start, line)

    for selection, code in (
        (SourceSelection(start_byte=1, end_byte=3), "content.invalid_selection"),
        (SourceSelection(start_line=5), "content.invalid_selection"),
        (SourceSelection(), "content.too_large"),
    ):
        selected = TextSelection(selection, 3)
        with pytest.raises(NousError) as failure:
            for byte in raw:
                selected.feed(bytes((byte,)))
                if selected.done:
                    break
            selected.finish()
        assert failure.value.code == code

    empty = TextSelection(SourceSelection(), 3)
    assert empty.finish() == (b"", 0, 1)


@pytest.mark.skipif(not sys.platform.startswith("linux"), reason="Linux procfs is required")
def test_selected_procfs_reads_the_callers_process() -> None:
    async def read() -> bytes:
        pool = FileReaderPool(1)
        chunks = pool.chunks(
            Path("/proc/self/status"),
            4096,
            selection=SourceSelection(start_line=1, end_line=8),
            limit=4096,
        )
        try:
            return b"".join([chunk async for chunk in chunks])
        finally:
            await chunks.aclose()
            await pool.aclose()

    assert f"Pid:\t{os.getpid()}\n".encode() in asyncio.run(read())
