"""A recovered writing request and its save share bounded admission safely."""

import asyncio
import json
from http.server import BaseHTTPRequestHandler
from pathlib import Path
from unittest.mock import patch

import pytest
from model_support import configure_models, model_server

from nous.application.configuration import open_configuration_operations
from nous.cognition.writing.contracts import AfterWrite, DraftContext, WritingOutcome
from nous.cognition.writing.drafts import DraftStore
from nous.common.errors import NousError
from nous.content.contracts import ResultRef
from nous.models.generation import ProducedTask
from nous.runtime.bootstrap import open_instance


def test_recovery_and_direct_save_do_not_invert_single_admission(tmp_path: Path) -> None:
    requests: list[bytes] = []

    class Handler(BaseHTTPRequestHandler):
        def do_POST(self) -> None:
            requests.append(self.rfile.read(int(self.headers["Content-Length"])))
            payload = json.dumps(
                {
                    "id": "writing-admission",
                    "object": "chat.completion",
                    "created": 1700000000,
                    "model": "local-model",
                    "choices": [
                        {
                            "index": 0,
                            "message": {"role": "assistant", "content": "Recovered draft."},
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

    captured: list[tuple[ProducedTask, DraftContext, ResultRef]] = []

    async def interrupt_save(
        self: DraftStore, produced: ProducedTask, context: DraftContext, ref: ResultRef
    ) -> object:
        captured.append((produced, context, ref))
        raise NousError("test.interrupt", "Interrupt after generation")

    instance = tmp_path / "instance"
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
                base = (await cognition.records.create_journal("Original body")).record
                with patch.object(DraftStore, "save_produced", new=interrupt_save):
                    with pytest.raises(NousError, match="Interrupt after generation"):
                        await cognition.writing.rewrite(
                            base.revision_ref,
                            "Rewrite this",
                            adoption="draft",
                            after=AfterWrite(annotation="skip"),
                        )
            assert len(captured) == 1 and len(requests) == 1
            produced, context, request_ref = captured[0]
            async with open_instance(
                instance,
                environment={},
                overrides={"execution.max_in_flight": 1, "execution.default_timeout_seconds": None},
            ) as runtime:
                cognition = await runtime.cognition()
                admitted, release = asyncio.Event(), asyncio.Event()

                async def nested_recovery() -> WritingOutcome:
                    async with runtime.execution.operation("composed.recovery"):
                        admitted.set()
                        await release.wait()
                        return await cognition.writing.recover(request_ref)

                parent = asyncio.create_task(nested_recovery())
                direct = None
                try:
                    await asyncio.wait_for(admitted.wait(), 5)
                    direct = asyncio.create_task(
                        cognition.writing.drafts.save_produced(produced, context, request_ref)
                    )
                    for _ in range(400):
                        if runtime.execution.status()["queued"] == 1:
                            break
                        await asyncio.sleep(0.005)
                    assert runtime.execution.status()["queued"] == 1
                    release.set()
                    first, second = await asyncio.wait_for(asyncio.gather(parent, direct), 8)
                    assert first.draft_ref == second.draft_ref
                    assert first.draft_ref is not None
                    assert len(requests) == 1
                finally:
                    release.set()
                    for task in (parent, direct):
                        if task is not None and not task.done():
                            task.cancel()
                    await asyncio.gather(
                        *(task for task in (parent, direct) if task is not None),
                        return_exceptions=True,
                    )

        asyncio.run(scenario())
