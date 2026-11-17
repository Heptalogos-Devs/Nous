"""Resource identity and execution contracts exercised through the real Runtime."""

import asyncio
import json
import os
import shutil
import subprocess
import sys
from pathlib import Path

import pytest

from nous.common.errors import NousError
from nous.configuration import Configuration
from nous.runtime.bootstrap import initialize_instance, open_instance
from nous.storage.database import Database


def test_portable_move_and_resource_replacement(tmp_path: Path) -> None:
    async def run() -> None:
        original = tmp_path / "original"
        await initialize_instance(original, environment={})
        async with open_instance(original, environment={}) as runtime:
            identity = (await runtime.database.check()).database_id
        moved = tmp_path / "移动 instance"
        original.rename(moved)
        async with open_instance(moved, environment={}) as runtime:
            assert (await runtime.database.check()).database_id == identity

        for kind in ("data", "content", "index"):
            alternate = tmp_path / ("new-" + kind)
            environment = {f"NOUS__PATHS__{kind.upper()}_DIR": str(alternate)}
            with pytest.raises(NousError, match="relocation") as rejected:
                async with open_instance(moved, environment=environment):
                    pytest.fail("A bound resource directory was silently changed")
            assert rejected.value.code == "configuration.relocation_required"
            assert not alternate.exists()

        config_file = moved / "nous.toml"
        saved = config_file.read_text(encoding="utf-8")
        config_file.write_text(saved + '\n[paths]\nindex_dir="different-index"\n', encoding="utf-8")
        with pytest.raises(NousError) as manual:
            async with open_instance(moved, environment={}):
                pytest.fail("A manually edited resource path was silently adopted")
        assert manual.value.code == "configuration.relocation_required"
        config_file.write_text(saved, encoding="utf-8")

        content = moved / "content"
        content.rename(moved / "old-content")
        content.mkdir()
        with pytest.raises(NousError) as missing:
            async with open_instance(moved, environment={}):
                pytest.fail("An empty replacement content directory was adopted")
        assert missing.value.code == "instance.resource_missing"
        content.rmdir()
        (moved / "old-content").rename(content)

        settings = Configuration(config_file, environment={}).snapshot.values
        foreign = moved / "data" / "foreign.sqlite3"
        async with Database.open(foreign, settings.database, settings.instance.id, create=True):
            pass
        database_file = moved / "data" / settings.database.filename
        database_file.unlink()
        foreign.rename(database_file)
        with pytest.raises(NousError) as different_database:
            async with open_instance(moved, environment={}):
                pytest.fail("A different database with the same instance UUID was adopted")
        assert different_database.value.code == "storage.binding_conflict"

    asyncio.run(run())


def test_distinct_locators_share_the_data_lock(tmp_path: Path) -> None:
    async def run() -> None:
        original, alias = tmp_path / "config-a", tmp_path / "config-b"
        environment = {
            f"NOUS__PATHS__{kind.upper()}_DIR": str(tmp_path / "shared" / kind)
            for kind in ("data", "content", "index")
        }
        await initialize_instance(original, environment=environment)
        alias.mkdir()
        for filename in ("nous.toml", ".nous-instance.json"):
            shutil.copyfile(original / filename, alias / filename)
        child_environment = {
            key: value for key, value in os.environ.items() if not key.upper().startswith("NOUS__")
        }
        child_environment.update(environment)
        command = [
            sys.executable,
            "-c",
            "from nous.transports.cli import main; main()",
            "--instance",
            str(alias),
            "--json",
            "runtime",
            "check",
        ]
        async with open_instance(original, environment=environment):
            child = await asyncio.to_thread(
                subprocess.run,
                command,
                env=child_environment,
                capture_output=True,
                encoding="utf-8",
                timeout=20,
            )
        assert child.returncode != 0
        assert json.loads(child.stdout)["error"]["code"] == "instance.in_use"
        async with open_instance(alias, environment=environment) as runtime:
            assert (await runtime.database.check()).foreign_keys

    asyncio.run(run())


def test_nested_operation_keeps_snapshot_and_total_deadline(tmp_path: Path) -> None:
    async def run() -> None:
        await initialize_instance(tmp_path / "instance", environment={})
        async with open_instance(
            tmp_path / "instance",
            environment={},
            overrides={"execution.default_timeout_seconds": 1},
        ) as runtime:
            async with runtime.execution.operation("outer") as outer:
                await runtime.configuration.update({"logging.level": "WARNING"})
                assert outer.deadline is not None
                async with runtime.execution.operation(
                    "inner", deadline=outer.deadline + 20
                ) as inner:
                    assert inner.settings is outer.settings
                    assert inner.deadline == outer.deadline
                with pytest.raises(NousError) as conflict:
                    async with runtime.execution.operation(
                        "different-snapshot", runtime.configuration.snapshot
                    ):
                        pytest.fail("A nested operation changed the operation snapshot")
                assert conflict.value.code == "operation.snapshot_conflict"

            deadline = asyncio.get_running_loop().time() + 0.03
            with pytest.raises(TimeoutError):
                async with runtime.execution.operation("deadline-owner", deadline=deadline):
                    async with runtime.execution.operation(
                        "deadline-child", deadline=deadline + 20
                    ):
                        await asyncio.sleep(10)

    asyncio.run(run())
