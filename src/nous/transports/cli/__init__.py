"""Local instance management through the shared application operations."""

import asyncio
import json
import sys
from collections.abc import Awaitable, Mapping
from contextvars import ContextVar
from dataclasses import asdict, dataclass
from io import TextIOWrapper
from pathlib import Path
from typing import Annotated, Literal, cast

import click
import typer
from rich.console import Console
from typer.exceptions import TyperException

from nous.common.errors import NousError, failure_code, failure_message
from nous.common.operations import exception_context
from nous.configuration import Configuration
from nous.configuration.service import Object
from nous.presentation import error_message, format_status, render_error, select_language, translate
from nous.runtime.instance import (
    InstanceLocator,
    inspect_instance,
)

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


def run_async[T](action: Awaitable[T]) -> T:
    """Translate cancellation after owners have released their actual workers and resources."""
    try:
        return asyncio.run(action)
    except (asyncio.CancelledError, KeyboardInterrupt, TimeoutError) as error:
        details = exception_context(error)
        cancelled = isinstance(error, (asyncio.CancelledError, KeyboardInterrupt))
        raise NousError(
            "operation.cancelled" if cancelled else "operation.timeout",
            "Operation cancelled" if cancelled else "Operation exceeded its deadline.",
            details=details,
            operation_id=cast(str | None, details.get("operation_id")),
            trace_id=cast(str | None, details.get("trace_id")),
            cause=error,
        ) from error


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
    from nous.application.configuration import open_configuration_operations

    async with open_configuration_operations(options(ctx).locator) as service:
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

    descriptor = run_async(initialize_instance(options(ctx).locator, layout=layout, name=name))
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
    emit(ctx, run_async(offline_change(ctx, "update", {path: parsed})))


@config_cli.command("unset")
def config_unset(ctx: typer.Context, path: str) -> None:
    """删除本层覆盖并恢复继承 / Restore inheritance."""
    emit(ctx, run_async(offline_change(ctx, "unset", path)))


@config_cli.command("remove-resource")
def config_remove_resource(ctx: typer.Context, path: str) -> None:
    """检查引用后删除资源 / Remove an unreferenced resource."""
    emit(ctx, run_async(offline_change(ctx, "remove_resource", path)))


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
def runtime_check(
    ctx: typer.Context, details: Annotated[bool, typer.Option("--details")] = False
) -> None:
    """打开资源并报告本地能力 / Inspect actual local owners."""
    from nous.runtime.bootstrap import open_instance

    async def run() -> Object:
        async with open_instance(options(ctx).locator) as runtime:
            result = await runtime.check()
        result["state"] = runtime.state
        result["execution"] = runtime.execution.status()
        result["in_flight"] = runtime.execution.in_flight
        return result

    result = run_async(run())
    if options(ctx).json_mode or details:
        emit(ctx, result)
        return
    console = Console()
    console.print(f"{result['name']} — {result['state']}", markup=False)
    for key in (
        "configuration",
        "database",
        "models",
        "media",
        "embedding",
        "wave",
        "execution",
        "observability",
    ):
        value = cast(dict[str, object], result[key])
        if key == "configuration":
            summary = f"saved={value['version']}; effective={value['effective_version']}; {value['state']}; pending restart={value['pending_restart']}"
        elif key == "database":
            summary = f"schema {value['schema_version']}; {value['journal_mode']}; foreign keys={value['foreign_keys']}"
        elif key == "embedding":
            spaces = cast(list[dict[str, object]], value["spaces"])
            counts = cast(dict[str, int], value["adopted_rows"])
            summary = (
                "; ".join(
                    f"{space['space_id']}: {cast(dict[str, object], space['definition'])['dimensions']} dimensions, {counts.get(str(space['space_id']), 0)} adopted rows"
                    for space in spaces
                )
                or "no bound spaces"
            )
        elif key == "execution":
            summary = f"{value['root_operations']} root operations; {value['queued']} queued; {value['running']} running; {value['blocking_workers']} blocking workers"
        elif key == "models":
            summary = (
                f"{len(cast(list[object], value['resources']))} configured resources; tasks: "
                + ", ".join(cast(dict[str, object], value.get("tasks", {})))
            )
        elif key == "wave":
            summary = f"{value['snapshots']} snapshots; Native " + (
                "loaded" if value["native_loaded"] else "not loaded"
            )
        elif key == "observability":
            summary = f"logs={value['logs']}; spans=" + (
                "not persisted locally; export=" + str(value["trace_export"])
            )
        elif key == "media":
            summary = ", ".join(cast(list[str], value.get("image_formats", [])))
            for name, item in cast(
                dict[str, dict[str, object]], value.get("executables", {})
            ).items():
                summary += f"; {name}=" + (
                    str(item["path"]) if item["available"] else "unavailable"
                )
        else:
            summary = json.dumps(value, ensure_ascii=False, separators=(",", ":"))
        console.print(f"{key}: {summary}", markup=False)
    recent = cast(list[dict[str, object]], result["recent_results"])
    if recent:
        console.print(
            "Recent adopted results: "
            + ", ".join(f"{item['operation']} {item['address']}" for item in recent),
            markup=False,
        )


@runtime_cli.command("logs")
def runtime_logs(
    ctx: typer.Context, operation_id: Annotated[str, typer.Option("--operation")]
) -> None:
    """查看一次操作的保留日志 / Read retained operation diagnostics."""
    from nous.observability.diagnostics import operation_logs

    state = inspect_instance(options(ctx).locator)
    paths = cast(dict[str, str], state["paths"])
    result = operation_logs(Path(paths["logs"]), operation_id)
    if options(ctx).json_mode:
        emit(ctx, result)
        return
    typer.echo(f"{result['operation']} — {result['result']} ({result['operation_id']})")
    for event in cast(list[dict[str, object]], result["events"]):
        if event.get("event") == "request.attempt":
            typer.echo(
                f"  {event.get('stage', '')}: attempt {event.get('number')} {event.get('route')} → {event.get('outcome')} {event.get('error_code', '')} {event.get('duration_seconds', '')}s"
            )
        elif event.get("event") in {"operation.failed", "stage.completed", "operation.completed"}:
            typer.echo(
                f"  {event.get('stage', '')}: {event.get('result')} {event.get('error_message', '')} {event.get('duration_seconds', '')}s"
            )


@time_cli.command("parse")
def time_parse(
    ctx: typer.Context,
    value: str,
    domain: Annotated[str, typer.Option(help="时间域 ID / Time domain ID")] = "utc",
    timezone: Annotated[str | None, typer.Option(help="IANA 时区 / IANA timezone")] = None,
    fold: Annotated[int | None, typer.Option(help="DST 重复时刻 / DST fold: 0 or 1")] = None,
) -> None:
    """保留原文、精度和不确定性 / Preserve precision and uncertainty."""
    from nous.temporal.values import parse_explicit_time

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


from .tools import tools_cli  # noqa: E402

cli.add_typer(tools_cli, name="tools")


def main() -> None:
    _current.set(None)
    for stream in (sys.stdout, sys.stderr):
        if isinstance(stream, TextIOWrapper):
            stream.reconfigure(encoding="utf-8")
    try:
        cli(standalone_mode=False)
    except (Exception, KeyboardInterrupt) as error:
        context = exception_context(error)
        failure = (
            error
            if isinstance(error, NousError)
            else NousError(
                "operation.cancelled"
                if isinstance(error, (typer.Abort, KeyboardInterrupt))
                else "input.invalid"
                if isinstance(error, (click.ClickException, TyperException))
                else failure_code(error),
                (
                    failure_message(error)
                    or (
                        "Operation exceeded its deadline."
                        if isinstance(error, TimeoutError)
                        else ""
                    )
                )
                if isinstance(
                    error,
                    (
                        ValueError,
                        OSError,
                        click.ClickException,
                        TyperException,
                        typer.Abort,
                        KeyboardInterrupt,
                    ),
                )
                else "Unexpected error",
                details=context,
                operation_id=cast(str | None, context.get("operation_id")),
                trace_id=cast(str | None, context.get("trace_id")),
                cause=error,
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
