"""Concept meaning management and exact-body annotation commands."""

import json
from pathlib import Path
from typing import Annotated, cast

import typer

from nous.application.inputs import InputParameter
from nous.cognition.concept_contracts import AnnotationProposalView, AnnotationView, ConceptView
from nous.cognition.concepts import ConceptTarget
from nous.common.errors import NousError
from nous.presentation import translate

from . import application_action, cli, language_for, options
from .cognition import journal_cli, note_cli
from .input_files import json_parameter

concept_cli = typer.Typer(help="Concept meanings", no_args_is_help=True)
annotation_cli = typer.Typer(help="Annotations", no_args_is_help=True)
cli.add_typer(concept_cli, name="concept")
journal_cli.add_typer(annotation_cli, name="annotation")


def emit_concept(ctx: typer.Context, result: ConceptView) -> None:
    if options(ctx).json_mode:
        typer.echo(result.model_dump_json())
    else:
        typer.echo(f"{result.label} — {result.ref.identifier}\n{result.description}")
        if result.aliases:
            typer.echo(
                translate("Aliases", language_for(options(ctx))) + ": " + ", ".join(result.aliases)
            )


@concept_cli.command("add")
def concept_add(
    ctx: typer.Context,
    label: str,
    description: str,
    namespace: str | None = None,
    alias: Annotated[list[str] | None, typer.Option("--alias")] = None,
    author: str | None = None,
):
    result = application_action(
        ctx,
        "concepts.create",
        namespace,
        label,
        description,
        alias or (),
        author=author,
        scope=namespace,
    )
    emit_concept(ctx, cast(ConceptView, result))


@concept_cli.command("read")
def concept_read(ctx: typer.Context, ref: str, namespace: str | None = None):
    result = application_action(ctx, "concepts.read", ref, namespace, scope=namespace)
    emit_concept(ctx, cast(ConceptView, result))


@concept_cli.command("find")
def concept_find(
    ctx: typer.Context,
    text: str = "",
    namespace: str | None = None,
    limit: int = 20,
    offset: int = 0,
):
    result = cast(
        tuple[ConceptView, ...],
        application_action(
            ctx, "concepts.find", namespace, text, offset=offset, limit=limit, scope=namespace
        ),
    )
    if options(ctx).json_mode:
        typer.echo(
            json.dumps(
                {"concepts": [item.model_dump(mode="json") for item in result]}, ensure_ascii=False
            )
        )
    else:
        for concept in result:
            emit_concept(ctx, concept)


@concept_cli.command("list")
def concept_list(
    ctx: typer.Context, namespace: str | None = None, limit: int = 20, offset: int = 0
):
    concept_find(ctx, "", namespace, limit, offset)


@concept_cli.command("revise")
def concept_revise(
    ctx: typer.Context,
    ref: str,
    label: str,
    description: str,
    expected: str | None = None,
    namespace: str | None = None,
    alias: Annotated[list[str] | None, typer.Option("--alias")] = None,
    author: str | None = None,
):
    if expected is None:
        if "@" not in ref:
            raise NousError(
                "input.expected_required", "Provide an exact Concept ref with @N or --expected"
            )
        expected = ref
    elif expected.isdecimal():
        expected = ref.split("@")[0] + "@" + expected

    result = application_action(
        ctx,
        "concepts.revise",
        ref,
        expected,
        label,
        description,
        alias or (),
        namespace=namespace,
        author=author,
        scope=namespace,
    )
    emit_concept(ctx, cast(ConceptView, result))


def emit_annotation(ctx: typer.Context, result: AnnotationProposalView | AnnotationView):
    if options(ctx).json_mode:
        typer.echo(result.model_dump_json())
    elif isinstance(result, AnnotationProposalView):
        typer.echo(
            f"{translate('Proposal', language_for(options(ctx)))}: {result.result_ref.identifier}\n{translate('Body', language_for(options(ctx)))}: {result.body_revision.identifier}"
        )
        for concept in result.annotation.concepts:
            typer.echo(
                f"{concept.key}: "
                + (
                    f"{translate('Reuse', language_for(options(ctx)))} {concept.reuse_key}"
                    if concept.reuse_key
                    else concept.label
                )
            )
    else:
        typer.echo(
            f"{translate('Annotation', language_for(options(ctx)))}: {result.ref.identifier}\n{translate('Body', language_for(options(ctx)))}: {result.ref.body_revision.identifier}"
        )
        for key, concept in result.bindings.items():
            typer.echo(f"{key}: {concept.label} — {concept.ref.identifier}")


@journal_cli.command("annotate")
@note_cli.command("annotate")
def annotate(
    ctx: typer.Context,
    ref: str,
    namespace: str | None = None,
    proposal: Annotated[
        Path | None, typer.Option("--proposal", help="Human AnnotationProposal JSON")
    ] = None,
    proposal_result: str | None = None,
    bindings: Path | None = None,
    known_concept: Annotated[
        list[str] | None, typer.Option("--known-concept", help='"C1=con(...)@N"')
    ] = None,
    draft_only: bool = False,
    historical: bool = False,
    task: str | None = None,
    background: bool = False,
    refresh: bool = False,
):
    if proposal is not None and proposal_result is not None:
        raise NousError("input.conflicting_proposal", "Choose --proposal or --proposal-result")
    if background and (
        proposal is not None
        or proposal_result is not None
        or known_concept is not None
        or bindings is not None
        or draft_only
        or historical
    ):
        raise NousError(
            "input.unsupported_background_annotation",
            "Background annotation generates and adopts a current revision without manual options",
        )
    if refresh and not background:
        raise NousError("input.invalid", "--refresh requires --background for annotation")
    candidates: dict[str, ConceptTarget] = {}
    for value in known_concept or ():
        key, separator, target = value.partition("=")
        if not separator or not key or not target or key in candidates:
            raise NousError(
                "input.invalid_candidate", 'Use distinct --known-concept "C1=con(...)@N" entries'
            )
        candidates[key] = target
    if background:
        result = application_action(
            ctx,
            "maintenance.request_annotation",
            ref,
            namespace=namespace,
            task=task,
            refresh=refresh,
        )
    else:
        parameters: list[InputParameter] = []
        if bindings is not None:
            parameters.append(json_parameter("bindings", bindings))
        if proposal is not None:
            parameters.append(json_parameter("proposal", proposal))
        result = application_action(
            ctx,
            "annotations.from_input",
            ref,
            scope=namespace,
            namespace=namespace,
            proposal=None,
            proposal_result=proposal_result,
            known_concepts=candidates if known_concept is not None else None,
            bindings=None,
            draft_only=draft_only,
            task=task,
            activate_for_current=not historical,
            input_parameters=parameters,
        )
    if background:
        from nous.jobs.contracts import JobRef

        assert isinstance(result, JobRef)
        if options(ctx).json_mode:
            typer.echo(result.model_dump_json())
        else:
            language = language_for(options(ctx))
            typer.echo(
                f"{translate('Queued', language)} · {translate('Job', language)}: {result.workflow_id}"
            )
    else:
        assert isinstance(result, (AnnotationProposalView, AnnotationView))
        emit_annotation(ctx, result)


@annotation_cli.command("read")
def annotation_read(ctx: typer.Context, ref: str, namespace: str | None = None):
    result = application_action(ctx, "annotations.read", ref, scope=namespace, namespace=namespace)
    emit_annotation(ctx, cast(AnnotationView, result))


@annotation_cli.command("proposal")
def proposal_read(ctx: typer.Context, ref: str, namespace: str | None = None):
    result = application_action(ctx, "annotations.read_proposal", ref, scope=namespace)
    emit_annotation(ctx, cast(AnnotationProposalView, result))


@annotation_cli.command("recover")
def annotation_recover(ctx: typer.Context, ref: str, namespace: str | None = None):
    result = application_action(ctx, "annotations.recover", ref, scope=namespace)
    emit_annotation(ctx, cast(AnnotationProposalView | AnnotationView, result))
