"""Concept meaning management and exact-body annotation commands."""

from pathlib import Path
from typing import Annotated

import typer

from nous.application.inputs import FileInput, InputProvider
from nous.cognition.annotation.contracts import (
    AnnotateRecord,
    ReadAnnotation,
    ReadAnnotationProposal,
)
from nous.cognition.annotation.operations import (
    ANNOTATE_RECORD,
    READ_ANNOTATION,
    READ_ANNOTATION_PROPOSAL,
)
from nous.cognition.concept_contracts import (
    AnnotationProposalView,
    AnnotationView,
    ConceptView,
    CreateConcept,
    FindConcepts,
    ReadConcept,
    ReviseConcept,
)
from nous.cognition.concepts import (
    CREATE_CONCEPT,
    FIND_CONCEPTS,
    READ_CONCEPT,
    REVISE_CONCEPT,
    ConceptTarget,
)
from nous.common.errors import NousError
from nous.content.contracts import ExternalInput
from nous.presentation import translate

from .cognition import journal_cli, note_cli
from .context import application_call, emit, language_for, options
from .groups import cli

concept_cli = typer.Typer(help="Concept meanings", no_args_is_help=True)
annotation_cli = typer.Typer(help="Annotations", no_args_is_help=True)
cli.add_typer(concept_cli, name="concept")
journal_cli.add_typer(annotation_cli, name="annotation")


def emit_concept(ctx: typer.Context, result: ConceptView) -> None:
    if options(ctx).json_mode:
        emit(ctx, result.model_dump(mode="json"))
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
    result = application_call(
        ctx,
        CREATE_CONCEPT,
        CreateConcept(
            namespace=namespace,
            label=label,
            description=description,
            aliases=tuple(alias or ()),
            author=author,
        ),
    )
    emit_concept(ctx, result)


@concept_cli.command("read")
def concept_read(ctx: typer.Context, ref: str, namespace: str | None = None):
    result = application_call(ctx, READ_CONCEPT, ReadConcept(target=ref, namespace=namespace))
    emit_concept(ctx, result)


@concept_cli.command("find")
def concept_find(
    ctx: typer.Context,
    text: str = "",
    namespace: str | None = None,
    limit: int = 20,
    offset: int = 0,
):
    result = application_call(
        ctx,
        FIND_CONCEPTS,
        FindConcepts(namespace=namespace, text=text, offset=offset, limit=limit),
    )
    if options(ctx).json_mode:
        emit(ctx, {"concepts": [item.model_dump(mode="json") for item in result]})
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

    result = application_call(
        ctx,
        REVISE_CONCEPT,
        ReviseConcept(
            target=ref,
            expected_revision=expected,
            label=label,
            description=description,
            aliases=tuple(alias or ()),
            namespace=namespace,
            author=author,
        ),
    )
    emit_concept(ctx, result)


def emit_annotation(ctx: typer.Context, result: AnnotationProposalView | AnnotationView):
    if options(ctx).json_mode:
        emit(ctx, result.model_dump(mode="json"))
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
        from nous.cognition.maintenance.contracts import MaintainRecord
        from nous.cognition.maintenance.operations import MAINTAIN_RECORD

        result = application_call(
            ctx,
            MAINTAIN_RECORD,
            MaintainRecord(
                target=ref, namespace=namespace, task=task, refresh=refresh, execution="background"
            ),
        )
    else:
        inputs: dict[str, InputProvider] = {}
        if bindings is not None:
            inputs["bindings"] = FileInput(bindings)
        if proposal is not None:
            inputs["proposal"] = FileInput(proposal)
        result = application_call(
            ctx,
            ANNOTATE_RECORD,
            AnnotateRecord(
                target=ref,
                namespace=namespace,
                proposal=ExternalInput(key="proposal") if proposal else None,
                proposal_result=proposal_result,
                known_concepts=candidates if known_concept is not None else None,
                bindings=ExternalInput(key="bindings") if bindings else {},
                draft_only=draft_only,
                task=task,
                activate_for_current=not historical,
            ),
            inputs=inputs,
        )
    emit(ctx, result.model_dump(mode="json"))


@annotation_cli.command("read")
def annotation_read(ctx: typer.Context, ref: str, namespace: str | None = None):
    result = application_call(ctx, READ_ANNOTATION, ReadAnnotation(ref=ref, namespace=namespace))
    emit_annotation(ctx, result)


@annotation_cli.command("proposal")
def proposal_read(ctx: typer.Context, ref: str, namespace: str | None = None):
    result = application_call(ctx, READ_ANNOTATION_PROPOSAL, ReadAnnotationProposal(ref=ref))
    emit_annotation(ctx, result)
