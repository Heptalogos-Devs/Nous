"""Recovery of completed generation and media stages."""

import asyncio
import json
import os
import shutil
import signal
import sqlite3
import subprocess
import sys
import wave
from http.server import BaseHTTPRequestHandler
from pathlib import Path
from typing import cast

import pytest
from model_support import configure_models, model_cli, model_server
from pydantic import JsonValue, TypeAdapter

from nous.application.configuration import open_configuration_operations


def test_language_response_survives_interpretation_failure_and_reopen(tmp_path: Path) -> None:
    """A saved protocol response outlives prompt changes and is adopted without another request."""
    from dataclasses import replace
    from unittest.mock import patch

    from nous.common.errors import NousError
    from nous.models.generation import GenerationRequest, PendingResponse
    from nous.models.tasks import TaskDefinition
    from nous.runtime.bootstrap import open_instance

    requests: list[dict[str, JsonValue]] = []

    class Handler(BaseHTTPRequestHandler):
        def do_POST(self) -> None:
            request = TypeAdapter(dict[str, JsonValue]).validate_json(
                self.rfile.read(int(self.headers["Content-Length"]))
            )
            requests.append(request)
            payload = json.dumps(
                {
                    "id": "durable-response",
                    "object": "chat.completion",
                    "created": 1700000000,
                    "model": "local-model",
                    "choices": [
                        {
                            "index": 0,
                            "message": {
                                "role": "assistant",
                                "content": "Persisted language output.",
                            },
                            "finish_reason": "stop",
                        }
                    ],
                }
            ).encode()
            self.send_response(200)
            self.send_header("Content-Type", "application/json")
            self.send_header("Content-Length", str(len(payload)))
            self.end_headers()
            self.wfile.write(payload)

        def log_message(self, format: str, *args: object) -> None:
            pass

    async def fail_interpretation(*args: object, **kwargs: object) -> dict[str, object]:
        raise NousError("test.interpretation_failed", "Downstream interpretation failed")

    instance = tmp_path / "durable-language"
    with model_server(Handler) as server:
        configure_models(instance, server)

        async def scenario() -> None:
            async with open_instance(instance, environment={}) as runtime:
                snapshot = runtime.configuration.snapshot
                source = await runtime.artifacts.ingest(
                    "[label]: source:S1\r\n\r\nBody [label].\r\n\r\n"
                )
                frozen_ref = await runtime.generation.freeze_language_task("read", snapshot)
                selected = await runtime.generation.accept_inputs(source, snapshot)
                request = GenerationRequest(frozen_ref, selected)
                with patch.object(TaskDefinition, "outputs", new=fail_interpretation):
                    with pytest.raises(
                        NousError, match="Downstream interpretation failed"
                    ) as failed:
                        await runtime.generation.produce(
                            request,
                            snapshot=snapshot,
                            computation_key="workflow:stable:compute",
                        )
                assert failed.value.details["response_result_ref"]
                response = await runtime.results.find_producer("workflow:stable:compute:response")
                assert response is not None
                output = next(
                    value.artifact for value in response.outputs if value.name == "response"
                )
                assert "durable-response" in await runtime.artifacts.read_text(output)
                resource = runtime.prompts.get("summarize")
                directory = runtime.paths.config / "prompts" / "summarize"
                directory.mkdir(parents=True)
                (directory / "prompt.toml").write_text(resource.manifest_text, encoding="utf-8")
                for name, template in resource.templates.items():
                    target = directory / name
                    target.parent.mkdir(parents=True, exist_ok=True)
                    target.write_text(template + "\nCHANGED TEMPLATE", encoding="utf-8")
            async with open_instance(instance, environment={}) as runtime:
                snapshot = runtime.configuration.snapshot
                with patch.object(
                    runtime.generation,
                    "load_language_task",
                    side_effect=AssertionError("provider lookup"),
                ):
                    produced = await runtime.generation.produce(
                        request, snapshot=snapshot, computation_key="workflow:stable:compute"
                    )
                    recovered = await runtime.generation.recover_response(response.ref, snapshot)
                assert recovered == produced
                tasks = runtime.generation.executor.compiler.catalog.tasks
                definition = tasks["reading-summary"]
                with patch.dict(
                    tasks.definitions,
                    {
                        "reading-summary": replace(
                            definition, named_outputs=(("output", "Updated explanatory help"),)
                        )
                    },
                ):
                    assert (
                        await runtime.generation.recover_response(response.ref, snapshot)
                        == produced
                    )
                with patch.dict(
                    tasks.definitions,
                    {"reading-summary": replace(definition, program_outputs=("different",))},
                ):
                    with pytest.raises(NousError) as changed:
                        await runtime.generation.recover_response(response.ref, snapshot)
                    assert changed.value.code == "model.task_contract_changed"
                prepared = await runtime.generation.prepare_adoption(
                    produced, producer_key="workflow:stable:result"
                )
                async with runtime.database.write() as tx:
                    adopted = await runtime.generation.adopt_in(tx, prepared)
                async with runtime.database.write() as tx:
                    repeated = await runtime.generation.adopt_in(tx, prepared)
                assert repeated.ref == adopted.ref
                assert produced.outputs["output"].data == b"Persisted language output."
                assert len(requests) == 1
                assert "[label]: source:S1\\r\\n" in json.dumps(requests[0])
                with patch.object(
                    runtime.generation.responses,
                    "_adopt_response",
                    side_effect=NousError("test.adopt", "save failed"),
                ):
                    with pytest.raises(NousError) as failed:
                        await runtime.generation.produce(
                            request, snapshot=snapshot, computation_key="workflow:second"
                        )
                pending = cast(object, failed.value.__dict__.get("nous_recovery"))
                assert isinstance(pending, PendingResponse)
                with patch.object(
                    runtime.results,
                    "prepare_outputs",
                    side_effect=AssertionError("already published"),
                ):
                    restored = await runtime.generation.resume_response(pending, snapshot)
                assert restored.outputs["output"].data == b"Persisted language output."
                with pytest.raises(NousError) as conflict:
                    await runtime.generation.resume_response(
                        replace(pending, identity="different-request"), snapshot
                    )
                assert conflict.value.code == "content.producer_conflict"
                assert len(requests) == 2
                renamed = selected.model_copy(
                    update={
                        "sources": tuple(
                            item.model_copy(update={"display_name": "renamed"})
                            for item in selected.sources
                        )
                    }
                )
                same = await runtime.generation.produce(
                    GenerationRequest(frozen_ref, renamed),
                    snapshot=snapshot,
                    computation_key="workflow:stable:compute",
                )
                assert same.outputs["output"] == produced.outputs["output"]
                first, second = await asyncio.gather(
                    *(
                        runtime.generation.produce(
                            request, snapshot=snapshot, computation_key="workflow:concurrent"
                        )
                        for _ in range(2)
                    )
                )
                assert first.outputs["output"] == second.outputs["output"]
                assert len(requests) == 3

                prepare_outputs = runtime.results.prepare_outputs

                async def fail_response_publication(outputs: object) -> object:
                    if isinstance(outputs, dict) and {"response", "computation"} <= outputs.keys():
                        raise OSError("response storage unavailable")
                    return await prepare_outputs(outputs)  # type: ignore[arg-type]

                with patch.object(
                    runtime.results, "prepare_outputs", side_effect=fail_response_publication
                ):
                    with pytest.raises(OSError) as failed:
                        await runtime.generation.produce(
                            request, snapshot=snapshot, computation_key="workflow:publish-failure"
                        )
                pending = failed.value.__dict__.get("nous_recovery")
                assert isinstance(pending, PendingResponse) and pending.outputs is None
                assert len(requests) == 4
                resumed = await runtime.generation.resume_response(pending, snapshot)
                assert resumed.outputs["output"].data == b"Persisted language output."
                assert len(requests) == 4

                with patch.object(
                    runtime.results, "prepare_outputs", side_effect=fail_response_publication
                ):
                    with pytest.raises(OSError):
                        await runtime.generation.produce(
                            request, snapshot=snapshot, computation_key="workflow:retry-publication"
                        )
                recovered = await runtime.generation.produce(
                    request, snapshot=snapshot, computation_key="workflow:retry-publication"
                )
                assert recovered.outputs["output"].data == b"Persisted language output."
                assert len(requests) == 5

                with patch.object(
                    runtime.generation.responses.journal,
                    "save",
                    side_effect=OSError("recovery storage unavailable"),
                ):
                    with pytest.raises(OSError) as failed:
                        await runtime.generation.produce(
                            request, snapshot=snapshot, computation_key="workflow:journal-failure"
                        )
                pending = failed.value.__dict__.get("nous_recovery")
                assert isinstance(pending, PendingResponse)
                with pytest.raises(NousError) as conflict:
                    await runtime.generation.resume_response(
                        replace(pending, identity="different-request"), snapshot
                    )
                assert conflict.value.code == "content.producer_conflict"
                resumed = await runtime.generation.resume_response(pending, snapshot)
                assert resumed.outputs["output"].data == b"Persisted language output."
                assert len(requests) == 6

        asyncio.run(scenario())
        crash_program = """import asyncio, os, signal, sys
from pathlib import Path
from unittest.mock import patch
from nous.models.generation import GenerationRequest
from nous.runtime.bootstrap import open_instance

async def run():
    async with open_instance(sys.argv[1], environment={}) as runtime:
        snapshot = runtime.configuration.snapshot
        source = await runtime.artifacts.ingest("Language response crash recovery source.")
        selected = await runtime.generation.accept_inputs(source, snapshot)
        Path(sys.argv[2]).write_text(selected.model_dump_json(), encoding="utf-8")
        request = GenerationRequest("read", selected)
        original = runtime.results.prepare_outputs
        async def fail_response(outputs):
            if {"response", "computation"} <= outputs.keys():
                await original({"response": outputs["response"]})
                raise OSError("injected response publication failure")
            return await original(outputs)
        with patch.object(runtime.results, "prepare_outputs", side_effect=fail_response):
            try:
                await runtime.generation.produce(request, snapshot=snapshot, computation_key=sys.argv[3])
            except OSError as error:
                assert error.__dict__["nous_context"]["producer_key"] == sys.argv[3] + ":response"
            else:
                raise AssertionError("expected response publication failure")
        assert len(list((runtime.artifacts.content.root / "pending" / "responses").glob("*.json"))) == 1
        if hasattr(signal, "SIGKILL"):
            os.kill(os.getpid(), signal.SIGKILL)
        os._exit(91)
asyncio.run(run())
"""
        for mode in ("resume", "retry"):
            selection_path = tmp_path / f"selected-{mode}.json"
            key = f"workflow:crash-{mode}"
            before = len(requests)
            crashed = subprocess.run(
                [sys.executable, "-c", crash_program, str(instance), str(selection_path), key],
                capture_output=True,
                text=True,
                timeout=60,
                check=False,
            )
            assert crashed.returncode == (-signal.SIGKILL if hasattr(signal, "SIGKILL") else 91), (
                crashed.stdout,
                crashed.stderr,
            )
            assert len(requests) == before + 1

            async def restore(
                mode: str = mode, key: str = key, selection_path: Path = selection_path
            ) -> None:
                from nous.content.contracts import SelectedInputs

                async with open_instance(instance, environment={}) as runtime:
                    snapshot = runtime.configuration.snapshot
                    if mode == "resume":
                        tasks = runtime.generation.executor.compiler.catalog.tasks
                        definition = tasks["reading-summary"]
                        with patch.dict(
                            tasks.definitions,
                            {"reading-summary": replace(definition, program_outputs=("changed",))},
                        ):
                            with pytest.raises(NousError) as changed:
                                await runtime.generation.resume_response(
                                    key + ":response", snapshot
                                )
                            assert changed.value.code == "model.task_contract_changed"
                        restored = await runtime.generation.resume_response(
                            key + ":response", snapshot
                        )
                    else:
                        selected = SelectedInputs.model_validate_json(
                            await runtime.execution.blocking(
                                selection_path.read_text, encoding="utf-8"
                            )
                        )
                        restored = await runtime.generation.produce(
                            GenerationRequest("read", selected),
                            snapshot=snapshot,
                            computation_key=key,
                        )
                    assert restored.outputs["output"].data == b"Persisted language output."
                    assert not list(
                        (runtime.artifacts.content.root / "pending" / "responses").glob("*.json")
                    )

            asyncio.run(restore())
            assert len(requests) == before + 1

        for failed_stage in ("response", "output", "committed"):
            cli_program = """from nous.content.results import ResultStore
original = ResultStore.prepare_outputs
async def fail_response(self, outputs):
    if __FAILED_STAGE__ == "response" and {"response", "computation"} <= outputs.keys():
        await original(self, {"response": outputs["response"]})
        raise OSError("injected response publication failure")
    if __FAILED_STAGE__ == "output" and "output" in outputs:
        await original(self, {"output": outputs["output"]})
        raise OSError("injected result publication failure")
    return await original(self, outputs)
ResultStore.prepare_outputs = fail_response
if __FAILED_STAGE__ == "committed":
    import nous.models.generation.operations as operations
    def fail_after_commit(ref):
        raise OSError("injected post-commit failure")
    operations.record_completed = fail_after_commit
from nous.transports.cli import main
main()
""".replace("__FAILED_STAGE__", repr(failed_stage))
            before = len(requests)
            failed_cli = subprocess.run(
                [
                    sys.executable,
                    "-c",
                    cli_program,
                    "--instance",
                    str(instance),
                    "--json",
                    "tools",
                    "model",
                    "run",
                    "--task",
                    "read",
                    "--text",
                    "CLI recovery source",
                ],
                env={
                    key: value for key, value in os.environ.items() if not key.startswith("NOUS__")
                },
                capture_output=True,
                text=True,
                timeout=60,
                check=False,
            )
            assert failed_cli.returncode != 0, (failed_cli.stdout, failed_cli.stderr)
            response_key = json.loads(failed_cli.stdout)["error"]["details"]["producer_key"]
            result_key = response_key.removesuffix(":response") + ":result"
            with sqlite3.connect(instance / "data" / "nous.sqlite3") as connection:
                already_committed = connection.execute(
                    "SELECT identifier FROM processing_results WHERE producer_key = ?",
                    (result_key,),
                ).fetchone()
            assert (already_committed is not None) == (failed_stage == "committed")
            assert len(requests) == before + 1
            recovered_cli = model_cli(
                instance, "tools", "model", "recover", "--producer-key", response_key
            )
            assert recovered_cli.returncode == 0, (recovered_cli.stdout, recovered_cli.stderr)
            repeated_cli = model_cli(
                instance, "tools", "model", "recover", "--producer-key", response_key
            )
            assert repeated_cli.returncode == 0, (repeated_cli.stdout, repeated_cli.stderr)
            recovered_ref = json.loads(recovered_cli.stdout)["results"][0]["ref"]
            assert recovered_ref == json.loads(repeated_cli.stdout)["results"][0]["ref"]
            if already_committed is not None:
                assert recovered_ref["identifier"] == already_committed[0]
            assert len(requests) == before + 1


def test_writing_recovery_keeps_base_sources_and_adoption_after_reopen(tmp_path: Path) -> None:
    """A saved writing intent and response complete without another model request."""
    from unittest.mock import patch

    from nous.cognition.writing.contracts import AfterWrite, SourceRequest
    from nous.cognition.writing.drafts import DraftStore
    from nous.common.errors import NousError
    from nous.content.contracts import ResultRef
    from nous.runtime.bootstrap import open_instance

    requests: list[dict[str, JsonValue]] = []

    class Handler(BaseHTTPRequestHandler):
        def do_POST(self) -> None:
            requests.append(
                TypeAdapter(dict[str, JsonValue]).validate_json(
                    self.rfile.read(int(self.headers["Content-Length"]))
                )
            )
            body = json.dumps(
                {
                    "id": "rewrite",
                    "object": "chat.completion",
                    "created": 1700000000,
                    "model": "local-model",
                    "choices": [
                        {
                            "index": 0,
                            "message": {
                                "role": "assistant",
                                "content": "# Rewritten\n\nRetained facts.",
                            },
                            "finish_reason": "stop",
                        }
                    ],
                }
            ).encode()
            self.send_response(200)
            self.send_header("Content-Type", "application/json")
            self.send_header("Content-Length", str(len(body)))
            self.end_headers()
            self.wfile.write(body)

        def log_message(self, format: str, *args: object) -> None:
            pass

    instance = tmp_path / "writing-recovery"
    with model_server(Handler) as server:
        configure_models(instance, server)

        async def scenario() -> None:
            async with open_configuration_operations(instance, environment={}) as config:
                await config.update(
                    {
                        "tasks.journal-rewrite": {
                            "model": "reader",
                            "prompt": "journal-rewrite",
                            "output_mode": "text",
                        }
                    }
                )
            async with open_instance(instance, environment={}) as runtime:
                cognition = await runtime.cognition()
                note = (await cognition.records.add_note("A source")).record
                base = (await cognition.records.create_journal("Original body")).record
                with patch.object(
                    DraftStore, "save_produced", side_effect=NousError("test.save", "save failed")
                ):
                    with pytest.raises(NousError) as failed:
                        await cognition.writing.rewrite(
                            base.revision_ref,
                            "Rewrite this",
                            sources=[
                                SourceRequest(target=note.revision_ref, usage="current_basis")
                            ],
                            adoption="save",
                            after=AfterWrite(annotation="skip"),
                        )
                request_ref = ResultRef.model_validate(failed.value.details["writing_request_ref"])

            async with open_instance(instance, environment={}) as runtime:
                cognition = await runtime.cognition()
                with patch.object(
                    runtime.generation,
                    "load_language_task",
                    side_effect=AssertionError("provider lookup"),
                ):
                    outcome = await cognition.writing.recover(request_ref)
                    repeated = await cognition.writing.recover(request_ref)
                assert outcome.record is not None and repeated.record is not None
                assert outcome.record.record.revision_ref == repeated.record.record.revision_ref
                assert outcome.draft is not None
                assert outcome.draft.context.base_revision == base.revision_ref
                assert outcome.draft.context.source_bindings[0].owner_revision == note.revision_ref
                assert (
                    await cognition.records.read(outcome.record.record.revision_ref)
                ).body.startswith("# Rewritten")
                assert len(requests) == 1

        asyncio.run(scenario())


def test_audio_analysis_resumes_without_repeating_transcription(tmp_path: Path) -> None:
    if not shutil.which("ffmpeg") or not shutil.which("ffprobe"):
        pytest.skip("Audio preparation needs FFmpeg and ffprobe")
    events: list[str] = []
    structure_ok = False

    class Handler(BaseHTTPRequestHandler):
        def do_POST(self) -> None:
            data = self.rfile.read(int(self.headers["Content-Length"]))
            if self.path.endswith("/audio/transcriptions"):
                events.append("transcription")
                status, value = 200, {"text": "The speaker describes the meeting."}
            else:
                request = json.loads(data)
                structured = "response_format" in request
                events.append("structure" if structured else "description")
                status = 503 if structured and not structure_ok else 200
                content = (
                    json.dumps({"title": "Meeting", "items": []})
                    if structured
                    else "A meeting is described."
                )
                value = (
                    {"error": {"message": "unavailable"}}
                    if status == 503
                    else {
                        "id": "audio-stage",
                        "object": "chat.completion",
                        "created": 1700000000,
                        "model": request["model"],
                        "choices": [
                            {
                                "index": 0,
                                "message": {"role": "assistant", "content": content},
                                "finish_reason": "stop",
                            }
                        ],
                    }
                )
            payload = json.dumps(value).encode()
            self.send_response(status)
            self.send_header("Content-Type", "application/json")
            self.send_header("Content-Length", str(len(payload)))
            self.end_headers()
            self.wfile.write(payload)

        def log_message(self, format: str, *args: object) -> None:
            pass

    audio = tmp_path / "speech.wav"
    with wave.open(str(audio), "wb") as output:
        output.setparams((1, 2, 16000, 0, "NONE", "not compressed"))
        output.writeframes(b"\0\0" * 16000)
    instance = tmp_path / "audio-analysis"
    with model_server(Handler) as server:
        configure_models(instance, server)

        async def configure() -> None:
            async with open_configuration_operations(instance, environment={}) as config:
                await config.update(
                    {
                        "models.asr": {
                            "provider": "local",
                            "model_name": "asr",
                            "capabilities": ["audio"],
                            "input_formats": ["audio/wav"],
                        },
                        "tasks.asr": {"model": "asr", "operation": "transcription"},
                        "tasks.describe": {"model": "reader", "prompt": "describe-media"},
                        "tasks.structure": {
                            "model": "reader",
                            "prompt": "extract-information",
                            "output_mode": "native",
                        },
                        "media.audio.transcription_task": "asr",
                    }
                )

        asyncio.run(configure())

        async def retry_after_busy() -> None:
            from unittest.mock import patch

            from nous.common.errors import NousError
            from nous.runtime.bootstrap import open_instance

            async with open_instance(
                instance, environment={}, overrides={"database.busy_timeout_ms": 30}
            ) as runtime:
                artifact_ref = (await runtime.artifacts.ingest(audio)).ref
                adopt = runtime.results.adopt
                failed = False

                async def busy_adopt(*args: object, **kwargs: object) -> object:
                    nonlocal failed
                    if kwargs.get("operation") == "asr" and not failed:
                        failed = True
                        with sqlite3.connect(runtime.database.path) as connection:
                            connection.execute("BEGIN IMMEDIATE")
                            return await adopt(*args, **kwargs)  # type: ignore[arg-type]
                    return await adopt(*args, **kwargs)  # type: ignore[arg-type]

                with patch.object(runtime.results, "adopt", side_effect=busy_adopt):
                    with pytest.raises(NousError) as error:
                        await runtime.media.analyze(
                            artifact_ref,
                            "description_only",
                            runtime.configuration.snapshot,
                            description_task="describe",
                        )
                    assert error.value.code == "storage.busy"
                    outcome = await runtime.media.analyze(
                        artifact_ref,
                        "description_only",
                        runtime.configuration.snapshot,
                        description_task="describe",
                    )
                assert outcome.batches and events.count("transcription") == 1

        asyncio.run(retry_after_busy())
        events.clear()

        async def accept_for_crash() -> str:
            from nous.runtime.bootstrap import open_instance

            async with open_instance(instance, environment={}) as runtime:
                return (await runtime.artifacts.ingest(audio)).ref.model_dump_json()

        saved_ref = asyncio.run(accept_for_crash())
        crash_program = """import asyncio, os, signal, sqlite3, sys
from unittest.mock import patch
from nous.common.errors import NousError
from nous.content.contracts import ArtifactRef
from nous.runtime.bootstrap import open_instance

async def run():
    async with open_instance(sys.argv[1], environment={}, overrides={"database.busy_timeout_ms": 30}) as runtime:
        original = runtime.results.adopt
        async def busy(*args, **kwargs):
            if kwargs.get("operation") == "asr":
                with sqlite3.connect(runtime.database.path) as connection:
                    connection.execute("BEGIN IMMEDIATE")
                    return await original(*args, **kwargs)
            return await original(*args, **kwargs)
        with patch.object(runtime.results, "adopt", side_effect=busy):
            try:
                await runtime.media.analyze(ArtifactRef.model_validate_json(sys.argv[2]), "description_only", runtime.configuration.snapshot, description_task="describe")
            except NousError as error:
                assert error.code == "storage.busy", error.code
            else:
                raise AssertionError("expected a busy adoption")
        assert len(list((runtime.artifacts.content.root / "pending" / "transcriptions").glob("*.json"))) == 1
        if hasattr(signal, "SIGKILL"):
            os.kill(os.getpid(), signal.SIGKILL)
        os._exit(91)
asyncio.run(run())
"""
        crashed = subprocess.run(
            [sys.executable, "-c", crash_program, str(instance), saved_ref],
            capture_output=True,
            text=True,
            timeout=60,
            check=False,
        )
        assert crashed.returncode == (-signal.SIGKILL if hasattr(signal, "SIGKILL") else 91), (
            crashed.stdout,
            crashed.stderr,
        )
        assert events.count("transcription") == 1

        async def retry_after_crash() -> None:
            from unittest.mock import patch

            from nous.content.contracts import ArtifactRef
            from nous.runtime.bootstrap import open_instance

            async with open_instance(instance, environment={}) as runtime:
                journal = runtime.generation.transcription.journal
                with patch.object(journal, "discard", side_effect=OSError("cleanup unavailable")):
                    outcome = await runtime.media.analyze(
                        ArtifactRef.model_validate_json(saved_ref),
                        "description_only",
                        runtime.configuration.snapshot,
                        description_task="describe",
                    )
                assert outcome.batches
                assert list(journal.root.glob("*.json"))
            async with open_instance(instance, environment={}) as runtime:
                outcome = await runtime.media.analyze(
                    ArtifactRef.model_validate_json(saved_ref),
                    "description_only",
                    runtime.configuration.snapshot,
                    description_task="describe",
                )
                assert outcome.batches
                assert not list(
                    (runtime.artifacts.content.root / "pending" / "transcriptions").glob("*.json")
                )

        asyncio.run(retry_after_crash())
        assert events.count("transcription") == 1
        events.clear()
        cli_program = """from nous.common.errors import NousError
from nous.content.results import ResultStore
original = ResultStore.adopt
async def busy_after_transcription(self, *args, **kwargs):
    if kwargs.get("operation") == "asr":
        raise NousError("storage.busy", "injected database busy")
    return await original(self, *args, **kwargs)
ResultStore.adopt = busy_after_transcription
from nous.transports.cli import main
main()
"""
        failed_cli = subprocess.run(
            [
                sys.executable,
                "-c",
                cli_program,
                "--instance",
                str(instance),
                "--json",
                "tools",
                "media",
                "analyze",
                str(audio),
                "--strategy",
                "description_only",
                "--description-task",
                "describe",
            ],
            env={key: value for key, value in os.environ.items() if not key.startswith("NOUS__")},
            capture_output=True,
            text=True,
            timeout=60,
            check=False,
        )
        assert failed_cli.returncode != 0, (failed_cli.stdout, failed_cli.stderr)
        failure = json.loads(failed_cli.stdout)["error"]
        assert failure["code"] == "storage.busy"
        artifact_for_retry = failure["details"]["artifact"]
        assert events.count("transcription") == 1
        recovered_cli = model_cli(
            instance,
            "tools",
            "media",
            "analyze",
            artifact_for_retry,
            "--strategy",
            "description_only",
            "--description-task",
            "describe",
        )
        assert recovered_cli.returncode == 0, (recovered_cli.stdout, recovered_cli.stderr)
        assert events.count("transcription") == 1
        events.clear()
        added = model_cli(instance, "tools", "content", "add", str(audio))
        assert added.returncode == 0, added.stdout
        artifact = json.loads(added.stdout)["ref"]["identifier"]
        command = [
            "tools",
            "media",
            "analyze",
            artifact,
            "--strategy",
            "describe_then_structure",
            "--description-task",
            "describe",
            "--task",
            "structure",
        ]
        failed = model_cli(instance, *command)
        assert failed.returncode != 0
        error = json.loads(failed.stdout)["error"]
        descriptions = error["details"]["description_result_refs"]
        assert len(descriptions) == 1
        assert events.count("transcription") == 1
        assert events.count("description") == 1
        structure_ok = True
        recovered = model_cli(instance, *command, "--description-result", descriptions[0])
        assert recovered.returncode == 0, (recovered.stdout, recovered.stderr)
    assert events.count("transcription") == 1
    assert events.count("description") == 1
    result = json.loads(recovered.stdout)
    assert result["descriptions"] == descriptions
    assert result["results"][0]["inputs"][0]["parts"][0]["representation"] == "transcript"


def test_transcription_journal_survives_cancellation_and_rejects_corruption(tmp_path: Path) -> None:
    import threading
    from unittest.mock import patch

    from nous.common.errors import NousError
    from nous.models.contracts import GenerationResult
    from nous.models.generation.transcription_journal import TranscriptionJournal
    from nous.runtime.bootstrap import initialize_instance, open_instance

    async def scenario() -> None:
        instance = tmp_path / "transcription-journal"
        await initialize_instance(instance, environment={})
        async with open_instance(instance, environment={}) as runtime:
            journal = TranscriptionJournal(runtime.artifacts.content.root, runtime.execution)
            generated = GenerationResult("transcript", {"kind": "model"}, {}, {"task": "asr"}, 1)
            entered, release = threading.Event(), threading.Event()
            save = journal.files._save  # pyright: ignore[reportPrivateUsage]

            def delayed(key: str, value: dict[str, JsonValue]) -> None:
                entered.set()
                assert release.wait(10)
                save(key, value)

            with patch.object(journal.files, "_save", side_effect=delayed):
                operation = asyncio.create_task(journal.save("transcription:test", generated))
                assert await asyncio.to_thread(entered.wait, 10)
                operation.cancel()
                release.set()
                with pytest.raises(asyncio.CancelledError):
                    await operation
            assert await journal.load("transcription:test") == generated
            stored = next(journal.root.glob("*.json"))
            envelope = json.loads(stored.read_text(encoding="utf-8"))
            envelope["value"]["output"] = "altered"
            stored.write_text(json.dumps(envelope), encoding="utf-8")
            with pytest.raises(NousError) as error:
                await journal.load("transcription:test")
            assert error.value.code == "media.transcription_recovery_invalid"

    asyncio.run(scenario())
