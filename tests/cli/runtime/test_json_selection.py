"""JSON field selection applies to the same CLI results and keeps saved references."""

import asyncio
import json
from pathlib import Path

from tests.cli.shared.host import command, initialize


def test_cli_select_validates_before_writes_and_reports_completed_refs(tmp_path: Path) -> None:
    instance = tmp_path / "instance"
    asyncio.run(initialize(instance))
    invalid = command(instance, "--select", "not-a-pointer", "note", "add", "--text", "No write")
    assert invalid.returncode != 0
    assert json.loads(invalid.stdout)["error"]["code"] == "input.invalid_selection"
    before = command(instance, "note", "list")
    assert before.returncode == 0 and json.loads(before.stdout)["records"] == []

    missing = command(instance, "--select", "/missing", "note", "add", "--text", "Saved body")
    assert missing.returncode != 0
    error = json.loads(missing.stdout)["error"]
    assert error["code"] == "input.invalid_selection"
    revision = next(
        ref for ref in error["details"]["completed_refs"] if ref.get("kind") == "record_revision"
    )
    ref = f"{revision['record_ref']['identifier']}@{revision['ordinal']}"
    selected = command(instance, "--select", "/selections/0/body_text", "read", ref)
    assert selected.returncode == 0, selected.stdout
    assert json.loads(selected.stdout) == {"/selections/0/body_text": "Saved body"}
    query = command(instance, "query", f'@ref("{ref}")')
    assert query.returncode == 0, query.stdout
    outcome = json.loads(query.stdout)
    assert outcome["evidence_ref"] is None
    query_file = tmp_path / "query.json"
    query_file.write_text(query.stdout, encoding="utf-8")
    saved = command(instance, "save-evidence", str(query_file))
    assert saved.returncode == 0, saved.stdout
    delivery_ref = json.loads(saved.stdout)["identifier"]
    passage_key = outcome["evidence"]["passages"][0]["key"]
    read = command(instance, "read", delivery_ref, "--part", passage_key)
    assert read.returncode == 0, read.stdout
    assert json.loads(read.stdout)["selections"][0]["body_text"] == "Saved body"
    schema = command(
        instance,
        "--select",
        "/result/properties/body_selection",
        "operations",
        "describe",
        "records.read",
    )
    assert schema.returncode == 0
    assert json.loads(schema.stdout)["/result/properties/body_selection"]
