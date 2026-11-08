"""Shared local model-protocol test setup."""

import asyncio
import os
import sqlite3
import subprocess
import sys
import threading
from collections.abc import Generator
from contextlib import closing, contextmanager
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path

from nous.application.configuration import open_configuration_operations
from nous.runtime.bootstrap import initialize_instance


@contextmanager
def model_server(handler: type[BaseHTTPRequestHandler]) -> Generator[ThreadingHTTPServer]:
    server = ThreadingHTTPServer(("127.0.0.1", 0), handler)
    thread = threading.Thread(target=server.serve_forever, daemon=True)
    thread.start()
    try:
        yield server
    finally:
        server.shutdown()
        server.server_close()
        thread.join()


def model_cli(instance: Path, *arguments: str) -> subprocess.CompletedProcess[str]:
    return subprocess.run(
        [
            sys.executable,
            "-c",
            "from nous.transports.cli import main; main()",
            "--instance",
            str(instance),
            "--json",
            *arguments,
        ],
        env={key: value for key, value in os.environ.items() if not key.startswith("NOUS__")},
        capture_output=True,
        encoding="utf-8",
        timeout=45,
        check=False,
    )


def configure_models(
    instance: Path, server: ThreadingHTTPServer, output_mode: str = "text"
) -> None:
    async def configure() -> None:
        await initialize_instance(instance, environment={})
        async with open_configuration_operations(instance, environment={}) as configuration:
            await configuration.update(
                {
                    "logging.level": "ERROR",
                    "providers.local": {
                        "protocol": "openai-chat-completions",
                        "endpoint": f"http://127.0.0.1:{server.server_port}/v1",
                    },
                    "models.reader": {
                        "provider": "local",
                        "model_name": "local-model",
                        "capabilities": ["text", "stream", "structured_output"],
                        "output_modes": ["text", "native", "tool"],
                    },
                    "models.vectors": {
                        "provider": "local",
                        "model_name": "local-embedding",
                        "capabilities": ["embedding"],
                    },
                    "tasks.read": {
                        "model": "reader",
                        "prompt": "summarize" if output_mode == "text" else "custom-reading",
                        "output_mode": output_mode,
                    },
                    "model_execution.structured_retries": 1,
                    "embedding.spaces.test-space": {
                        "model": "vectors",
                        "deployment_revision": "test-v1",
                        "dimensions": 2,
                        "modalities": ["text"],
                        "normalization": "none",
                        "encoding": {
                            "document_instruction": "",
                            "query_instruction": "",
                            "preprocessing_revision": "test-v1",
                            "input_combination": "text",
                        },
                    },
                }
            )

    asyncio.run(configure())


def persisted_task_results(instance: Path) -> int:
    with closing(
        sqlite3.connect((instance / "data" / "nous.sqlite3").as_uri() + "?mode=ro", uri=True)
    ) as database:
        return database.execute(
            "select count(*) from processing_results where operation != 'model.language_response'"
        ).fetchone()[0]
