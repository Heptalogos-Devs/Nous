"""Foreground annotation can resume the original adoption after model success."""

import asyncio
import json
from http.server import BaseHTTPRequestHandler
from pathlib import Path
from unittest.mock import patch

import pytest
from model_support import configure_models, model_server

from nous.application.configuration import open_configuration_operations
from nous.cognition.concept_contracts import AnnotationView
from nous.common.operations import exception_context
from nous.content.contracts import ResultRef
from nous.runtime.bootstrap import open_instance


def test_foreground_annotation_resumes_adoption_after_reopen(tmp_path: Path) -> None:
    calls: list[object] = []

    class Handler(BaseHTTPRequestHandler):
        def do_POST(self) -> None:
            calls.append(json.loads(self.rfile.read(int(self.headers["Content-Length"]))))
            proposal = {
                "concepts": [
                    {"key": "K", "label": "Meaning", "description": "A meaning", "reuse_key": None}
                ],
                "occurrences": [],
                "paths": [],
            }
            data = json.dumps(
                {
                    "id": "annotation",
                    "object": "chat.completion",
                    "created": 1700000000,
                    "model": "local-model",
                    "choices": [
                        {
                            "index": 0,
                            "message": {"role": "assistant", "content": json.dumps(proposal)},
                            "finish_reason": "stop",
                        }
                    ],
                }
            ).encode()
            self.send_response(200)
            self.send_header("Content-Type", "application/json")
            self.send_header("Content-Length", str(len(data)))
            self.end_headers()
            self.wfile.write(data)

        def log_message(self, format: str, *args: object) -> None:
            pass

    instance = tmp_path / "annotation-recovery"
    with model_server(Handler) as server:
        configure_models(instance, server)

        async def scenario() -> None:
            async with open_configuration_operations(instance, environment={}) as configuration:
                await configuration.update(
                    {
                        "tasks.annotate-concepts": {
                            "model": "reader",
                            "prompt": "annotate-concepts",
                            "output_mode": "native",
                        }
                    }
                )
            async with open_instance(instance, environment={}) as runtime:
                cognition = await runtime.cognition()
                record = (await cognition.records.create_journal("# Title\n\nA body.")).record
                concept = await cognition.concepts.create(None, "Existing", "Existing meaning")
                with patch.object(
                    cognition.annotations, "adopt", side_effect=OSError("injected adoption failure")
                ):
                    with pytest.raises(OSError) as failure:
                        await cognition.annotations.annotate(
                            record.revision_ref, bindings={"K": concept.ref.identifier}
                        )
                request = ResultRef.model_validate(
                    exception_context(failure.value)["annotation_request_ref"]
                )
                assert len(calls) == 1
            async with open_instance(instance, environment={}) as runtime:
                cognition = await runtime.cognition()
                with patch.object(
                    runtime.generation,
                    "load_language_task",
                    side_effect=AssertionError("provider lookup"),
                ):
                    adopted = await cognition.annotations.recover(request)
                    repeated = await cognition.annotations.recover(request)
                assert isinstance(adopted, AnnotationView)
                assert isinstance(repeated, AnnotationView)
                assert adopted.ref == repeated.ref
                assert adopted.bindings["K"].ref == concept.ref
                assert len(calls) == 1

        asyncio.run(scenario())
