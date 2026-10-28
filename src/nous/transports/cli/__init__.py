"""Online commands use the public client; instance creation is explicitly offline."""

import asyncio
import json
from pathlib import Path
from typing import Annotated, Literal

import typer
import uvicorn
from nous_wave import NousClient
from nous_wave.contracts import AdmitFiles, ImportDocument, QueryRequest
from pydantic import BaseModel
from rich.console import Console

from nous.platform.configuration import Settings
from nous.platform.instance import initialize

cli = typer.Typer(help="Nous Wave cognitive service", no_args_is_help=True)
console = Console()


def client(instance: Path, url: str) -> NousClient:
    return NousClient(url, credential_file=instance / "credentials/admin.token")


def output(value: object) -> None:
    if isinstance(value, BaseModel):
        value = value.model_dump(mode="json")
    console.print_json(json.dumps(value, ensure_ascii=False))


@cli.command()
def init(instance: Path = Path("temp/instances/default")) -> None:
    """Create a fresh instance directory and protected admin credential."""
    path = initialize(instance)
    output({"instance": str(instance.resolve()), "credential_file": str(path.resolve())})


@cli.command()
def serve(
    instance: Path = Path("temp/instances/default"), host: str = "127.0.0.1", port: int = 7441
) -> None:
    """Run one authority and its durable workers."""
    from nous.transports.http import create_app

    settings = Settings(instance=instance, host=host, port=port)
    from nous.platform.observability import configure_logging

    configure_logging(settings.log_level)
    uvicorn.run(create_app(settings), host=host, port=port, log_config=None)


@cli.command()
def status(
    instance: Path = Path("temp/instances/default"), url: str = "http://127.0.0.1:7441"
) -> None:
    async def run() -> None:
        async with client(instance, url) as api:
            output(await api.request("GET", "/api/status"))

    asyncio.run(run())


@cli.command("namespace")
def namespace(
    name: str, instance: Path = Path("temp/instances/default"), url: str = "http://127.0.0.1:7441"
) -> None:
    async def run() -> None:
        async with client(instance, url) as api:
            output(await api.create_namespace(name))

    asyncio.run(run())


@cli.command("subject")
def subject(
    namespace_id: str,
    name: str = "default",
    instance: Path = Path("temp/instances/default"),
    url: str = "http://127.0.0.1:7441",
) -> None:
    async def run() -> None:
        async with client(instance, url) as api:
            output(await api.create_subject(namespace_id, name=name))

    asyncio.run(run())


@cli.command("import")
def import_documents(
    namespace_id: str,
    subject_id: str,
    paths: list[Path],
    llm: bool = True,
    vectors: Literal["disabled", "metadata_allowed", "required"] = "metadata_allowed",
    instance: Path = Path("temp/instances/default"),
    url: str = "http://127.0.0.1:7441",
) -> None:
    """Adopt actual UTF-8 documents, preserving their source identity."""
    documents = tuple(
        ImportDocument(
            text=path.read_text(encoding="utf-8"),
            filename=path.name,
            file_identity=str(path.resolve()),
            source_type="document",
        )
        for path in paths
    )
    command = AdmitFiles(inputs=documents, llm_enabled=llm, vectors=vectors)

    async def run() -> None:
        async with client(instance, url) as api:
            output(await api.admit_documents(namespace_id, subject_id, command))

    asyncio.run(run())


@cli.command("query")
def query(
    namespace_id: str,
    subject_id: str,
    text: str,
    mode: Literal["lexical", "dense", "hybrid", "vcp"] = "lexical",
    limit: int = 10,
    instance: Path = Path("temp/instances/default"),
    url: str = "http://127.0.0.1:7441",
) -> None:
    command = QueryRequest(text=text, mode=mode, limit=limit)

    async def run() -> None:
        async with client(instance, url) as api:
            output(await api.query_cognition(namespace_id, subject_id, command))

    asyncio.run(run())


@cli.command("mcp")
def mcp(
    namespace_id: Annotated[str, typer.Option(envvar="NOUS_NAMESPACE_ID")],
    subject_id: Annotated[str, typer.Option(envvar="NOUS_SUBJECT_ID")],
    instance: Path = Path("temp/instances/default"),
    url: str = "http://127.0.0.1:7441",
    credential_file: Path | None = None,
) -> None:
    """Bind MCP stdio to an authorized namespace and subject."""
    from nous.transports.mcp import run

    run(instance, url, namespace_id, subject_id, credential_file)


@cli.command("backup")
def backup(
    instance: Path = Path("temp/instances/default"), url: str = "http://127.0.0.1:7441"
) -> None:
    """Create an online consistent archive including all retained content."""

    async def run() -> None:
        async with client(instance, url) as api:
            output(await api.request("POST", "/api/backups"))

    asyncio.run(run())


@cli.command("restore")
def restore(archive: Path, instance: Path) -> None:
    """Restore into a new instance without modifying the original."""
    from dataclasses import asdict

    from nous.platform.persistence import BackupService

    async def run() -> None:
        result = await BackupService.restore(archive, instance)
        output(
            {
                key: str(value) if isinstance(value, Path) else value
                for key, value in asdict(result).items()
            }
        )

    asyncio.run(run())


def main() -> None:
    cli()
