"""Command groups shared by the CLI composition root and command modules."""

import typer

cli = typer.Typer(help="Nous 本地实例管理 / Local instance management", no_args_is_help=True)
config_cli = typer.Typer(help="读取、解释和修改配置 / Configuration", no_args_is_help=True)
instance_cli = typer.Typer(help="查看实例 / Inspect instance", no_args_is_help=True)
runtime_cli = typer.Typer(help="运行诊断 / Runtime diagnostics", no_args_is_help=True)
time_cli = typer.Typer(help="解析显式时间 / Explicit time", no_args_is_help=True)
catalog_cli = typer.Typer(help="查看可用操作 / Application operations", no_args_is_help=True)
for command, name in (
    (config_cli, "config"),
    (instance_cli, "instance"),
    (runtime_cli, "runtime"),
    (time_cli, "time"),
    (catalog_cli, "operations"),
):
    cli.add_typer(command, name=name)
