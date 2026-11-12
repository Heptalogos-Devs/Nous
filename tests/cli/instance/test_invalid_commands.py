"""The local runtime's user-visible path and its persistent contracts."""

import asyncio
import json
from pathlib import Path

from nous.runtime.bootstrap import initialize_instance
from tests.cli.shared.host import command


def test_cli_invalid_arguments_and_media_fail_as_json(tmp_path: Path) -> None:
    instance = tmp_path / "invalid-cli"
    asyncio.run(initialize_instance(instance, environment={}))
    invalid_pdf = tmp_path / "broken.pdf"
    invalid_pdf.write_bytes(b"%PDF-1.7\nnot a readable document")
    text = tmp_path / "source.txt"
    text.write_text("A source", encoding="utf-8")
    for arguments, code in (
        (("tools", "media", "analyze"), "input.invalid"),
        (("tools", "media", "analyze", str(text), "--strategy", "wrong"), "input.invalid"),
        (("not-a-command",), "input.invalid"),
        (("tools", "media", "inspect", str(invalid_pdf)), "media.invalid_pdf"),
        (
            (
                "tools",
                "media",
                "analyze",
                str(text),
                "--strategy",
                "description_only",
                "--from",
                "inf",
            ),
            "media.invalid_selection",
        ),
        (
            (
                "tools",
                "media",
                "analyze",
                str(text),
                "--strategy",
                "description_only",
                "--to",
                "nan",
            ),
            "media.invalid_selection",
        ),
    ):
        result = command(instance, *arguments)
        assert result.returncode != 0
        assert json.loads(result.stdout)["error"]["code"] == code
        assert "Traceback" not in result.stderr
