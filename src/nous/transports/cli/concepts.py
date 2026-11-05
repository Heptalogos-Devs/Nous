"""Concept meaning management and exact-body annotation commands."""

import json
from pathlib import Path
from typing import Annotated, cast

import typer

from nous.cognition.concept_contracts import AnnotationProposalView, AnnotationView, ConceptView
from nous.cognition.concepts import ConceptTarget
from nous.common.errors import NousError
from nous.content.annotations import AnnotationProposal
from nous.presentation import translate
from nous.runtime.bootstrap import open_instance

from . import cli, language_for, options, run_async
from .cognition import journal_cli, note_cli

concept_cli = typer.Typer(help="Concept meanings", no_args_is_help=True)
annotation_cli = typer.Typer(help="Annotations", no_args_is_help=True)
cli.add_typer(concept_cli, name="concept")
journal_cli.add_typer(annotation_cli, name="annotation")


def emit_concept(ctx: typer.Context, result: ConceptView) -> None:
    if options(ctx).json_mode:
        typer.echo(result.model_dump_json())
    else:
        typer.echo(f"{result.label} — {result.ref.address}\n{result.description}")
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
    async def run():
        async with open_instance(options(ctx).locator) as runtime:
            return await (await runtime.cognition(namespace)).concepts.create(
                namespace, label, description, alias or (), author=author
            )

    emit_concept(ctx, run_async(run()))


@concept_cli.command("read")
def concept_read(ctx: typer.Context, ref: str, namespace: str | None = None):
    async def run():
        async with open_instance(options(ctx).locator) as runtime:
            return await (await runtime.cognition(namespace)).concepts.read(ref, namespace)

    emit_concept(ctx, run_async(run()))


@concept_cli.command("find")
def concept_find(
    ctx: typer.Context,
    text: str = "",
    namespace: str | None = None,
    limit: int = 20,
    offset: int = 0,
):
    async def run():
        async with open_instance(options(ctx).locator) as runtime:
            return await (await runtime.cognition(namespace)).concepts.find(
                namespace, text, offset=offset, limit=limit
            )

    result = run_async(run())
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

    async def run():
        async with open_instance(options(ctx).locator) as runtime:
            return await (await runtime.cognition(namespace)).concepts.revise(
                ref, expected, label, description, alias or (), namespace=namespace, author=author
            )

    emit_concept(ctx, run_async(run()))


def emit_annotation(ctx: typer.Context, result: AnnotationProposalView | AnnotationView):
    if options(ctx).json_mode:
        typer.echo(result.model_dump_json())
    elif isinstance(result, AnnotationProposalView):
        typer.echo(
            f"{translate('Proposal', language_for(options(ctx)))}: {result.result_ref.address}\n{translate('Body', language_for(options(ctx)))}: {result.body_revision.address}"
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
            f"{translate('Annotation', language_for(options(ctx)))}: {result.ref.address}\n{translate('Body', language_for(options(ctx)))}: {result.ref.body_revision.address}"
        )
        for key, concept in result.bindings.items():
            typer.echo(f"{key}: {concept.label} — {concept.ref.address}")


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
        list[str] | None, typer.Option("--known-concept", help='"C1=con:...@N"')
    ] = None,
    draft_only: bool = False,
    historical: bool = False,
    task: str | None = None,
):
    if proposal is not None and proposal_result is not None:
        raise NousError("input.conflicting_proposal", "Choose --proposal or --proposal-result")
    candidates: dict[str, ConceptTarget] = {}
    for value in known_concept or ():
        key, separator, target = value.partition("=")
        if not separator or not key or not target or key in candidates:
            raise NousError(
                "input.invalid_candidate", 'Use distinct --known-concept "C1=con:...@N" entries'
            )
        candidates[key] = target
    supplied: dict[str, ConceptTarget | None] | None = None
    if bindings is not None:
        raw: object = json.loads(bindings.read_text(encoding="utf-8"))
        if not isinstance(raw, dict) or any(
            not isinstance(key, str) or (value is not None and not isinstance(value, str))
            for key, value in cast(dict[object, object], raw).items()
        ):
            raise NousError(
                "input.invalid_bindings",
                "Bindings must map local Concept keys to exact refs or null for new",
            )
        supplied = cast(dict[str, ConceptTarget | None], raw)
    manual = (
        AnnotationProposal.model_validate_json(proposal.read_text(encoding="utf-8"))
        if proposal
        else None
    )

    async def run():
        async with open_instance(options(ctx).locator) as runtime:
            return await (await runtime.cognition(namespace)).annotations.annotate(
                ref,
                namespace=namespace,
                proposal=manual,
                proposal_result=proposal_result,
                known_concepts=candidates if known_concept is not None else None,
                bindings=supplied,
                draft_only=draft_only,
                task=task,
                activate_for_current=not historical,
            )

    emit_annotation(ctx, run_async(run()))


@annotation_cli.command("read")
def annotation_read(ctx: typer.Context, ref: str, namespace: str | None = None):
    async def run():
        async with open_instance(options(ctx).locator) as runtime:
            return await (await runtime.cognition(namespace)).annotations.read(
                ref, namespace=namespace
            )

    emit_annotation(ctx, run_async(run()))


@annotation_cli.command("proposal")
def proposal_read(ctx: typer.Context, ref: str, namespace: str | None = None):
    async def run():
        async with open_instance(options(ctx).locator) as runtime:
            return await (await runtime.cognition(namespace)).annotations.read_proposal(ref)

    emit_annotation(ctx, run_async(run()))
