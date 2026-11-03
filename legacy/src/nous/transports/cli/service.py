"""Explicit per-user service lifecycle commands."""

import typer

from nous.platform.instance.service import UserService

from .common import emit, options


def register(cli: typer.Typer) -> None:
    group = typer.Typer(
        help="Explicit native user-service lifecycle; foreground serve remains available.",
        no_args_is_help=False,
    )
    cli.add_typer(group, name="service")

    @group.command("install")
    def install(
        ctx: typer.Context, autostart: bool = False, host: str = "127.0.0.1", port: int = 7441
    ) -> None:
        emit(
            ctx,
            UserService(options(ctx).instance, host=host, port=port).install(autostart=autostart),
        )

    @group.command("uninstall")
    def uninstall(ctx: typer.Context) -> None:
        emit(ctx, UserService(options(ctx).instance).uninstall())

    @group.command("start")
    def start(ctx: typer.Context) -> None:
        emit(ctx, UserService(options(ctx).instance).start())

    @group.command("stop")
    def stop(ctx: typer.Context) -> None:
        emit(ctx, UserService(options(ctx).instance).stop())

    @group.command("status")
    def status(ctx: typer.Context) -> None:
        emit(ctx, UserService(options(ctx).instance).status())
