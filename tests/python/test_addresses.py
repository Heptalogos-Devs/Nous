"""Address allocation and retrieval through the real storage and CLI paths."""

import asyncio
import json
import subprocess
import sys
from pathlib import Path
from unittest.mock import call, patch

from nous.runtime.bootstrap import initialize_instance, open_instance


def test_address_collision_retry_and_cli_roundtrip(tmp_path: Path) -> None:
    instance = tmp_path / "instance"

    async def allocate() -> tuple[str, str, str, str]:
        await initialize_instance(instance, environment={})
        async with open_instance(instance, environment={}) as runtime:
            with patch("nous.common.address.secrets.randbits", side_effect=[0, 0, 1]) as random:
                first = await runtime.artifacts.ingest("first")
                second = await runtime.artifacts.ingest("second")
            assert random.call_args_list == [call(65), call(65), call(65)]
            with patch("nous.common.address.secrets.randbits", return_value=(1 << 65) - 1):
                highest = await runtime.artifacts.ingest("highest")
            binary_path = tmp_path / "source.bin"
            binary_path.write_bytes(b"\x00\x01\x02")
            binary = await runtime.artifacts.ingest(binary_path)
            return first.ref.address, second.ref.address, highest.ref.address, binary.ref.address

    first, second, highest, binary = asyncio.run(allocate())
    assert first == "art:port with with with with with"
    assert second == "art:port with with with with that"
    assert highest == "art:poly ugly ugly ugly ugly ugly"

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
