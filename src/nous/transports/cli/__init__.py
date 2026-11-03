"""Local instance management through the shared application operations."""

import asyncio
import json
import sys
from collections.abc import Mapping
from contextvars import ContextVar
from dataclasses import asdict, dataclass, is_dataclass
from io import TextIOWrapper
from pathlib import Path
from typing import Annotated, Literal, cast

import click
import typer
from rich.console import Console

from nous.configuration import ConfigSnapshot, Configuration
from nous.configuration.service import Object
from nous.presentation import error_message, format_status, render_error, select_language, translate
from nous.runtime.errors import NousError
from nous.runtime.instance import (
    InstanceDescriptor,
    InstanceLocator,
    InstanceLock,
    inspect_instance,
)
from nous.runtime.paths import ResolvedPaths

cli = typer.Typer(help="Nous 本地实例管理 / Local instance management", no_args_is_help=True)
config_cli = typer.Typer(help="读取、解释和修改配置 / Configuration", no_args_is_help=True)
instance_cli = typer.Typer(help="查看实例 / Inspect instance", no_args_is_help=True)
runtime_cli = typer.Typer(help="运行诊断 / Runtime diagnostics", no_args_is_help=True)
time_cli = typer.Typer(help="解析显式时间 / Explicit time", no_args_is_help=True)
for command, name in (
    (config_cli, "config"),
    (instance_cli, "instance"),
    (runtime_cli, "runtime"),
    (time_cli, "time"),
):
    cli.add_typer(command, name=name)


@dataclass(frozen=True)
class Options:
    locator: InstanceLocator
    json_mode: bool = False
    language: str | None = None
    debug: bool = False


_current: ContextVar[Options | None] = ContextVar("local_cli_options", default=None)


@cli.callback()
def configure(
    ctx: typer.Context,
    instance: Annotated[
        Path | None,
        typer.Option(
            help="实例配置目录 / Instance configuration directory", envvar="NOUS_INSTANCE"
        ),
    ] = None,
    json_mode: Annotated[
        bool, typer.Option("--json", help="输出类型化 JSON / Typed JSON output")
    ] = False,
    language: Annotated[str | None, typer.Option(help="显示语言 / Language: zh-CN or en")] = None,
    debug: Annotated[bool, typer.Option(help="显示错误堆栈 / Show error traceback")] = False,
) -> None:
    if language not in {None, "zh-CN", "en"}:
        raise NousError("configuration.invalid_language", "Language must be zh-CN or en")
    ctx.obj = Options(InstanceLocator.resolve(instance), json_mode, language, debug)
    _current.set(ctx.obj)


def options(ctx: typer.Context) -> Options:
    return cast(Options, ctx.obj)


def language_for(selected: Options | None) -> str:
    configured: str | None = None
    if selected is not None and selected.language is None:
        try:
            configured = (
                Configuration(selected.locator.config_file)
                .snapshot.typed_values()
                .localization.language
            )
        except NousError, OSError:
            pass
    return select_language(selected.language if selected else None, configured)


def emit(ctx: typer.Context, value: Mapping[str, object]) -> None:
    if options(ctx).json_mode:
        typer.echo(json.dumps(value, ensure_ascii=False, separators=(",", ":")))
    else:
        typer.echo(format_status(value, language_for(options(ctx))))


def configuration(ctx: typer.Context) -> Configuration:
    return Configuration(options(ctx).locator.config_file)


async def offline_change(
    ctx: typer.Context,
    operation: Literal["update", "unset", "remove_resource"],
    argument: Mapping[str, object] | str,
) -> Object:
    """Ordinary edits need the instance lock and remain possible with an unreadable DB."""
    from nous.runtime.bindings import InstanceBinding
    from nous.storage.database import Database
    from nous.temporal import check_domains

    selected = options(ctx).locator
    with InstanceLock(selected.directory):
        service = Configuration(selected.config_file)
        settings = service.snapshot.typed_values()
        descriptor = InstanceDescriptor(
            settings.instance.id, settings.instance.name, settings.instance.layout
        )
        paths = ResolvedPaths.resolve(selected, descriptor, settings.paths)
        binding = InstanceBinding.read(selected)
        binding.check_locations(selected, descriptor, paths, settings.database.filename)

        async def binding_check(
            old: ConfigSnapshot, candidate: ConfigSnapshot, changed: tuple[str, ...]
        ) -> None:
            desired = candidate.typed_values()
            candidate_descriptor = InstanceDescriptor(
                desired.instance.id, desired.instance.name, desired.instance.layout
            )
            candidate_paths = ResolvedPaths.resolve(selected, candidate_descriptor, desired.paths)
            binding.check_locations(
                selected, candidate_descriptor, candidate_paths, desired.database.filename
            )
            if any(path.startswith("time.domains.") for path in changed):
                try:
                    with InstanceLock(paths.data, data=True):
                        binding.check_resources(paths)
                        async with Database.open(
                            paths.resolve_owned("data", settings.database.filename),
                            settings.database,
                            descriptor.instance_id,
                            database_id=str(binding.database_id),
                        ) as database:
                            await check_domains(database, candidate.typed_values().time.domains)
                except NousError:
                    raise
                except Exception as error:
                    raise NousError(
                        "configuration.binding_check_unavailable",
                        "The database could not be opened to check time domain bindings.",
                        cause=error,
                    ) from error

        service.binding_check = binding_check
        if operation == "update":
            assert isinstance(argument, Mapping)
            return await service.update(argument)
        assert isinstance(argument, str)
        if operation == "unset":
            return await service.unset(argument)
        return await service.remove_resource(argument)


@cli.command("init")
def initialize(
    ctx: typer.Context,
    layout: Annotated[str, typer.Option(help="布局 / Layout: portable or platform")] = "portable",
    name: Annotated[str, typer.Option(help="实例名称 / Instance name")] = "default",
) -> None:
    """创建实例及基础数据库 / Initialize instance and database."""
    from nous.runtime.bootstrap import initialize_instance

    descriptor = asyncio.run(initialize_instance(options(ctx).locator, layout=layout, name=name))
    emit(
        ctx,
        {
            "state": "initialized",
            "instance": asdict(descriptor),
            "config_file": str(options(ctx).locator.config_file),
        },
    )


@instance_cli.command("show")
def instance_show(ctx: typer.Context) -> None:
    """显示路径和占有状态 / Show paths and ownership."""
    emit(ctx, inspect_instance(options(ctx).locator))


@config_cli.command("list")
def config_list(
    ctx: typer.Context,
    level: Annotated[
        str, typer.Option(help="standard / advanced / developer / internal")
    ] = "standard",
) -> None:
    """列出类型、默认、作用域与生效方式 / List configuration fields."""
    fields = configuration(ctx).catalog(level)
    if options(ctx).json_mode:
        emit(ctx, {"fields": fields})
        return
    language = language_for(options(ctx))
    console = Console()
    for field in fields:
        console.print(str(field["path"]), style="bold", markup=False)
        console.print(
            f"  {translate('Default', language)}: "
            f"{json.dumps(field.get('default'), ensure_ascii=False)}  "
            f"{translate('Scopes', language)}: {', '.join(cast(list[str], field['scopes']))}",
            markup=False,
        )
        console.print(
            f"  {translate('Apply / Impact', language)}: "
            f"{translate(str(field['apply_mode']), language)} / {field['impact']}",
            markup=False,
        )
        description = translate(str(field["description"]), language)
        if description:
            console.print(f"  {description}", markup=False)


@config_cli.command("show")
def config_show(
    ctx: typer.Context,
    effective: Annotated[bool, typer.Option(help="下次启动有效值 / Prospective values")] = False,
    saved: Annotated[bool, typer.Option(help="文件保存值 / Saved file values")] = False,
) -> None:
    """显示保存值或下次启动值 / Show saved or prospective values."""
    if saved and effective:
        raise NousError("configuration.invalid_option", "Choose either --saved or --effective")
    service = configuration(ctx)
    emit(
        ctx,
        {
            "state": "saved" if saved else "prospective",
            "file": str(service.config_file),
            "values": service.saved() if saved else service.snapshot.to_dict(),
            "version": service.snapshot.version,
        },
    )


@config_cli.command("explain")
def config_explain(ctx: typer.Context, path: str) -> None:
    """解释来源、覆盖和生效状态 / Explain value sources and application."""
    emit(ctx, configuration(ctx).explain(path))


@config_cli.command("set")
def config_set(
    ctx: typer.Context,
    path: str,
    value: Annotated[str | None, typer.Argument()] = None,
    null: Annotated[
        bool, typer.Option("--null", help="显式清空 nullable 字段 / Explicit null")
    ] = False,
    value_file: Annotated[Path | None, typer.Option(help="JSON 值文件 / JSON value file")] = None,
) -> None:
    """保存字段值 / Save a configuration value."""
    if sum((value is not None, null, value_file is not None)) != 1:
        raise NousError("configuration.value_required", "Provide a value, --null, or --value-file")
    parsed: object
    if value_file is not None:
        parsed = json.loads(value_file.read_text(encoding="utf-8"))
    elif null:
        parsed = None
    else:
        assert value is not None
        try:
            parsed = json.loads(value)
        except ValueError:
            parsed = value
    emit(ctx, asyncio.run(offline_change(ctx, "update", {path: parsed})))


@config_cli.command("unset")
def config_unset(ctx: typer.Context, path: str) -> None:
    """删除本层覆盖并恢复继承 / Restore inheritance."""
    emit(ctx, asyncio.run(offline_change(ctx, "unset", path)))


@config_cli.command("remove-resource")
def config_remove_resource(ctx: typer.Context, path: str) -> None:
    """检查引用后删除资源 / Remove an unreferenced resource."""
    emit(ctx, asyncio.run(offline_change(ctx, "remove_resource", path)))


@config_cli.command("validate")
def config_validate(ctx: typer.Context) -> None:
    """校验语法、字段及资源引用 / Validate syntax, fields and references."""
    emit(ctx, configuration(ctx).validate())


@config_cli.command("schema")
def config_schema(
    ctx: typer.Context,
    output: Annotated[Path | None, typer.Option(help="保存 JSON Schema / Save JSON Schema")] = None,
) -> None:
    """导出正式字段 Schema / Export configuration JSON Schema."""
    schema = Configuration.schema()
    if output is None:
        emit(ctx, schema)
    else:
        output.parent.mkdir(parents=True, exist_ok=True)
        output.write_text(json.dumps(schema, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")
        emit(ctx, {"file": str(output), "state": "saved"})


@runtime_cli.command("check")
def runtime_check(ctx: typer.Context) -> None:
    """实际打开并关闭运行资源 / Open, inspect and close runtime resources."""
    from nous.observability import operation
    from nous.runtime.bootstrap import open_instance

    async def run() -> Object:
        async with open_instance(options(ctx).locator) as runtime:
            with operation("runtime.check"):
                database = await runtime.database.check()
                result = runtime.status()
                result["database"] = asdict(database) if is_dataclass(database) else database
                settings = runtime.configuration.snapshot.typed_values()
                result["bindings"] = {
                    "time": "checked",
                    "embedding": "unavailable" if settings.embedding.spaces else "unconfigured",
                }
                result["observability"] = {
                    "trace_export": runtime.configuration.snapshot.typed_values().tracing.export,
                    "metrics_export": runtime.configuration.snapshot.typed_values().metrics.export,
                }
        result["state"] = runtime.state
        return result

    emit(ctx, asyncio.run(run()))


@time_cli.command("parse")
def time_parse(
    ctx: typer.Context,
    value: str,
    domain: Annotated[str, typer.Option(help="时间域 ID / Time domain ID")] = "utc",
    timezone: Annotated[str | None, typer.Option(help="IANA 时区 / IANA timezone")] = None,
    fold: Annotated[int | None, typer.Option(help="DST 重复时刻 / DST fold: 0 or 1")] = None,
) -> None:
    """保留原文、精度和不确定性 / Preserve precision and uncertainty."""
    from nous.temporal import parse_explicit_time

    service = configuration(ctx)
    if domain not in service.snapshot.typed_values().time.domains:
        raise NousError("time.domain_unknown", f"Unknown time domain: {domain}")
    zone = timezone or service.snapshot.typed_values().localization.timezone
    parsed = parse_explicit_time(value, domain=domain, timezone=zone, fold=fold)
    emit(
        ctx,
        {
            "input": value,
            "domain": domain,
            "timezone": zone,
            "value": parsed.model_dump(mode="json"),
        },
    )


def main() -> None:
    _current.set(None)
    for stream in (sys.stdout, sys.stderr):
        if isinstance(stream, TextIOWrapper):
            stream.reconfigure(encoding="utf-8")
    try:
        cli(standalone_mode=False)
    except (
        NousError,
        ValueError,
        OSError,
        click.ClickException,
        typer.Abort,
        KeyboardInterrupt,
    ) as error:
        failure = (
            error
            if isinstance(error, NousError)
            else NousError(
                "operation.cancelled"
                if isinstance(error, (typer.Abort, KeyboardInterrupt))
                else "input.invalid"
                if isinstance(error, (ValueError, click.ClickException))
                else "operation.failed",
                str(error),
            )
        )
        result = failure.to_dict()
        selected = _current.get()
        if selected and selected.debug:
            Console(stderr=True).print_exception(show_locals=False)
        language = language_for(selected)
        result["message"] = error_message(failure, language)
        if (selected and selected.json_mode) or (selected is None and "--json" in sys.argv):
            typer.echo(json.dumps({"error": result}, ensure_ascii=False))
        else:
            Console(stderr=True).print(render_error(failure, language), markup=False)
        code = failure.code
        status = (
            130
            if code == "operation.cancelled"
            else 4
            if code
            in {
                "configuration.save_failed",
                "configuration.apply_failed",
                "configuration.read_failed",
            }
            else 3
            if any(
                word in code
                for word in ("unavailable", "not_found", "in_use", "unreadable", "not_initialized")
            )
            else 2
            if code.startswith(("configuration.", "input.", "time."))
            else 4
        )
        raise SystemExit(status) from None
