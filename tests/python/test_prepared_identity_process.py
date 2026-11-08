"""A saved prepared identity survives a real application process restart."""

import json
import subprocess
import sys
import time
from http.server import BaseHTTPRequestHandler
from pathlib import Path

from model_support import configure_models, model_server


def test_prepared_identity_survives_independent_process_restart(tmp_path: Path) -> None:
    requests: list[bytes] = []

    class Handler(BaseHTTPRequestHandler):
        def do_POST(self) -> None:
            requests.append(self.rfile.read(int(self.headers["Content-Length"])))
            payload = json.dumps(
                {
                    "id": "restart-test",
                    "object": "chat.completion",
                    "created": 1700000000,
                    "model": "local-model",
                    "choices": [
                        {
                            "index": 0,
                            "message": {"role": "assistant", "content": "Recovered output."},
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

    instance = tmp_path / "instance"
    first_program = """\
import asyncio, json, sys
from pathlib import Path
from unittest.mock import patch
from nous.runtime.bootstrap import open_instance
async def main():
    async with open_instance(Path(sys.argv[1]), environment={}) as runtime:
        source = await runtime.artifacts.ingest('Real process restart source')
        await runtime.configuration.update({'logging.level': 'INFO'})
        snapshot = runtime.configuration.snapshot
        selected = await runtime.generation.accept_inputs(source, snapshot)
        plan = await runtime.generation.prepare_input('read', selected, snapshot)
        prepared = await runtime.generation.prepare_group(plan.frozen, plan.batches[0], snapshot)
        await runtime.generation.produce_prepared(prepared, producer_key='accepted-process')
        original = runtime.results.prepare_outputs
        async def fail(outputs):
            if {'response', 'computation'} <= outputs.keys(): raise OSError('injected failure')
            return await original(outputs)
        with patch.object(runtime.results, 'prepare_outputs', side_effect=fail):
            try: await runtime.generation.produce_prepared(prepared, producer_key='pending-process')
            except OSError: pass
            else: raise AssertionError('expected publication failure')
        marker = Path(sys.argv[2])
        temporary = marker.with_suffix('.tmp')
        temporary.write_text(json.dumps({'artifact': source.ref.identifier, 'version': snapshot.version}))
        temporary.replace(marker)
        await asyncio.Event().wait()
asyncio.run(main())
"""
    second_program = """\
import asyncio, json, sys
from pathlib import Path
from nous.runtime.bootstrap import open_instance
async def main():
    async with open_instance(Path(sys.argv[1]), environment={}) as runtime:
        snapshot = runtime.configuration.snapshot
        source = await runtime.artifacts.get(sys.argv[2])
        selected = await runtime.generation.accept_inputs(source, snapshot)
        plan = await runtime.generation.prepare_input('read', selected, snapshot)
        prepared = await runtime.generation.prepare_group(plan.frozen, plan.batches[0], snapshot)
        accepted = await runtime.generation.produce_prepared(prepared, producer_key='accepted-process')
        pending = await runtime.generation.produce_prepared(prepared, producer_key='pending-process')
        assert accepted.outputs['output'].data == pending.outputs['output'].data == b'Recovered output.'
        print(json.dumps({'version': snapshot.version, 'recovered': True}), flush=True)
asyncio.run(main())
"""
    with model_server(Handler) as server:
        configure_models(instance, server)
        marker = tmp_path / "ready.json"
        first = subprocess.Popen(
            [sys.executable, "-c", first_program, str(instance), str(marker)],
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            text=True,
        )
        try:
            deadline = time.monotonic() + 45
            while not marker.exists():
                if first.poll() is not None or time.monotonic() >= deadline:
                    first.kill()
                    stdout, stderr = first.communicate(timeout=10)
                    raise AssertionError((first.returncode, stdout, stderr))
                time.sleep(0.05)
            first_result = json.loads(marker.read_text())
            assert first_result["version"] == 2
            assert len(requests) == 2
            first.kill()
            first.communicate(timeout=10)
            assert first.returncode != 0
        finally:
            if first.poll() is None:
                first.kill()
                first.communicate(timeout=10)
        second = subprocess.run(
            [sys.executable, "-c", second_program, str(instance), first_result["artifact"]],
            capture_output=True,
            text=True,
            timeout=45,
            check=False,
        )
        assert second.returncode == 0, (second.stdout, second.stderr)
        assert json.loads(second.stdout) == {"version": 1, "recovered": True}
        assert len(requests) == 2
