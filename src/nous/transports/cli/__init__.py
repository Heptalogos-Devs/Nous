"""Local instance management through the shared application operations."""

import json
import sys
from dataclasses import asdict
from io import TextIOWrapper
from pathlib import Path
from typing import Annotated, cast

import click
import typer
from pydantic import JsonValue
from rich.console import Console
from typer.exceptions import TyperException

from nous.application.config_operations import (
    EXPLAIN_CONFIGURATION,
    LIST_CONFIGURATION,
    REMOVE_CONFIGURATION_RESOURCE,
    SET_CONFIGURATION,
    SHOW_CONFIGURATION,
    UNSET_CONFIGURATION,
    VALIDATE_CONFIGURATION,
    ConfigurationPath,
    EmptyRequest,
    ListConfiguration,
    SetConfiguration,
    ShowConfiguration,
)
from nous.application.inputs import FileInput
from nous.application.time_operations import PARSE_TIME, ParseTime
from nous.common.errors import NousError, failure_code, failure_message
from nous.common.operations import exception_context
from nous.configuration import Configuration
from nous.content.contracts import ExternalInput
from nous.presentation import error_message, render_error, translate
from nous.presentation.json_selection import validate_json_pointers
from nous.runtime.instance import (
    InstanceLocator,
    InstanceProfiles,
    inspect_instance,
)

from .context import (
    Options,
    application_call,
    application_catalog,
    current_options,
    emit,
    language_for,
    options,
    run_async,
)
from .groups import catalog_cli, cli, config_cli, instance_cli, runtime_cli, time_cli


@cli.callback()
def configure(
    ctx: typer.Context,
    instance: Annotated[
        Path | None,
        typer.Option(help="实例配置目录 / Instance configuration directory (NOUS_INSTANCE)"),
    ] = None,
    profile: Annotated[
        str | None, typer.Option(help="已登记实例名称 / Registered instance profile")
    ] = None,
    json_mode: Annotated[
        bool, typer.Option("--json", help="输出类型化 JSON / Typed JSON output")
    ] = False,
    select: Annotated[
        list[str] | None,
        typer.Option(
            "--select", help="选择 JSON Pointer，可重复 / Select JSON Pointer, repeatable"
        ),
    ] = None,
    language: Annotated[str | None, typer.Option(help="显示语言 / Language: zh-CN or en")] = None,
    debug: Annotated[bool, typer.Option(help="显示错误堆栈 / Show error traceback")] = False,
) -> None:
    if language not in {None, "zh-CN", "en"}:
        raise NousError("configuration.invalid_language", "Language must be zh-CN or en")
    if select and not json_mode:
        raise NousError("input.invalid", "Selecting result fields requires --json")
    ctx.obj = Options(
        locator=InstanceLocator.resolve(instance, profile=profile),
        json_mode=json_mode,
        language=language,
        debug=debug,
        select=validate_json_pointers(select or ()),
    )
    current_options.set(ctx.obj)


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


@catalog_cli.command("list")
def operations_list(ctx: typer.Context) -> None:
    """列出完整应用操作 / List complete application operations."""
    emit(ctx, application_catalog(ctx))


@catalog_cli.command("describe")
def operations_describe(ctx: typer.Context, name: str) -> None:
    """查看请求、结果和事件格式 / Describe an operation's schemas."""
    emit(ctx, application_catalog(ctx, name))


@instance_cli.command("show")
def instance_show(ctx: typer.Context) -> None:
    """显示路径和占有状态 / Show paths and ownership."""
    emit(ctx, inspect_instance(options(ctx).locator))


@instance_cli.command("register")
def instance_register(ctx: typer.Context, name: str, directory: Path) -> None:
    """登记现有实例 / Register an existing instance."""
    emit(ctx, InstanceProfiles().register(name, directory))


@instance_cli.command("use")
def instance_use(ctx: typer.Context, name: str) -> None:
    """选择日常使用的实例 / Select the usual instance."""
    emit(ctx, InstanceProfiles().use(name))


@instance_cli.command("current")
def instance_current(ctx: typer.Context) -> None:
    """显示当前选择 / Show the effective instance selection."""
    selected = options(ctx).locator
    emit(
        ctx,
        {
            "profile": selected.profile,
            "directory": str(selected.directory),
            "config_file": str(selected.config_file),
        },
    )


@config_cli.command("list")
def config_list(
    ctx: typer.Context,
    level: Annotated[
        str, typer.Option(help="standard / advanced / developer / internal")
    ] = "standard",
) -> None:
    """列出类型、默认、作用域与生效方式 / List configuration fields."""
    fields = application_call(ctx, LIST_CONFIGURATION, ListConfiguration(level=level))
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
    emit(
        ctx,
        application_call(ctx, SHOW_CONFIGURATION, ShowConfiguration(saved=saved)),
    )


@config_cli.command("explain")
def config_explain(ctx: typer.Context, path: str) -> None:
    """解释来源、覆盖和生效状态 / Explain value sources and application."""
    emit(
        ctx,
        application_call(ctx, EXPLAIN_CONFIGURATION, ConfigurationPath(path=path)),
    )


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
        parsed = None
    elif null:
        parsed = None
    else:
        assert value is not None
        try:
            parsed = json.loads(value)
        except ValueError:
            parsed = value
    request = SetConfiguration(
        path=path,
        value=cast(JsonValue, parsed),
        input=ExternalInput(key="value") if value_file else None,
    )
    result = application_call(
        ctx,
        SET_CONFIGURATION,
        request,
        inputs={"value": FileInput(value_file)} if value_file else None,
    )
    emit(ctx, result)


@config_cli.command("unset")
def config_unset(ctx: typer.Context, path: str) -> None:
    """删除本层覆盖并恢复继承 / Restore inheritance."""
    emit(ctx, application_call(ctx, UNSET_CONFIGURATION, ConfigurationPath(path=path)))


@config_cli.command("remove-resource")
def config_remove_resource(ctx: typer.Context, path: str) -> None:
    """检查引用后删除资源 / Remove an unreferenced resource."""
    emit(ctx, application_call(ctx, REMOVE_CONFIGURATION_RESOURCE, ConfigurationPath(path=path)))


@config_cli.command("validate")
def config_validate(ctx: typer.Context) -> None:
    """校验语法、字段及资源引用 / Validate syntax, fields and references."""
    emit(
        ctx,
        application_call(ctx, VALIDATE_CONFIGURATION, EmptyRequest()),
    )


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
        text = json.dumps(schema, ensure_ascii=False, indent=2) + "\n"
        from .io import write_text

        run_async(write_text(output, text))
        emit(ctx, {"file": str(output), "state": "saved"})


@runtime_cli.command("check")
def runtime_check(
    ctx: typer.Context, details: Annotated[bool, typer.Option("--details")] = False
) -> None:
    """打开资源并报告本地能力 / Inspect actual local owners."""
    from nous.runtime.operations import CHECK_RUNTIME, ListOperations

    result = application_call(ctx, CHECK_RUNTIME, ListOperations())
    if options(ctx).json_mode or details:
        emit(ctx, result)
        return
    console = Console()
    language = language_for(options(ctx))
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
            summary = (
                f"{value['root_operations']} root operations; "
                f"{value['background_running']} background operations; "
                f"{value['blocking_workers']} blocking workers"
            )
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
            + ", ".join(f"{item['operation']} {item['identifier']}" for item in recent),
            markup=False,
        )
    wave = cast(dict[str, object], result["wave"])
    preparations = cast(list[dict[str, object]], wave.get("recent_preparations", []))
    if preparations:
        preparation = cast(dict[str, object], preparations[0].get("preparation") or {})
        stages = cast(dict[str, object], preparations[0].get("stages_ms") or {})
        console.print(
            f"{translate('Latest content preparation', language)}: "
            f"{preparation.get('documents', 0)} {translate('documents', language)}, "
            f"{preparation.get('candidates', 0)} {translate('candidates', language)}, "
            f"{float(cast(int | float, stages.get('content_prepare', 0))):.0f} ms",
            markup=False,
        )
    observability = cast(dict[str, object], result["observability"])
    problems = cast(list[dict[str, object]], observability.get("recent_problems", []))
    if problems:
        console.print(f"{translate('Recent problems', language)}:", markup=False)
        for problem in problems:
            reference = str(problem.get("operation_id") or "")[:8]
            issues = cast(list[dict[str, object]], problem.get("issues") or [])
            description = "; ".join(
                f"{translate(str(issue.get('stage') or ''), language)}: "
                f"{issue.get('code')} — {issue.get('message')}"
                for issue in issues
            ) or str(problem.get("error_message") or problem.get("error_code") or "")
            completed = ", ".join(
                str(item.get("identifier") or item)
                for item in cast(list[dict[str, object]], problem.get("completed_refs") or [])
            )
            completed_suffix = f" · {completed}" if completed else ""
            console.print(
                f"  {translate(str(problem.get('result') or ''), language)} · "
                f"{reference} · {description}{completed_suffix}",
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
                f"  {event.get('stage', '')}: attempt {event.get('number')} "
                f"{event.get('route')} → {event.get('outcome')} "
                f"{event.get('error_code', '')} {event.get('error_type', '')} "
                f"{event.get('duration_seconds', '')}s"
                + (f" action={event['next_action']}" if event.get("next_action") else "")
                + (f" reason={event['retry_reason']}" if event.get("retry_reason") else "")
                + (f" status={event['provider_status']}" if event.get("provider_status") else "")
            )
        elif event.get("event") in {"operation.terminal", "stage.completed"}:
            typer.echo(
                f"  {event.get('stage', '')}: {event.get('result')} {event.get('error_message', '')} {event.get('duration_seconds', '')}s"
            )
    for issue in cast(list[dict[str, object]], result["issues"]):
        typer.echo(f"  {issue['stage']}: {issue['code']} — {issue['message']}")
    for reference in cast(list[dict[str, object]], result["completed_refs"]):
        typer.echo(f"  {reference.get('identifier') or reference}")


@time_cli.command("parse")
def time_parse(
    ctx: typer.Context,
    value: str,
    domain: Annotated[str, typer.Option(help="时间域 ID / Time domain ID")] = "utc",
    timezone: Annotated[str | None, typer.Option(help="IANA 时区 / IANA timezone")] = None,
    fold: Annotated[int | None, typer.Option(help="DST 重复时刻 / DST fold: 0 or 1")] = None,
) -> None:
    """保留原文、精度和不确定性 / Preserve precision and uncertainty."""
    result = application_call(
        ctx,
        PARSE_TIME,
        ParseTime(value=value, domain=domain, timezone=timezone, fold=fold),
    )
    emit(ctx, result.model_dump(mode="json"))


from . import runtime as _runtime  # noqa: E402, F401  # pyright: ignore[reportUnusedImport]
from .tools import tools_cli  # noqa: E402

cli.add_typer(tools_cli, name="tools")


def main(*, args: list[str] | None = None) -> None:
    current_options.set(None)
    for stream in (sys.stdout, sys.stderr):
        if isinstance(stream, TextIOWrapper):
            stream.reconfigure(encoding="utf-8")
    try:
        exit_code = cli(args=args, standalone_mode=False)
        if isinstance(exit_code, int):
            raise SystemExit(exit_code)
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
        selected = current_options.get()
        if selected and selected.debug:
            Console(stderr=True).print_exception(show_locals=False)
        language = language_for(selected)
        result["message"] = error_message(failure, language)
        if (selected and selected.json_mode) or (
            selected is None and "--json" in (args if args is not None else sys.argv)
        ):
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


from . import cognition as cognition  # noqa: E402, F401
from . import concepts as concepts  # noqa: E402, F401
from . import feedback as feedback  # noqa: E402, F401
from . import journalize as journalize  # noqa: E402, F401
from . import retrieval as retrieval  # noqa: E402, F401
