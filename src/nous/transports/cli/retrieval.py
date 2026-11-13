"""Query, read, follow, arrange, and continue work through the application."""

import json
from pathlib import Path
from typing import Annotated, Literal

import typer
from pydantic import TypeAdapter

from nous.cognition.material_contracts import (
    FollowMaterial,
    MaterialRef,
    MaterialSelection,
    ReadMaterial,
)
from nous.cognition.materials import FOLLOW_MATERIAL, READ_MATERIAL
from nous.cognition.retrieval.contracts import (
    ContextSelection,
    DeliveryRequest,
    EvidenceDelivery,
    QueryAnchor,
    QueryControls,
    QueryRequest,
    RefreshRetrieval,
    SaveEvidence,
)
from nous.cognition.retrieval.operations import QUERY, REFRESH, SAVE_EVIDENCE
from nous.cognition.steward.contracts import ArrangeReading
from nous.cognition.steward.operations import ARRANGE
from nous.cognition.work.contracts import (
    CreateWork,
    ListWorkUses,
    ReadWork,
    RecordWorkUse,
    ReviseWork,
    SetActiveWork,
    WorkState,
)
from nous.cognition.work.operations import (
    CREATE_WORK,
    LIST_WORK_USES,
    READ_WORK,
    RECORD_WORK_USE,
    REVISE_WORK,
    SET_ACTIVE_WORK,
)
from nous.common.errors import NousError
from nous.content.contracts import ReadResult, ResultOutputRef, ResultRef
from nous.content.operations import READ_RESULT
from nous.presentation import translate
from nous.presentation.evidence import format_material_read, format_retrieval
from nous.wave.contracts import DeliveryBudget

from .cognition import journal_cli
from .context import application_call, emit, language_for, options
from .groups import cli

work_cli = typer.Typer(help="持续工作 / Persistent work", no_args_is_help=True)
cli.add_typer(work_cli, name="work")


def _result(ctx: typer.Context, ref: str) -> ResultRef:
    return application_call(ctx, READ_RESULT, ReadResult(ref=ref, sources=False)).ref


def _ref(value: MaterialRef | ResultRef) -> str:
    if isinstance(value, ResultOutputRef):
        return f"{value.result.identifier}:{value.output}"
    return value.identifier


@cli.command("save-evidence")
def save_evidence(
    ctx: typer.Context,
    file: Annotated[Path, typer.Argument(exists=True, dir_okay=False)],
    retain_snapshot: bool = False,
) -> None:
    """Save a previous JSON query result for feedback or work."""
    value: object = json.loads(file.read_text(encoding="utf-8-sig"))
    if isinstance(value, dict):
        fields = TypeAdapter(dict[str, object]).validate_python(value)
        if "evidence" in fields:
            value = fields["evidence"]
    result = application_call(
        ctx,
        SAVE_EVIDENCE,
        SaveEvidence(
            evidence=EvidenceDelivery.model_validate(value), retain_snapshot=retain_snapshot
        ),
    )
    if options(ctx).json_mode:
        emit(ctx, result.model_dump(mode="json"))
    else:
        typer.echo(result.identifier)


@cli.command("query")
def query(
    ctx: typer.Context,
    text: str,
    namespace: str | None = None,
    work: str | None = None,
    context_stage: Annotated[list[str] | None, typer.Option("--context-stage")] = None,
    limit: Annotated[int | None, typer.Option("--limit", min=1)] = None,
    purpose: str | None = None,
    steward: Literal["off", "optional", "required"] | None = None,
    save: bool = False,
    explain: bool = False,
    budget: Annotated[int | None, typer.Option("--budget", min=1)] = None,
    budget_unit: Literal["characters", "bytes", "tokens"] = "characters",
    prior: str | None = None,
    snapshot: str | None = None,
    anchored: bool = False,
) -> None:
    stages = tuple(context_stage or ())
    invalid_stages = set(stages) - {"retrieval", "localization", "evaluation"}
    if invalid_stages:
        raise NousError(
            "input.unsupported_context_stage",
            "Context stage is not available",
            details={"stages": sorted(invalid_stages)},
        )
    selected_stages = (
        TypeAdapter(tuple[Literal["retrieval", "localization", "evaluation"], ...]).validate_python(
            stages
        )
        if stages
        else None
    )
    revision = (
        application_call(ctx, READ_WORK, ReadWork(ref=work, namespace=namespace)).revision_ref
        if work
        else None
    )
    prior_ref = _result(ctx, prior) if prior else None
    anchor = (
        QueryAnchor(
            prior_delivery=prior_ref, snapshot=snapshot, view="anchored" if anchored else "current"
        )
        if prior or snapshot or anchored
        else None
    )
    response = application_call(
        ctx,
        QUERY,
        QueryRequest(
            text=text,
            namespace=namespace,
            controls=QueryControls(limit=limit, diagnostics=explain),
            context=ContextSelection(work=revision, stages=selected_stages),
            anchor=anchor,
            save=save,
            delivery=DeliveryRequest(
                purpose=purpose,
                steward=steward,
                budget=DeliveryBudget(amount=budget, unit=budget_unit) if budget else None,
            ),
        ),
    )
    if options(ctx).json_mode:
        emit(ctx, response.model_dump(mode="json"))
        return
    language = language_for(options(ctx))
    typer.echo(format_retrieval(response, language, include_issues=False))
    for issue in response.issues:
        typer.echo(f"{issue.code}: {issue.message}", err=True)
    if explain:
        typer.echo(
            json.dumps(response.evidence.model_dump(mode="json"), ensure_ascii=False, indent=2)
        )


@cli.command("read")
def read(
    ctx: typer.Context,
    ref: str,
    part: str | None = None,
    namespace: str | None = None,
    sources: bool = False,
    blocks: str | None = None,
) -> None:
    if part and blocks:
        raise NousError("input.invalid", "Choose an evidence part or material blocks")
    request = (
        ReadMaterial(delivery=ref, key=part, namespace=namespace, sources=sources)
        if part
        else ReadMaterial(
            ref=ref,
            namespace=namespace,
            sources=sources,
            blocks=tuple(blocks.split(",")) if blocks else (),
        )
    )
    value = application_call(ctx, READ_MATERIAL, request)
    if options(ctx).json_mode:
        emit(ctx, value.model_dump(mode="json"))
        return
    typer.echo(format_material_read(value, language_for(options(ctx))))


@cli.command("follow")
def follow(
    ctx: typer.Context,
    ref: str,
    relation: str | None = None,
    limit: Annotated[int, typer.Option(min=1)] = 100,
    namespace: str | None = None,
) -> None:
    value = application_call(
        ctx,
        FOLLOW_MATERIAL,
        FollowMaterial(ref=ref, relation=relation, limit=limit, namespace=namespace),
    )
    if options(ctx).json_mode:
        emit(ctx, value.model_dump(mode="json"))
    else:
        for item in value.relations:
            typer.echo(f"{item.kind}: {_ref(item.target)}")


@cli.command("arrange")
def arrange(
    ctx: typer.Context,
    ref: str,
    purpose: str | None = None,
    detail: Literal["concise", "normal", "expanded"] = "normal",
    language: str | None = None,
) -> None:
    value = application_call(
        ctx,
        ARRANGE,
        ArrangeReading(
            evidence=_result(ctx, ref), purpose=purpose, detail=detail, output_language=language
        ),
    )
    if options(ctx).json_mode:
        emit(ctx, value.model_dump(mode="json"))
        return
    if value.brief.title:
        typer.echo(value.brief.title)
    for item in value.brief.items:
        typer.echo(
            item.markdown + (f" [{', '.join(item.evidence_keys)}]" if item.evidence_keys else "")
        )
    typer.echo(value.reading_ref.identifier)


@journal_cli.command("refresh")
def refresh(ctx: typer.Context, namespace: str | None = None, include_review: bool = False) -> None:
    result = application_call(
        ctx,
        REFRESH,
        RefreshRetrieval(namespace=namespace, include_review=True if include_review else None),
    )
    if options(ctx).json_mode:
        emit(ctx, result.model_dump(mode="json"))
    else:
        typer.echo(result.snapshot.identifier)


@work_cli.command("create")
def create_work(
    ctx: typer.Context,
    title: str,
    objective: str,
    body: Path | None = None,
    state: Path | None = None,
    namespace: str | None = None,
) -> None:
    initial = (
        WorkState.model_validate_json(state.read_text(encoding="utf-8"))
        if state
        else WorkState(title=title, objective=objective)
    )
    value = application_call(
        ctx,
        CREATE_WORK,
        CreateWork(
            state=initial,
            body=body.read_text(encoding="utf-8") if body else "",
            namespace=namespace,
        ),
    )
    emit(ctx, value.model_dump(mode="json")) if options(ctx).json_mode else typer.echo(
        value.revision_ref.identifier
    )


@work_cli.command("read")
def read_work(ctx: typer.Context, ref: str, namespace: str | None = None) -> None:
    value = application_call(ctx, READ_WORK, ReadWork(ref=ref, namespace=namespace))
    if options(ctx).json_mode:
        emit(ctx, value.model_dump(mode="json"))
    else:
        language = language_for(options(ctx))
        typer.echo(
            f"{value.state.title} — {value.revision_ref.identifier}\n{value.state.objective}"
        )
        if value.state.focus:
            typer.echo(value.state.focus)
        for item in value.state.decisions:
            typer.echo(f"{item.key} [{translate(item.state, language)}]: {item.statement}")
        for item in value.state.questions:
            typer.echo(f"{item.key} [{translate(item.state, language)}]: {item.question}")
        if value.body:
            typer.echo(value.body)


@work_cli.command("revise")
def revise_work(
    ctx: typer.Context,
    ref: str,
    state: Path,
    expected: str | None = None,
    body: Path | None = None,
    namespace: str | None = None,
) -> None:
    current = application_call(ctx, READ_WORK, ReadWork(ref=ref, namespace=namespace))
    value = application_call(
        ctx,
        REVISE_WORK,
        ReviseWork(
            target=ref,
            expected=expected,
            state=WorkState.model_validate_json(state.read_text(encoding="utf-8")),
            body=body.read_text(encoding="utf-8") if body else current.body,
            namespace=namespace,
        ),
    )
    emit(ctx, value.model_dump(mode="json")) if options(ctx).json_mode else typer.echo(
        value.revision_ref.identifier
    )


@work_cli.command("set-active")
def set_active_work(
    ctx: typer.Context,
    ref: str,
    selections: Path,
    expected: str | None = None,
    namespace: str | None = None,
) -> None:
    active = TypeAdapter(tuple[MaterialSelection, ...]).validate_json(selections.read_bytes())
    value = application_call(
        ctx,
        SET_ACTIVE_WORK,
        SetActiveWork(target=ref, expected=expected, active=active, namespace=namespace),
    )
    emit(ctx, value.model_dump(mode="json")) if options(ctx).json_mode else typer.echo(
        value.revision_ref.identifier
    )


@work_cli.command("record-use")
def record_work_use(
    ctx: typer.Context,
    ref: str,
    evidence: str,
    selections: Path,
    purpose: str,
    reading: str | None = None,
    decision: str | None = None,
    namespace: str | None = None,
) -> None:
    revision = application_call(ctx, READ_WORK, ReadWork(ref=ref, namespace=namespace)).revision_ref
    actual = TypeAdapter(tuple[MaterialSelection, ...]).validate_json(selections.read_bytes())
    value = application_call(
        ctx,
        RECORD_WORK_USE,
        RecordWorkUse(
            revision=revision,
            evidence=_result(ctx, evidence),
            reading=_result(ctx, reading) if reading else None,
            selections=actual,
            purpose=purpose,
            decision_key=decision,
        ),
    )
    emit(ctx, value.model_dump(mode="json")) if options(ctx).json_mode else typer.echo(value.id)


@work_cli.command("uses")
def list_work_uses(ctx: typer.Context, ref: str, namespace: str | None = None) -> None:
    values = application_call(ctx, LIST_WORK_USES, ListWorkUses(ref=ref, namespace=namespace))
    if options(ctx).json_mode:
        emit(ctx, {"uses": [item.model_dump(mode="json") for item in values]})
    else:
        for item in values:
            typer.echo(f"{item.revision.identifier} · {item.purpose} · {item.evidence.identifier}")
