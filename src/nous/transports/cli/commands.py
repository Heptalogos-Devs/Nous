"""Everyday terminal operations use one bound Namespace client."""

from __future__ import annotations

import sys
from os.path import commonpath
from pathlib import Path
from typing import Annotated, Literal

import typer
from nous_sdk.contracts import ImportFormat, MediaReference, QueryRequest
from nous_sdk.contracts.common import FormationPurpose
from nous_sdk.contracts.context import ChangeWorkContext, WorkContextContent
from nous_sdk.presentation import present, present_query

from .client_state import ClientState, save
from .common import invoke, invoke_bound, options


def content(value: str | None) -> str:
    selected = sys.stdin.read() if value is None or value == "-" else value
    if not selected.strip():
        raise ValueError("Provide content or pipe it through stdin")
    return selected


def work_key(ctx: typer.Context, namespace_id: str) -> str:
    return options(ctx).url.rstrip("/") + "|" + namespace_id


def register(cli: typer.Typer) -> None:
    @cli.command("use")
    def use(ctx: typer.Context, namespace: str, project: bool = False) -> None:
        """Save the current Namespace; --project selects this directory."""

        async def choose(root):
            bound = await root.namespace(namespace)
            selected = options(ctx)
            state = selected.client_state or ClientState()
            target = Path.cwd() / ".nous.toml" if project else selected.client_file
            assert target is not None
            save(
                target,
                state.model_copy(
                    update={
                        "url": selected.url,
                        "instance": selected.instance,
                        "credential_file": selected.credential_file,
                        "namespace": bound.namespace_id,
                    }
                ),
            )
            return {
                "ref": "namespace:" + bound.namespace_id,
                "text": bound.identity.display_name,
                "state": bound.identity.state,
            }

        invoke(ctx, choose)

    @cli.command("remember")
    def remember(
        ctx: typer.Context,
        text: Annotated[str | None, typer.Argument()] = None,
        source: str | None = None,
        tag: Annotated[list[str] | None, typer.Option()] = None,
        pin: bool = False,
    ) -> None:
        """Save explicit content. Use '-' or stdin for several paragraphs."""
        selected = content(text)
        invoke_bound(
            ctx,
            lambda namespace: namespace.remember(
                selected, source=source, tags=tuple(tag or ()), pin=pin
            ),
        )

    @cli.command("query")
    def query(
        ctx: typer.Context,
        query: Annotated[str, typer.Argument()] = "",
        context: str | None = None,
        profile: str | None = None,
        limit: Annotated[int, typer.Option(min=1, max=1000)] = 8,
        strict: bool = False,
        trace: bool = False,
        image: Annotated[list[Path] | None, typer.Option()] = None,
        audio: Annotated[list[Path] | None, typer.Option()] = None,
        video: Annotated[list[Path] | None, typer.Option()] = None,
        file: Annotated[list[Path] | None, typer.Option()] = None,
    ) -> None:
        """Find natural language or NousQL matches; continue with 'nous read REF'."""

        async def run(namespace):
            media: list[MediaReference] = []
            paths = [*(image or ()), *(audio or ()), *(video or ()), *(file or ())]
            if paths:
                submission = await namespace.client.import_files(
                    namespace.namespace_id,
                    paths,
                    transfer_root=Path(commonpath([str(path.resolve().parent) for path in paths])),
                    formation_purpose=None,
                )
                await namespace.client.wait_success(
                    (submission.job_id,), namespace_id=namespace.namespace_id
                )
                imported = await namespace.client.text_import_result(
                    namespace.namespace_id, submission.request_id
                )
                for unit in imported.admission.catalog.units:
                    cue = unit.source or MediaReference(
                        kind="material", id=unit.material_id, observation_id=unit.observation_id
                    )
                    if cue not in media:
                        media.append(cue)
            selected_context = context
            state = options(ctx).client_state
            if selected_context is None and state is not None:
                selected_context = state.work.get(work_key(ctx, namespace.namespace_id))
            result = await namespace.query(
                QueryRequest(
                    text=query,
                    context=selected_context,
                    media=tuple(media),
                    wave_profile=profile,
                    limit=limit,
                    capability_policy="strict" if strict else None,
                )
            )
            if trace and not options(ctx).raw:
                return {**present_query(result).model_dump(mode="json"), "trace": result.plan}
            return result

        invoke_bound(ctx, run)

    @cli.command("read")
    def read(
        ctx: typer.Context,
        ref: str,
        sources: bool = False,
        cursor: str | None = None,
        offset: int = 0,
        limit: Annotated[int, typer.Option(min=1, max=1000000)] = 12000,
    ) -> None:
        """Read a result, object, material, work or job reference."""
        invoke_bound(
            ctx,
            lambda namespace: namespace.read(
                ref, sources=sources, cursor=cursor, offset=offset, max_chars=limit
            ),
        )

    @cli.command("correct")
    def correct(
        ctx: typer.Context,
        ref: str,
        text: Annotated[str | None, typer.Argument()] = None,
        reason: str = "",
        source: str | None = None,
    ) -> None:
        """Save a correction as a new revision of the selected memory."""
        selected = content(text)
        invoke_bound(
            ctx, lambda namespace: namespace.correct(ref, selected, reason=reason, source=source)
        )

    @cli.command("history")
    def history(ctx: typer.Context, ref: str, limit: int = 20) -> None:
        async def run(namespace):
            return {
                "items": [present(record) for record in await namespace.history(ref, limit=limit)]
            }

        invoke_bound(ctx, run)

    @cli.command("import")
    def import_files(
        ctx: typer.Context,
        paths: Annotated[list[Path], typer.Argument(exists=True, dir_okay=False)],
        purpose: FormationPurpose = "memory",
        no_formation: bool = False,
        wait: Annotated[bool, typer.Option("--wait/--no-wait")] = True,
        format: ImportFormat | None = None,
    ) -> None:
        """Capture originals and form memory notes, graph links, or both."""

        async def run(namespace):
            submission = await namespace.client.import_files(
                namespace.namespace_id,
                paths,
                transfer_root=Path(commonpath([str(path.resolve().parent) for path in paths])),
                input_format=format,
                formation_purpose=None if no_formation else purpose,
                completion="formed" if not no_formation and wait else "accepted",
            )
            if not wait:
                return {"ref": "job:" + submission.job_id, "status": "queued"}
            await namespace.client.wait_success(
                (submission.job_id,), namespace_id=namespace.namespace_id
            )
            result = await namespace.client.text_import_result(
                namespace.namespace_id, submission.request_id
            )
            return (
                result
                if options(ctx).raw
                else {
                    "ref": "job:" + submission.job_id,
                    "status": "completed",
                    "materials": [
                        {"ref": "material:" + unit.material_id}
                        for unit in result.admission.catalog.units
                    ],
                    "items": [
                        {"ref": reference.exact(), "kind": reference.kind}
                        for output in result.admission.outputs
                        for reference in output.cognition
                    ],
                }
            )

        invoke_bound(ctx, run)

    @cli.command("status")
    def status(ctx: typer.Context) -> None:
        async def run(root):
            response = await root.request("GET", "/api/manage/status")
            if options(ctx).raw:
                return response
            result: dict[str, object] = {"service": "connected", "url": options(ctx).url}
            if selected := options(ctx).namespace:
                bound = await root.namespace(selected)
                result.update(
                    namespace=bound.identity.slug,
                    state=bound.identity.state,
                    features=list(bound.identity.enabled_features),
                )
            return result

        invoke(ctx, run)

    work = typer.Typer(
        help="Keep a goal, progress, questions and reading anchors.", no_args_is_help=True
    )
    cli.add_typer(work, name="work")

    @work.command("start")
    def start_work(
        ctx: typer.Context,
        title: str,
        text: str = "",
        anchor: Annotated[list[str] | None, typer.Option()] = None,
    ) -> None:
        async def run(namespace):
            from nous_sdk.contracts.targets import CognitiveTarget, MediaTarget

            targets = []
            for ref in anchor or ():
                value = await namespace.resolve_reference(ref)
                if isinstance(value.reference, (CognitiveTarget, MediaTarget)):
                    targets.append(value.reference)
            created = await namespace.runtime.create_work(
                ChangeWorkContext(action='create', content=WorkContextContent(purpose=title, context_text=text, anchors=tuple(targets))),
            )
            selected = options(ctx)
            state = selected.client_state or ClientState()
            assert selected.client_file is not None
            save(
                selected.client_file,
                state.model_copy(
                    update={
                        "work": {
                            **state.work,
                            work_key(ctx, namespace.namespace_id): "work:" + created.id,
                        }
                    }
                ),
            )
            return created

        invoke_bound(ctx, run)

    @work.command("show")
    def show_work(ctx: typer.Context, ref: Annotated[str | None, typer.Argument()] = None) -> None:
        async def run(namespace):
            state = options(ctx).client_state
            chosen = ref or (
                state.work.get(work_key(ctx, namespace.namespace_id)) if state else None
            )
            if chosen is None:
                return {
                    "items": [
                        present(value)
                        for value in await namespace.runtime.works()
                    ]
                }
            return await namespace.read(chosen)

        invoke_bound(ctx, run)

    @work.command("update")
    def update_work(
        ctx: typer.Context,
        ref: str,
        text: Annotated[str | None, typer.Argument()] = None,
        question: Annotated[list[str] | None, typer.Option()] = None,
    ) -> None:
        selected = content(text)

        async def run(namespace):
            current = await namespace.runtime.work(ref)
            updated = current.content.model_copy(
                update={
                    "context_text": selected,
                    "unresolved_questions": tuple(question)
                    if question is not None
                    else current.content.unresolved_questions,
                }
            )
            return await namespace.runtime.change_work(
                ChangeWorkContext(work_context_id=current.id, expected_revision=current.revision, action='edit', content=updated),
            )

        invoke_bound(ctx, run)

    def change_work(ctx: typer.Context, ref: str, action: Literal["pause", "resume", "end"]):
        async def run(namespace):
            current = await namespace.runtime.work(ref)
            return await namespace.runtime.change_work(
                ChangeWorkContext(work_context_id=current.id, expected_revision=current.revision, action=action),
            )

        invoke_bound(ctx, run)

    @work.command("pause")
    def pause_work(ctx: typer.Context, ref: str):
        change_work(ctx, ref, "pause")

    @work.command("resume")
    def resume_work(ctx: typer.Context, ref: str):
        change_work(ctx, ref, "resume")

    @work.command("finish")
    def finish_work(ctx: typer.Context, ref: str):
        change_work(ctx, ref, "end")

    jobs = typer.Typer(
        help="Inspect and continue actual background work.", invoke_without_command=True
    )
    cli.add_typer(jobs, name="jobs")

    @jobs.callback()
    def job_list(ctx: typer.Context, ref: Annotated[str | None, typer.Argument()] = None):
        if ctx.invoked_subcommand is None:
            invoke_bound(
                ctx, lambda namespace: namespace.jobs.get(ref) if ref else namespace.jobs.list()
            )

    @jobs.command("cancel")
    def cancel_job(ctx: typer.Context, ref: str):
        invoke_bound(ctx, lambda namespace: namespace.jobs.cancel(ref))

    @jobs.command("resume")
    def resume_job(ctx: typer.Context, ref: str):
        invoke_bound(ctx, lambda namespace: namespace.jobs.resume(ref))

    namespaces = typer.Typer(
        help="Manage independent cognitive data domains.", no_args_is_help=True
    )
    cli.add_typer(namespaces, name="namespace")

    @namespaces.command("list")
    def namespace_list(ctx: typer.Context):
        async def run(root):
            return {
                "items": [value.model_dump(mode="json") for value in await root.namespaces.list()]
            }

        invoke(ctx, run)

    @namespaces.command("create")
    def namespace_create(ctx: typer.Context, name: str, persona: bool = False):
        invoke(
            ctx,
            lambda root: root.namespaces.create(name, mode="subject" if persona else "knowledge"),
        )
