"""The local runtime's user-visible path and its persistent contracts."""

import asyncio
from pathlib import Path

import pytest

from nous.cognition.writing.contracts import (
    AddNote,
    ReadRecord,
    ReplaceRecord,
)
from nous.cognition.writing.operations import (
    ADD_NOTE,
    HISTORY_RECORD,
    READ_RECORD,
    REPLACE_RECORD,
)
from nous.common.errors import NousError
from nous.content.contracts import (
    InlineText,
)
from nous.runtime.bootstrap import initialize_instance, open_instance
from nous.runtime.instance import InstanceLock


def test_edit_retains_locked_and_conflicting_drafts_and_replays_unconfirmed_save(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    import click
    from typer.testing import CliRunner

    from nous.transports.cli import cli
    from nous.transports.cli import cognition as commands

    instance = tmp_path / "editing"

    async def create():
        await initialize_instance(instance, environment={})
        async with open_instance(instance, environment={}) as runtime:
            return await runtime.app.call(ADD_NOTE, AddNote(body=InlineText(text="Original body")))

    original = asyncio.run(create())
    args = ["--instance", str(instance), "--json", "note", "edit"]
    runner = CliRunner()
    lock = InstanceLock(instance)

    def edit_while_locked(*, filename: str) -> None:
        Path(filename).write_text("Retained under lock", encoding="utf-8")
        lock.__enter__()

    monkeypatch.setattr(click, "edit", edit_while_locked)
    try:
        failed = runner.invoke(cli, [*args, original.commit.ref.identifier])
        assert isinstance(failed.exception, NousError)
        assert failed.exception.code == "instance.in_use"
    finally:
        lock.__exit__(None, None, None)
    descriptor = next((instance / "data/recovery/edits").glob("*.json"))
    file = descriptor.with_suffix(".md")
    assert file.read_text() == "Retained under lock"
    saved = runner.invoke(cli, [*args, "--resume", str(descriptor)])
    assert saved.exit_code == 0, saved.output
    assert not descriptor.exists() and not file.exists()

    def edit_again(*, filename: str) -> None:
        Path(filename).write_text("Committed before output failed", encoding="utf-8")

    monkeypatch.setattr(click, "edit", edit_again)
    emit_record = commands.emit_record

    def unavailable_output(*_args: object) -> None:
        raise OSError("terminal unavailable")

    monkeypatch.setattr(commands, "emit_record", unavailable_output)
    failed_output = runner.invoke(cli, [*args, original.commit.ref.identifier])
    assert isinstance(failed_output.exception, OSError)
    descriptor = next((instance / "data/recovery/edits").glob("*.json"))
    monkeypatch.setattr(commands, "emit_record", emit_record)

    async def history():
        async with open_instance(instance, environment={}) as runtime:
            return await runtime.app.call(HISTORY_RECORD, ReadRecord(target=original.commit.ref))

    before = asyncio.run(history())
    resumed = runner.invoke(cli, [*args, "--resume", str(descriptor)])
    assert resumed.exit_code == 0, resumed.output
    assert asyncio.run(history()) == before
    assert not descriptor.exists()

    monkeypatch.setattr(click, "edit", edit_while_locked)
    try:
        runner.invoke(cli, [*args, original.commit.ref.identifier])
    finally:
        lock.__exit__(None, None, None)
    descriptor = next((instance / "data/recovery/edits").glob("*.json"))

    async def advance_head():
        async with open_instance(instance, environment={}) as runtime:
            current = await runtime.app.call(READ_RECORD, ReadRecord(target=original.commit.ref))
            await runtime.app.call(
                REPLACE_RECORD,
                ReplaceRecord(
                    target=current.ref,
                    body=InlineText(text="Another writer"),
                    expected=current.revision_ref,
                ),
            )

    asyncio.run(advance_head())
    conflict = runner.invoke(cli, [*args, "--resume", str(descriptor)])
    assert isinstance(conflict.exception, NousError)
    assert conflict.exception.code == "cognition.revision_conflict"
    assert (
        conflict.exception.details["expected_revision"]
        != conflict.exception.details["current_revision"]
    )
    assert descriptor.with_suffix(".md").read_text() == "Retained under lock"

    moved = tmp_path / "relocated editing"
    instance.rename(moved)
    descriptor = moved / "data/recovery/edits" / descriptor.name
    moved_conflict = runner.invoke(
        cli, ["--instance", str(moved), "--json", "note", "edit", "--resume", str(descriptor)]
    )
    assert isinstance(moved_conflict.exception, NousError)
    assert moved_conflict.exception.code == "cognition.revision_conflict"
    assert descriptor.with_suffix(".md").read_text() == "Retained under lock"
