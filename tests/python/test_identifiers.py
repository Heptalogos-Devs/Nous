"""Identifier allocation and retrieval through the real storage and CLI paths."""

import asyncio
import json
import subprocess
import sys
from pathlib import Path
from unittest.mock import call, patch

import pytest

from nous.cognition.contracts import RecordView
from nous.cognition.writing.contracts import RecordOutcome
from nous.content.contracts import Artifact
from nous.runtime.bootstrap import initialize_instance, open_instance


def test_identifier_collision_retry_and_cli_roundtrip(tmp_path: Path) -> None:
    instance = tmp_path / "instance"

    async def allocate() -> tuple[str, str, str, str]:
        await initialize_instance(instance, environment={})
        async with open_instance(instance, environment={}) as runtime:
            with patch("nous.common.identifier.secrets.randbits", side_effect=[0, 0, 1]) as random:
                first = await runtime.artifacts.ingest("first")
                second = await runtime.artifacts.ingest("second")
            assert random.call_args_list == [call(65), call(65), call(65)]
            with patch("nous.common.identifier.secrets.randbits", return_value=(1 << 65) - 1):
                highest = await runtime.artifacts.ingest("highest")
            binary_path = tmp_path / "source.bin"
            binary_path.write_bytes(b"\x00\x01\x02")
            binary = await runtime.artifacts.ingest(binary_path)
            return first.ref.identifier, second.ref.identifier, highest.ref.identifier, binary.ref.identifier

    first, second, highest, binary = asyncio.run(allocate())
    assert first == "art(port with with with with with)"
    assert second == "art(port with with with with that)"
    assert highest == "art(poly ugly ugly ugly ugly ugly)"

    def invoke(*args: str) -> dict[str, object]:
        result = subprocess.run(
            [
                sys.executable,
                "-c",
                "from nous.transports.cli import main; main()",
                "--instance",
                str(instance),
                "--json",
                *args,
            ],
            capture_output=True,
            encoding="utf-8",
            timeout=45,
            check=True,
        )
        return json.loads(result.stdout)

    assert invoke("tools", "content", "read", first)["text"] == "first"
    assert invoke("tools", "content", "read", second)["text"] == "second"
    assert invoke("tools", "content", "read", highest)["text"] == "highest"
    assert invoke("tools", "content", "read", binary)["export_hint"] == (
        f'nous tools content export "{binary}" --output PATH'
    )
    exported = tmp_path / "exported.bin"
    invoke("tools", "content", "export", binary, "--output", str(exported))
    assert exported.read_bytes() == b"\x00\x01\x02"

    saved = RecordOutcome.model_validate(
        invoke("note", "add", "--text", "Wait for the worker.", "--source", first, "--remember")
    )
    note = saved.record.ref.identifier
    changed = RecordOutcome.model_validate(
        invoke("note", "replace", note + "@1", "--text", "Release after the worker finishes.")
    )
    assert changed.record.revision_ref.identifier == note + "@2"
    old_data = invoke("read", note + "@1", "--sources")
    old_data.pop("body_selection", None)
    old = RecordView.model_validate(old_data)
    assert old.body == "Wait for the worker."
    assert old.source_views["S1"][0].body_text == "first"
    assert old.source_views["S1"][0].export_ref.identifier == first
    inspected = Artifact.model_validate(invoke("tools", "media", "inspect", first)["artifact"])
    assert inspected.ref.identifier == first
    with pytest.raises(subprocess.CalledProcessError) as invalid:
        invoke("read", note + "@0")
    assert json.loads(invalid.value.stdout)["error"]["code"] == "input.invalid_reference"
