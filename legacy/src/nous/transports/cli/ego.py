"""Free creator documents and readable current personality."""

import asyncio
import mimetypes
from pathlib import Path
from typing import Literal
from uuid import uuid4

import typer
from nous_sdk import NousClient
from nous_sdk.contracts import ImportDocument
from nous_sdk.contracts.ego_model import InterpretSeed, SaveSeed

from .common import invoke


def register(cli: typer.Typer) -> None:
    group = typer.Typer(help="Interpret creator documents and read the current personality.")
    cli.add_typer(group, name="ego")

    async def command(
        api: NousClient,
        namespace_id: str,
        paths: list[Path],
        instruction: str,
        previous_seed_id: str | None,
        operation_id: str | None,
    ) -> InterpretSeed:
        identity = operation_id or str(uuid4())
        documents: list[ImportDocument] = []
        for index, path in enumerate(paths):
            suffix = path.suffix.casefold()
            media_type = mimetypes.guess_type(path.name)[0] or "application/octet-stream"
            file_identity = str(await asyncio.to_thread(path.resolve))
            if suffix == ".pdf" or media_type.startswith(("image/", "audio/", "video/")):
                uploaded = await api.upload_file(
                    namespace_id,
                    path,
                    media_type=media_type,
                    operation_id=f"{identity}:upload:{index}",
                )
                documents.append(
                    ImportDocument(
                        material_id=uploaded.result.id,
                        format="pdf" if suffix == ".pdf" else "media",
                        filename=path.name,
                        file_identity=file_identity,
                        media_type=media_type,
                        source_type="self_seed",
                    )
                )
            else:
                format: Literal["text", "markdown", "html", "json", "jsonl"] = (
                    "markdown"
                    if suffix in {".md", ".markdown"}
                    else "html"
                    if suffix in {".html", ".htm"}
                    else "json"
                    if suffix == ".json"
                    else "jsonl"
                    if suffix == ".jsonl"
                    else "text"
                )
                text = await asyncio.to_thread(path.read_text, encoding="utf-8-sig")
                documents.append(
                    ImportDocument(
                        text=text,
                        format=format,
                        filename=path.name,
                        file_identity=file_identity,
                        source_type="self_seed",
                        media_type=media_type,
                    )
                )
        return InterpretSeed(
            operation_id=identity,
            documents=tuple(documents),
            instruction=instruction,
            previous_seed_id=previous_seed_id,
        )

    @group.command("save")
    def save(
        ctx: typer.Context,
        namespace_id: str,
        paths: list[Path],
        instruction: str = "",
        seed_id: str | None = None,
        expected_version: int = 0,
    ) -> None:
        """Save complete creator documents as an immutable version without model calls."""

        async def run(api: NousClient) -> object:
            documents = []
            for path in paths:
                media_type = mimetypes.guess_type(path.name)[0] or "application/octet-stream"
                if media_type.startswith("text/") or path.suffix.casefold() in {
                    ".md",
                    ".json",
                    ".yaml",
                    ".toml",
                }:
                    documents.append(
                        ImportDocument(
                            text=await asyncio.to_thread(path.read_text, encoding="utf-8-sig"),
                            filename=path.name,
                            source=str(path.resolve()),
                            source_type="self_seed",
                            media_type=media_type,
                        )
                    )
                else:
                    uploaded = await api.upload_file(namespace_id, path, media_type=media_type)
                    documents.append(
                        ImportDocument(
                            material_id=uploaded.result.id,
                            filename=path.name,
                            source=str(path.resolve()),
                            source_type="self_seed",
                            media_type=media_type,
                        )
                    )
            namespace = await api.namespace_by_id(namespace_id)
            return await namespace.seeds.save(
                SaveSeed(
                    documents=tuple(documents),
                    instruction=instruction,
                    seed_id=seed_id,
                    expected_version=expected_version,
                )
            )

        invoke(ctx, run)

    @group.command("version")
    def version(
        ctx: typer.Context, namespace_id: str, seed_id: str, version: int | None = None
    ) -> None:
        """Read a saved version and its exact source references."""

        async def run(api: NousClient) -> object:
            namespace = await api.namespace_by_id(namespace_id)
            return await namespace.seeds.read(seed_id, version=version)

        invoke(ctx, run)

    @group.command("history")
    def history(ctx: typer.Context, namespace_id: str, seed_id: str, limit: int = 32) -> None:
        async def run(api: NousClient) -> object:
            namespace = await api.namespace_by_id(namespace_id)
            return await namespace.seeds.history(seed_id, limit=limit)

        invoke(ctx, run)

    @group.command("interpret")
    def interpret(
        ctx: typer.Context,
        namespace_id: str,
        paths: list[Path],
        instruction: str = "",
        previous_seed_id: str | None = None,
        operation_id: str | None = None,
    ) -> None:
        async def run(api: NousClient) -> object:
            value = await command(
                api, namespace_id, paths, instruction, previous_seed_id, operation_id
            )
            return await api.interpret_seed(namespace_id, value)

        invoke(ctx, run)

    @group.command("import")
    def import_seed(
        ctx: typer.Context,
        namespace_id: str,
        paths: list[Path],
        instruction: str = "",
        previous_seed_id: str | None = None,
        operation_id: str | None = None,
    ) -> None:
        async def run(api: NousClient) -> object:
            value = await command(
                api, namespace_id, paths, instruction, previous_seed_id, operation_id
            )
            return await api.import_seed(namespace_id, value)

        invoke(ctx, run)

    @group.command("read")
    def read(ctx: typer.Context, namespace_id: str, output: Path | None = None) -> None:
        target = output.resolve() if output is not None else None

        async def run(api: NousClient) -> object:
            text = await api.ego_markdown(namespace_id)
            if target is not None:
                await asyncio.to_thread(target.write_text, text, encoding="utf-8")
                return {"path": str(target)}
            return {"markdown": text}

        invoke(ctx, run)
