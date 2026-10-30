"""Exact media lineage, transformations and selections through the public SDK."""

from pathlib import Path

import typer
from nous_sdk import NousClient
from nous_sdk.contracts import DomainError
from nous_sdk.contracts.context import MediaTarget
from nous_sdk.contracts.derivation import (
    CreateDerivedRegion,
    CreateRepresentation,
    DeriveMedia,
    RepresentationRef,
)

from .common import invoke, read_command


def register(cli: typer.Typer) -> None:
    group = typer.Typer(help="Read and transform exact authorized media.", no_args_is_help=True)
    cli.add_typer(group, name="media")

    @group.command("lineage")
    def lineage(ctx: typer.Context, namespace_id: str, subject_id: str, reference: str) -> None:
        async def run(api: NousClient):
            target = (await api.resolve_reference(subject_id, reference)).reference
            if not isinstance(target, MediaTarget):
                raise DomainError("media_required", "Lineage requires a media address")
            return await api.media_lineage(namespace_id, subject_id, target.reference)

        invoke(ctx, run)

    @group.command("derive")
    def derive(ctx: typer.Context, namespace_id: str, subject_id: str, command: Path) -> None:
        value = read_command(command, DeriveMedia)
        invoke(ctx, lambda api: api.derive_media(namespace_id, subject_id, value))

    @group.command("adopt-representation")
    def adopt(ctx: typer.Context, namespace_id: str, subject_id: str, command: Path) -> None:
        value = read_command(command, CreateRepresentation)
        invoke(ctx, lambda api: api.adopt_representation(namespace_id, subject_id, value))

    @group.command("representation")
    def representation(
        ctx: typer.Context,
        namespace_id: str,
        subject_id: str,
        representation_id: str,
        revision: int,
    ) -> None:
        invoke(
            ctx,
            lambda api: api.representation(
                namespace_id, subject_id, RepresentationRef(id=representation_id, revision=revision)
            ),
        )

    @group.command("history")
    def history(
        ctx: typer.Context, namespace_id: str, subject_id: str, representation_id: str
    ) -> None:
        invoke(
            ctx, lambda api: api.representation_history(namespace_id, subject_id, representation_id)
        )

    @group.command("select")
    def select_region(
        ctx: typer.Context, namespace_id: str, subject_id: str, command: Path
    ) -> None:
        value = read_command(command, CreateDerivedRegion)
        invoke(ctx, lambda api: api.create_derived_region(namespace_id, subject_id, value))

    @group.command("download")
    def download(
        ctx: typer.Context, namespace_id: str, subject_id: str, reference: str, destination: Path
    ) -> None:
        async def run(api: NousClient):
            target = (await api.resolve_reference(subject_id, reference)).reference
            if not isinstance(target, MediaTarget):
                raise DomainError("media_required", "Download requires a media address")
            source = target.reference
            if source.kind == "material":
                return await api.download_material(
                    source.id, destination, namespace_id=namespace_id, subject_id=subject_id
                )
            if source.kind == "source_region":
                return await api.download_region(namespace_id, subject_id, source.id, destination)
            return await api.download_derived(namespace_id, subject_id, source, destination)

        invoke(ctx, run)
