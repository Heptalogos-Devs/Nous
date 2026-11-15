"""Adopt and read immutable work revisions and their actual uses."""

from __future__ import annotations

from collections.abc import Iterable
from datetime import datetime
from typing import TYPE_CHECKING, cast
from uuid import uuid4

import sqlalchemy as sa
from pydantic import TypeAdapter
from sqlalchemy.dialects.sqlite import insert

from nous.application.contracts import CallContext, define_operation
from nous.cognition.contracts import NamespaceRef
from nous.cognition.material_contracts import (
    MaterialSelection,
)
from nous.cognition.scope import NamespaceOperations
from nous.common.errors import NousError
from nous.common.identifier import insert_identifier, split_revision
from nous.common.material_refs import WorkRef, WorkRevisionRef
from nous.content.artifacts import ArtifactStore
from nous.content.contracts import ContentSelection, ResultOutputRef, SourceSelection
from nous.content.results import ResultStore
from nous.presentation import select_language, translate
from nous.runtime.execution import ExecutionManager
from nous.storage.content import PublishedContent
from nous.storage.database import Database, WriteTx
from nous.temporal.values import Clock

from . import schema
from .contracts import (
    CreateWork,
    ListWorkUses,
    ReadWork,
    RecordWorkUse,
    ReviseWork,
    SetActiveWork,
    WorkContext,
    WorkContextField,
    WorkState,
    WorkUse,
    WorkView,
)
from .reading import WorkReader

if TYPE_CHECKING:
    from nous.cognition.materials import Materials

CREATE_WORK = define_operation(
    "work.create",
    request=CreateWork,
    result=WorkView,
    description="Create persistent work with its objective, decisions and sources.",
)
READ_WORK = define_operation(
    "work.read",
    request=ReadWork,
    result=WorkView,
    description="Read the current or an exact work revision.",
)
REVISE_WORK = define_operation(
    "work.revise",
    request=ReviseWork,
    result=WorkView,
    description="Adopt a complete work revision against an expected revision.",
)
SET_ACTIVE_WORK = define_operation(
    "work.set_active",
    request=SetActiveWork,
    result=WorkView,
    description="Replace the work's actively used materials.",
)
RECORD_WORK_USE = define_operation(
    "work.record_use",
    request=RecordWorkUse,
    result=WorkUse,
    description="Record the evidence actually used in work.",
)
LIST_WORK_USES = define_operation(
    "work.list_uses",
    request=ListWorkUses,
    result=tuple[WorkUse, ...],
    description="Read the uses recorded for work and its revisions.",
)


def render_work(
    state: WorkState, body: str, language: str
) -> tuple[bytes, dict[str, tuple[int, int]]]:
    """Render structured work and retain exact byte coordinates for every field."""
    parts: list[bytes] = []
    fields: dict[str, tuple[int, int]] = {}

    def field(key: str, label: str, value: str) -> None:
        if not value:
            return
        parts.append(f"## {label}\n\n".encode())
        start = sum(map(len, parts))
        parts.append(value.encode("utf-8"))
        fields[key] = (start, sum(map(len, parts)))
        parts.append(b"\n\n")

    field("title", translate("Title", language), state.title)
    field("objective", translate("Objective", language), state.objective)
    field("focus", translate("Focus", language), state.focus or "")
    for decision in state.decisions:
        field(
            "decision:" + decision.key,
            translate("Decision {key} ({state})", language).format(
                key=decision.key, state=translate(decision.state, language)
            ),
            decision.statement,
        )
    for question in state.questions:
        field(
            "question:" + question.key,
            translate("Question {key} ({state})", language).format(
                key=question.key, state=translate(question.state, language)
            ),
            question.question,
        )
    field("progress", translate("Progress", language), state.progress)
    for index, blocker in enumerate(state.blockers):
        field(f"blocker:{index}", translate("Blocker", language), blocker)
    field("body", translate("Notes", language), body)
    return b"".join(parts), fields


class WorkOperations:
    """Own work identity, complete revisions, and references to actual evidence use."""

    def __init__(
        self,
        database: Database,
        execution: ExecutionManager,
        scope: NamespaceOperations,
        artifacts: ArtifactStore,
        results: ResultStore,
        materials: Materials,
        clock: Clock,
        reader: WorkReader,
    ):
        self.database, self.execution, self.scope = database, execution, scope
        self.reader = reader
        self.artifacts, self.results, self.materials, self.clock = (
            artifacts,
            results,
            materials,
            clock,
        )

    async def _validate_state(self, state: WorkState, namespace: NamespaceRef) -> None:
        values = self.reader.references(state)
        unselected = tuple(item.material for _, item in values if not item.selections)
        if unselected:
            await self.materials.resolve_many(unselected, namespace=namespace)
        selected = tuple(item for _, item in values if item.selections)
        if selected:
            await self.materials.read_many(selected, namespace=namespace)

    async def context(
        self, target: str | WorkRevisionRef, namespace: NamespaceRef, window: int
    ) -> WorkContext:
        """Select a work revision's active context without reading its free-form body."""
        async with self.database.read() as tx:
            work, revision, _ = await self.reader.resolve_in(tx, target, namespace)
            ref = self.reader.ref(work, revision)
            representation = (
                (
                    await tx.connection.execute(
                        sa.select(schema.representations).where(
                            schema.representations.c.revision_id == revision["id"]
                        )
                    )
                )
                .mappings()
                .one()
            )
            artifact = await self.artifacts.get_in(tx, str(representation["artifact_id"]))
            uses = (
                (
                    await tx.connection.execute(
                        sa.select(schema.uses.c.selections)
                        .join(schema.revisions, schema.uses.c.revision_id == schema.revisions.c.id)
                        .where(schema.revisions.c.work_id == work["id"])
                        .order_by(schema.uses.c.recorded_at.desc(), schema.uses.c.id.desc())
                        .limit(max(window, 1))
                    )
                )
                .scalars()
                .all()
            )
        state = WorkState.model_validate(revision["state"])
        keys = ["objective"]
        if state.focus:
            keys.append("focus")
        choices = ["decision:" + item.key for item in state.decisions if item.state == "active"]
        choices.extend("question:" + item.key for item in state.questions if item.state == "open")
        keys.extend(choices[: max(0, window - len(keys))])
        ranges = representation["fields"]
        fields = tuple(
            WorkContextField(
                key=key,
                selection=ContentSelection(
                    artifact=artifact.ref,
                    selection=SourceSelection(start_byte=ranges[key][0], end_byte=ranges[key][1]),
                ),
            )
            for key in keys
        )
        active = (
            *state.active,
            *(basis for item in state.decisions if item.state == "active" for basis in item.basis),
        )[:window]
        adapter = TypeAdapter(tuple[MaterialSelection, ...])
        return WorkContext(
            ref=ref,
            fields=fields,
            active=active,
            recent=tuple(adapter.validate_python(item) for item in uses),
        )

    async def _prepare_content(
        self, state: WorkState, body: str, language: str
    ) -> tuple[PublishedContent, PublishedContent, dict[str, tuple[int, int]]]:
        published = await self.artifacts.content.publish_bytes(body.encode("utf-8"))
        rendered, fields = render_work(state, body, language)
        rendered_content = await self.artifacts.content.publish_bytes(rendered)
        return published, rendered_content, fields

    async def _insert_revision_in(
        self,
        tx: WriteTx,
        *,
        work_id: str,
        revision_id: str,
        ordinal: int,
        parent_id: str | None,
        state: WorkState,
        body: PublishedContent,
        rendered: PublishedContent,
        fields: dict[str, tuple[int, int]],
        recorded_at: datetime,
    ) -> None:
        artifact = await self.artifacts.register_in(
            tx,
            body,
            media_type="text/markdown",
            display_name="work.md",
            source=None,
            received_at=None,
            formed_at=recorded_at,
        )
        await tx.connection.execute(
            schema.revisions.insert().values(
                id=revision_id,
                work_id=work_id,
                ordinal=ordinal,
                parent_id=parent_id,
                state=state.model_dump(mode="json"),
                body_artifact_id=artifact.ref.id,
                recorded_at=recorded_at.isoformat(),
            )
        )
        rendered_artifact = await self.artifacts.register_in(
            tx,
            rendered,
            media_type="text/markdown",
            display_name="work-view.md",
            source=None,
            received_at=None,
            formed_at=recorded_at,
        )
        await tx.connection.execute(
            schema.representations.insert().values(
                revision_id=revision_id, artifact_id=rendered_artifact.ref.id, fields=fields
            )
        )
        await self._retain_results_in(
            tx, ((revision_id, item) for _, item in self.reader.references(state))
        )

    @staticmethod
    async def _retain_results_in(
        tx: WriteTx, values: Iterable[tuple[str, MaterialSelection]]
    ) -> None:
        dependencies = {
            (revision_id, item.material.result.id)
            for revision_id, item in values
            if isinstance(item.material, ResultOutputRef)
        }
        if dependencies:
            await tx.connection.execute(
                insert(schema.result_dependencies).on_conflict_do_nothing(),
                [
                    {"revision_id": revision, "result_id": result}
                    for revision, result in dependencies
                ],
            )

    async def create(self, request: CreateWork, call: CallContext) -> WorkView:
        async with self.execution.operation("work.create"):
            namespace = await self.scope.resolve_for(request.namespace)
            await self._validate_state(request.state, namespace)
            published, rendered_content, fields = await self._prepare_content(
                request.state,
                request.body,
                select_language(configured=call.settings.values.localization.language),
            )
            work_id, revision_id = str(uuid4()), str(uuid4())
            now = self.clock.utc_now()
            async with self.database.write() as tx:
                await self.scope.get_in(tx, namespace)
                identifier = await insert_identifier(
                    tx,
                    schema.works,
                    "work",
                    {"id": work_id, "namespace_id": namespace.id, "head_revision_id": revision_id},
                )
                await self._insert_revision_in(
                    tx,
                    work_id=work_id,
                    revision_id=revision_id,
                    ordinal=1,
                    parent_id=None,
                    state=request.state,
                    body=published,
                    rendered=rendered_content,
                    fields=fields,
                    recorded_at=now,
                )
                if self.execution.snapshot.values.cognition.search.include_work:
                    await self.scope.advance_in(tx, namespace.id)
                ref = WorkRef(
                    instance_id=self.scope.instance_id,
                    namespace_id=namespace.id,
                    id=work_id,
                    identifier=identifier,
                )
                work, revision, head = await self.reader.resolve_in(tx, ref, namespace)
            result = await self.reader.view(work, revision, head)
            await call.saved(result.revision_ref)
            return result

    async def _revise(
        self,
        target: str | WorkRef | WorkRevisionRef,
        expected: str | int | WorkRevisionRef | None,
        state: WorkState,
        body: str | None,
        namespace: NamespaceRef,
    ) -> WorkView:
        await self._validate_state(state, namespace)
        if body is None:
            async with self.database.read() as tx:
                work, chosen, _ = await self.reader.resolve_in(tx, target, namespace)
                expected_target = expected if expected is not None else target
                if isinstance(expected_target, str) and expected_target.isdecimal():
                    expected_target = int(expected_target)
                if isinstance(expected_target, int):
                    expected_target = f"{work['identifier']}@{expected_target}"
                _, base, _ = await self.reader.resolve_in(tx, expected_target, namespace)
                body_artifact = await self.artifacts.get_in(tx, str(base["body_artifact_id"]))
            body = await self.artifacts.read_text(body_artifact.ref)
        published, rendered_content, fields = await self._prepare_content(
            state,
            body,
            select_language(configured=self.execution.snapshot.values.localization.language),
        )
        async with self.database.write() as tx:
            work, chosen, head = await self.reader.resolve_in(tx, target, namespace)
            explicit = (
                isinstance(target, WorkRevisionRef)
                or isinstance(target, str)
                and split_revision(target)[1] is not None
            )
            if expected is None and not explicit:
                raise NousError(
                    "input.expected_required", "Provide an exact expected work revision"
                )
            expected_target = expected if expected is not None else target
            if isinstance(expected_target, str) and expected_target.isdecimal():
                expected_target = int(expected_target)
            if isinstance(expected_target, int):
                expected_target = f"{work['identifier']}@{expected_target}"
            expected_work, base, _ = await self.reader.resolve_in(tx, expected_target, namespace)
            if expected_work["id"] != work["id"] or explicit and chosen["id"] != base["id"]:
                raise NousError(
                    "input.conflicting_expected", "Target and expected work revisions differ"
                )
            if base["id"] != head["id"]:
                raise NousError(
                    "cognition.revision_conflict",
                    "Work changed since the expected revision",
                    details={
                        "current_revision": self.reader.ref(work, head).identifier,
                        "expected_revision": self.reader.ref(work, base).identifier,
                    },
                )
            now, revision_id = self.clock.utc_now(), str(uuid4())
            await self._insert_revision_in(
                tx,
                work_id=str(work["id"]),
                revision_id=revision_id,
                ordinal=int(head["ordinal"]) + 1,
                parent_id=str(head["id"]),
                state=state,
                body=published,
                rendered=rendered_content,
                fields=fields,
                recorded_at=now,
            )
            await tx.connection.execute(
                sa.update(schema.works)
                .where(schema.works.c.id == work["id"])
                .values(head_revision_id=revision_id)
            )
            if self.execution.snapshot.values.cognition.search.include_work:
                await self.scope.advance_in(tx, namespace.id)
            updated, revision, head = await self.reader.resolve_in(
                tx, cast(str, work["identifier"]), namespace
            )
        return await self.reader.view(updated, revision, head)

    async def revise(self, request: ReviseWork, call: CallContext) -> WorkView:
        async with self.execution.operation("work.revise"):
            namespace = await self.reader.namespace(request.namespace, request.target)
            result = await self._revise(
                request.target, request.expected, request.state, request.body, namespace
            )
            await call.saved(result.revision_ref)
            return result

    async def set_active(self, request: SetActiveWork, call: CallContext) -> WorkView:
        async with self.execution.operation("work.set_active"):
            namespace = await self.reader.namespace(request.namespace, request.target)
            async with self.database.read() as tx:
                work, revision, head = await self.reader.resolve_in(tx, request.target, namespace)
            current = await self.reader.view(work, revision, head)
            result = await self._revise(
                request.target,
                request.expected,
                current.state.model_copy(update={"active": request.active}),
                current.body,
                namespace,
            )
            await call.saved(result.revision_ref)
            return result

    async def record_use(self, request: RecordWorkUse, call: CallContext) -> WorkUse:
        async with self.execution.operation("work.record_use"):
            namespace = await self.reader.namespace(request.namespace, request.revision)
            async with self.database.read() as tx:
                work, revision, _ = await self.reader.resolve_in(tx, request.revision, namespace)
                revision_ref = self.reader.ref(work, revision)
            state = WorkState.model_validate(revision["state"])
            if request.decision_key is not None and request.decision_key not in {
                item.key for item in state.decisions
            }:
                raise NousError(
                    "cognition.decision_not_found", "Decision does not exist in this work revision"
                )
            await self.materials.selections(request.selections, namespace=namespace)
            selections = list(request.selections)
            evidence_ref = None
            if request.evidence is not None:
                from nous.cognition.retrieval.adoption import read_saved_evidence

                evidence = await read_saved_evidence(self.results, request.evidence)
                evidence_ref = evidence.ref
                if evidence.namespace != namespace:
                    raise NousError(
                        "cognition.namespace_mismatch", "Evidence belongs to another namespace"
                    )
                for key in request.evidence_keys:
                    source = evidence.source(key)
                    selection = MaterialSelection(material=source.material, selections=source.selections)
                    if selection not in selections:
                        selections.append(selection)
            reading_ref = None
            if request.reading is not None:
                reading = await self.results.get(request.reading)
                reading_ref = reading.ref
                output = next((item for item in reading.outputs if item.name == "reading"), None)
                if output is None or (
                    evidence_ref is not None and evidence_ref not in reading.dependencies
                ):
                    raise NousError(
                        "cognition.invalid_reading", "Reading was not produced from this evidence"
                    )
                async with self.database.read() as tx:
                    await self.materials.validate_sources_in(
                        tx, reading.sources_for("reading"), namespace=namespace
                    )
            use = WorkUse(
                id=str(uuid4()),
                revision=revision_ref,
                evidence=evidence_ref,
                reading=reading_ref,
                selections=tuple(selections),
                purpose=request.purpose,
                decision_key=request.decision_key,
                recorded_at=self.clock.utc_now(),
            )
            async with self.database.write() as tx:
                await self.reader.resolve_in(tx, revision_ref, namespace)
                await self._retain_results_in(
                    tx, ((revision_ref.revision_id, item) for item in selections)
                )
                await tx.connection.execute(
                    schema.uses.insert().values(
                        id=use.id,
                        revision_id=use.revision.revision_id,
                        evidence_id=use.evidence.id if use.evidence else None,
                        reading_id=use.reading.id if use.reading else None,
                        selections=[item.model_dump(mode="json") for item in use.selections],
                        purpose=use.purpose,
                        decision_key=use.decision_key,
                        recorded_at=use.recorded_at.isoformat(),
                    )
                )
            return use

    async def list_uses(self, request: ListWorkUses, call: CallContext) -> tuple[WorkUse, ...]:
        async with self.execution.operation("work.list_uses"):
            namespace = await self.reader.namespace(request.namespace, request.ref)
            exact = isinstance(request.ref, WorkRevisionRef) or (
                isinstance(request.ref, str) and split_revision(request.ref)[1] is not None
            )
            async with self.database.read() as tx:
                work, revision, _ = await self.reader.resolve_in(tx, request.ref, namespace)
                query = (
                    sa.select(schema.uses, schema.revisions.c.ordinal.label("revision_ordinal"))
                    .join(schema.revisions, schema.uses.c.revision_id == schema.revisions.c.id)
                    .where(schema.revisions.c.work_id == work["id"])
                )
                if exact:
                    query = query.where(schema.uses.c.revision_id == revision["id"])
                rows = (
                    (
                        await tx.connection.execute(
                            query.order_by(
                                schema.uses.c.recorded_at.desc(), schema.uses.c.id.desc()
                            ).limit(request.limit)
                        )
                    )
                    .mappings()
                    .all()
                )
                result_ids = tuple(
                    dict.fromkeys(
                        str(row[key])
                        for row in rows
                        for key in ("evidence_id", "reading_id")
                        if row[key] is not None
                    )
                )
                results = {
                    result.id: result for result in await self.results.refs_in(tx, result_ids)
                }
                work_ref = self.reader.ref(work, revision).work_ref
                return tuple(
                    WorkUse(
                        id=str(row["id"]),
                        revision=WorkRevisionRef(
                            work_ref=work_ref,
                            revision_id=str(row["revision_id"]),
                            ordinal=int(row["revision_ordinal"]),
                        ),
                        evidence=results[str(row["evidence_id"])] if row["evidence_id"] else None,
                        reading=results[str(row["reading_id"])] if row["reading_id"] else None,
                        selections=tuple(
                            MaterialSelection.model_validate(item) for item in row["selections"]
                        ),
                        purpose=str(row["purpose"]),
                        decision_key=row["decision_key"],
                        recorded_at=datetime.fromisoformat(str(row["recorded_at"])),
                    )
                    for row in rows
                )
