"""Configuration failures keep durable intent and active resources observable."""

import asyncio
import contextvars
import json
import logging
import os
import subprocess
import sys
from collections.abc import Mapping
from pathlib import Path
from unittest.mock import patch

import pytest
from pydantic import JsonValue, TypeAdapter

import nous.temporal.domains as time_domains
from nous.common.errors import NousError
from nous.configuration import Configuration
from nous.configuration.values import get_path
from nous.runtime.bootstrap import initialize_instance, open_instance
from nous.storage.database import WriteTx
from nous.temporal.domains import TimeDomainDefinition


def test_environment_resolves_generated_property_spelling_and_array_values(tmp_path: Path) -> None:
    instance = tmp_path / "generated-environment"
    asyncio.run(initialize_instance(instance, environment={}))
    variable = "NOUS__WAVE__NATIVE__QUERY__PIPELINE__ACTIVATIONMULTIPLIER"
    configuration = Configuration(instance / "nous.toml", environment={variable: "[1.0,1.25]"})
    pipeline = configuration.snapshot.values.wave.native.query["pipeline"]
    assert isinstance(pipeline, Mapping)
    assert pipeline["activationMultiplier"] == (1.0, 1.25)
    assert "activationmultiplier" not in pipeline
    assert configuration.explain("wave.native.query.pipeline.activationMultiplier")["source"] == {
        "layer": "environment",
        "location": variable,
    }


def test_configuration_update_obeys_runtime_admission(tmp_path: Path) -> None:
    async def scenario() -> None:
        instance = tmp_path / "configuration-admission"
        await initialize_instance(instance, environment={})
        async with open_instance(instance, environment={}) as runtime:
            await runtime.configuration.update(
                {
                    "execution.max_in_flight": 1,
                    "execution.default_timeout_seconds": 0.05,
                }
            )
        async with open_instance(instance, environment={}) as runtime:
            before = (instance / "nous.toml").read_bytes()
            snapshot = runtime.configuration.snapshot
            async with runtime.execution.operation(
                "hold.admission", deadline=asyncio.get_running_loop().time() + 2
            ):
                task = asyncio.create_task(
                    runtime.configuration.update({"logging.level": "DEBUG"}),
                    context=contextvars.Context(),
                )
                # The independent request must time out while waiting for the
                # only root slot, before it can change the file or live logger.
                with pytest.raises(TimeoutError):
                    await task
            assert runtime.configuration.snapshot is snapshot
            assert (instance / "nous.toml").read_bytes() == before

    asyncio.run(scenario())


def test_configuration_failure_recovery(tmp_path: Path) -> None:
    async def scenario() -> None:
        instance = tmp_path / "configuration-failures"
        await initialize_instance(instance, environment={})
        config_file = instance / "nous.toml"
        async with open_instance(instance, environment={}) as runtime:
            service = runtime.configuration
            active = service.snapshot
            original_file = config_file.read_bytes()
            original_entries = set(instance.iterdir())
            with patch(
                "nous.common.files.os.replace",
                side_effect=PermissionError("replace denied"),
            ):
                with pytest.raises(NousError) as failed_save:
                    await service.update({"execution.default_timeout_seconds": 2})
            assert failed_save.value.code == "configuration.save_failed"
            assert failed_save.value.details["file_saved"] is False
            assert failed_save.value.details["effective_version"] == active.version
            assert service.version == active.version
            assert service.snapshot is active and not service.pending_restart
            assert config_file.read_bytes() == original_file
            assert set(instance.iterdir()) == original_entries

            register = time_domains.register_domain

            async def fail_registration(
                definition: TimeDomainDefinition, tx: WriteTx
            ) -> TimeDomainDefinition:
                await register(definition, tx)
                if definition.domain_id == "colony":
                    raise RuntimeError("failure after insertion, before commit")
                return definition

            with patch.object(time_domains, "register_domain", fail_registration):
                with pytest.raises(NousError) as failed_binding:
                    await service.update(
                        {
                            "time.domains.colony": {
                                "kind": "linear",
                                "name": "Colony",
                                "tick_seconds": "1/1000",
                            }
                        }
                    )
            error = failed_binding.value
            assert error.code == "configuration.apply_failed"
            assert error.details["file_saved"] is True
            assert error.details["phase"] == "binding_apply"
            assert error.details["binding_application"] == "unconfirmed"
            assert error.details["version"] == active.version + 1
            assert error.details["effective_version"] == active.version
            assert error.details["recovery"]
            assert service.snapshot is active and service.pending_restart
            assert "colony" not in await runtime.time.domains()
            assert "colony" in TypeAdapter(dict[str, JsonValue]).validate_python(
                get_path(service.saved(), "time.domains")
            )
            pending = await service.update({"execution.default_timeout_seconds": 7})
            assert pending["state"] == "saved_pending_restart"
            assert pending["effective_version"] == active.version
            assert service.snapshot is active
            pending_reload = await service.reload()
            assert pending_reload["state"] == "saved_pending_restart"
            assert pending_reload["write_performed"] is False
            assert service.snapshot is active
        async with open_instance(instance, environment={}) as recovered:
            assert "colony" in await recovered.time.domains()
            assert recovered.configuration.snapshot.values.execution.default_timeout_seconds == 7
            assert not recovered.configuration.pending_restart

        async with open_instance(instance, environment={}) as runtime:
            service = runtime.configuration
            active = service.snapshot
            root = logging.getLogger()
            assert runtime.observability.logging is not None
            route = runtime.observability.logging.route
            inherited = route.getChild("configuration-live")
            previous_level = root.level
            previous_route_level = route.level
            original_set_level = route.setLevel
            assert not inherited.isEnabledFor(logging.DEBUG)

            def fail_after_level_change(level: int | str) -> None:
                original_set_level(level)
                assert inherited.isEnabledFor(logging.DEBUG)
                raise RuntimeError("logging failure after changing the level")

            with patch.object(route, "setLevel", fail_after_level_change):
                with pytest.raises(NousError) as failed_live:
                    await service.update(
                        {
                            "logging.level": "DEBUG",
                            "time.domains.orbit": {"kind": "opaque", "name": "Orbit"},
                        }
                    )
            error = failed_live.value
            assert error.code == "configuration.apply_failed"
            assert error.details["phase"] == "live_apply"
            assert error.details["file_saved"] is True
            assert error.details["binding_application"] == "applied"
            assert error.details["live_application"] == "failed"
            assert root.level == previous_level and not inherited.isEnabledFor(logging.DEBUG)
            assert route.level == previous_route_level
            assert service.snapshot is active and service.pending_restart
            # Registration is committed, while operations still retain their old configuration.
            assert "orbit" in await runtime.time.domains()
            assert "orbit" not in service.resolve().values.time.domains
            with pytest.raises(NousError) as conflict:
                await service.update(
                    {"execution.default_timeout_seconds": 9}, expected_version=active.version
                )
            assert conflict.value.code == "configuration.version_conflict"
            assert conflict.value.details["effective_version"] == active.version
            pending = await service.update({"execution.default_timeout_seconds": 9})
            assert pending["state"] == "saved_pending_restart"
            assert service.snapshot is active
        async with open_instance(instance, environment={}) as recovered:
            assert recovered.configuration.snapshot.values.logging.level == "DEBUG"
            assert recovered.configuration.snapshot.values.execution.default_timeout_seconds == 9
            assert "orbit" in await recovered.time.domains()

        async with open_instance(instance, environment={}) as runtime:
            service = runtime.configuration
            active = service.snapshot
            reached = asyncio.Event()

            async def cancel_during_registration(
                definition: TimeDomainDefinition, tx: WriteTx
            ) -> TimeDomainDefinition:
                await register(definition, tx)
                if definition.domain_id == "cancelled-domain":
                    reached.set()
                    await asyncio.Future[None]()
                return definition

            with patch.object(time_domains, "register_domain", cancel_during_registration):
                updating = asyncio.create_task(
                    service.update(
                        {"time.domains.cancelled-domain": {"kind": "opaque", "name": "Cancelled"}}
                    )
                )
                await asyncio.wait_for(reached.wait(), timeout=5)
                updating.cancel()
                with pytest.raises(asyncio.CancelledError):
                    await updating
            assert service.snapshot is active and service.pending_restart
            assert (
                TypeAdapter(dict[str, JsonValue]).validate_python(
                    service.status()["activation_failure"]
                )["file_saved"]
                is True
            )
            assert "cancelled-domain" not in await runtime.time.domains()
        async with open_instance(instance, environment={}) as recovered:
            assert "cancelled-domain" in await recovered.time.domains()

        original_file = config_file.read_bytes()
        command = "from unittest.mock import patch\nfrom nous.transports.cli import main\nwith patch('nous.common.files.os.replace',side_effect=PermissionError('replace denied')):\n main()"
        failed_cli = await asyncio.to_thread(
            subprocess.run,
            [
                sys.executable,
                "-c",
                command,
                "--instance",
                str(instance),
                "--json",
                "config",
                "set",
                "execution.default_timeout_seconds",
                "12",
            ],
            capture_output=True,
            text=True,
            encoding="utf-8",
            env={
                name: value
                for name, value in os.environ.items()
                if not name.upper().startswith("NOUS_")
            },
            timeout=30,
            check=False,
        )
        assert failed_cli.returncode == 4
        result = json.loads(failed_cli.stdout)["error"]
        assert result["code"] == "configuration.save_failed"
        assert result["details"]["file_saved"] is False
        assert result["details"]["recovery"]
        assert config_file.read_bytes() == original_file

    asyncio.run(scenario())
