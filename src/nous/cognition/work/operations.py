"""Adopt and read immutable work revisions and their actual uses."""

from __future__ import annotations

from datetime import datetime
from typing import TYPE_CHECKING, cast
from uuid import uuid4

import sqlalchemy as sa
from pydantic import ValidationError
from sqlalchemy.engine import RowMapping

from nous.application.contracts import CallContext, define_operation
from nous.cognition.contracts import NamespaceRef, NamespaceSelector
from nous.cognition.material_contracts import (
    CognitiveMaterialView,
    MaterialAttributes,
    MaterialIndex,
    MaterialRelation,
    MaterialSection,
    MaterialSelection,
    MaterialSnapshot,
    MaterialSource,
)
from nous.cognition.scope import NamespaceOperations
from nous.cognition.steward.contracts import ReadingBrief
from nous.common.errors import NousError
from nous.common.identifier import insert_identifier, split_revision
from nous.content.artifacts import ArtifactStore
from nous.content.contracts import ContentSelection, SourceSelection
from nous.content.results import ResultStore
from nous.presentation import select_language, translate
from nous.runtime.execution import ExecutionManager
from nous.storage.database import Database, ReadTx, WriteTx
from nous.temporal.values import Clock, CognitiveTimes, Instant, parse_explicit_time

from . import schema
from .contracts import (
    CreateWork,
    ListWorkUses,
    ReadWork,
    RecordWorkUse,
    ReviseWork,
    SetActiveWork,
    WorkAttributes,
    WorkState,
    WorkUse,
    WorkView,
)
from .refs import WorkRef, WorkRevisionRef

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
    ):
        self.database, self.execution, self.scope = database, execution, scope
        self.artifacts, self.results, self.materials, self.clock = (
            artifacts,
            results,
            materials,
            clock,
        )

    @staticmethod
    def material_key(ref: WorkRevisionRef) -> str:
        return "work:" + ref.revision_id

    @staticmethod
    def material_namespace(ref: WorkRevisionRef) -> NamespaceRef:
        return NamespaceRef(instance_id=ref.work_ref.instance_id, id=ref.work_ref.namespace_id)

    @staticmethod
    def _references(state: WorkState) -> tuple[tuple[str, MaterialSelection], ...]:
        return tuple(
            [("active", item) for item in state.active]
            + [("related", MaterialSelection(material=item)) for item in state.related]
            + [
                ("decision:" + decision.key, item)
                for decision in state.decisions
                for item in decision.basis
            ]
            + [
                ("question:" + question.key, item)
                for question in state.questions
                for item in question.references
            ]
        )

    async def _validate_state(self, state: WorkState, namespace: NamespaceRef) -> None:
        values = self._references(state)
        unselected = tuple(item.material for _, item in values if not item.selections)
        if unselected:
            await self.materials.resolve_many(unselected, namespace=namespace)
        selected = tuple(item for _, item in values if item.selections)
        if selected:
            await self.materials.read_many(selected, namespace=namespace)

    async def _insert_references(self, tx: WriteTx, revision_id: str, state: WorkState) -> None:
        rows = [
            {
                "revision_id": revision_id,
                "ordinal": index,
                "role": role,
                "material": item.material.model_dump(mode="json"),
                "selections": [part.model_dump(mode="json") for part in item.selections],
            }
            for index, (role, item) in enumerate(self._references(state))
        ]
        if rows:
            await tx.connection.execute(schema.references.insert(), rows)

    async def _resolve_in(
        self, tx: ReadTx, target: str | WorkRef | WorkRevisionRef, namespace: NamespaceRef
    ) -> tuple[RowMapping, RowMapping, RowMapping]:
        ref = target.work_ref if isinstance(target, WorkRevisionRef) else target
        if isinstance(ref, WorkRef):
            if ref.instance_id != self.scope.instance_id or ref.namespace_id != namespace.id:
                raise NousError(
                    "cognition.namespace_mismatch", "Work belongs to another namespace or instance"
                )
            identity, ordinal = (
                ref.id,
                target.ordinal if isinstance(target, WorkRevisionRef) else None,
            )
        else:
            identity, ordinal = split_revision(ref)
        work = (
            (
                await tx.connection.execute(
                    sa.select(schema.works).where(
                        schema.works.c.namespace_id == namespace.id,
                        sa.or_(
                            schema.works.c.id == identity, schema.works.c.identifier == identity
                        ),
                    )
                )
            )
            .mappings()
            .one_or_none()
        )
        if work is None:
            raise NousError(
                "cognition.work_not_found", "Work does not exist", details={"ref": identity}
            )
        if isinstance(ref, WorkRef) and (
            ref.id != work["id"] or ref.identifier != work["identifier"]
        ):
            raise NousError(
                "cognition.reference_mismatch", "Work reference does not match its identity"
            )
        head = (
            (
                await tx.connection.execute(
                    sa.select(schema.revisions).where(
                        schema.revisions.c.id == work["head_revision_id"]
                    )
                )
            )
            .mappings()
            .one()
        )
        revision = (
            head
            if ordinal is None
            else (
                await tx.connection.execute(
                    sa.select(schema.revisions).where(
                        schema.revisions.c.work_id == work["id"],
                        schema.revisions.c.ordinal == ordinal,
                    )
                )
            )
            .mappings()
            .one_or_none()
        )
        if revision is None:
            raise NousError("cognition.revision_not_found", "Work revision does not exist")
        if isinstance(target, WorkRevisionRef) and target.revision_id != revision["id"]:
            raise NousError("cognition.reference_mismatch", "Work revision identity does not match")
        return work, revision, head

    def _ref(self, work: RowMapping, revision: RowMapping) -> WorkRevisionRef:
        return WorkRevisionRef(
            work_ref=WorkRef(
                instance_id=self.scope.instance_id,
                namespace_id=str(work["namespace_id"]),
                id=str(work["id"]),
                identifier=str(work["identifier"]),
            ),
            revision_id=str(revision["id"]),
            ordinal=int(revision["ordinal"]),
        )

    async def _view(self, work: RowMapping, revision: RowMapping, head: RowMapping) -> WorkView:
        artifact = await self.artifacts.get(str(revision["body_artifact_id"]))
        async with self.database.read() as tx:
            representation = (
                (
                    await tx.connection.execute(
                        sa.select(schema.representations).where(
                            schema.representations.c.revision_id == revision["id"]
                        )
                    )
                )
                .mappings()
                .one_or_none()
            )
            if representation is None:
                raise NousError(
                    "cognition.work_representation_missing",
                    "Work revision has no rendered representation",
                )
            rendered = await self.artifacts.get_in(tx, str(representation["artifact_id"]))
        fields = {
            key: ContentSelection(
                artifact=rendered.ref,
                selection=SourceSelection(start_byte=value[0], end_byte=value[1]),
            )
            for key, value in representation["fields"].items()
        }
        ref = self._ref(work, revision)
        return WorkView(
            ref=ref.work_ref,
            revision_ref=ref,
            current_revision=self._ref(work, head),
            state=WorkState.model_validate(revision["state"]),
            body_artifact=artifact.ref,
            rendered_artifact=rendered.ref,
            fields=fields,
            body=await self.artifacts.read_text(artifact.ref),
            recorded_at=datetime.fromisoformat(str(revision["recorded_at"])),
        )

    async def _namespace(
        self, selector: NamespaceSelector, target: str | WorkRef | WorkRevisionRef
    ) -> NamespaceRef:
        ref = target.work_ref if isinstance(target, WorkRevisionRef) else target
        owner = (
            NamespaceRef(instance_id=ref.instance_id, id=ref.namespace_id)
            if isinstance(ref, WorkRef)
            else None
        )
        return await self.scope.resolve_for(selector, owner)

    async def read_revision(self, ref: WorkRevisionRef) -> WorkView:
        namespace = await self._namespace(None, ref)
        async with self.database.read() as tx:
            work, revision, head = await self._resolve_in(tx, ref, namespace)
        return await self._view(work, revision, head)

    async def resolve_identifier(
        self, ref: str, *, namespace: NamespaceSelector = None
    ) -> WorkRevisionRef:
        selected = await self._namespace(namespace, ref)
        async with self.database.read() as tx:
            work, revision, _ = await self._resolve_in(tx, ref, selected)
            return self._ref(work, revision)

    async def read(self, request: ReadWork, call: CallContext) -> WorkView:
        async with self.execution.operation("work.read"):
            namespace = await self._namespace(request.namespace, request.ref)
            async with self.database.read() as tx:
                work, revision, head = await self._resolve_in(tx, request.ref, namespace)
            return await self._view(work, revision, head)

    async def create(self, request: CreateWork, call: CallContext) -> WorkView:
        async with self.execution.operation("work.create"):
            namespace = await self.scope.resolve_for(request.namespace)
            await self._validate_state(request.state, namespace)
            published = await self.artifacts.content.publish_bytes(request.body.encode("utf-8"))
            rendered_bytes, fields = render_work(
                request.state,
                request.body,
                select_language(configured=call.settings.values.localization.language),
            )
            rendered_content = await self.artifacts.content.publish_bytes(rendered_bytes)
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
                artifact = await self.artifacts.register_in(
                    tx,
                    published,
                    media_type="text/markdown",
                    display_name="work.md",
                    source=None,
                    received_at=None,
                    formed_at=now,
                )
                await tx.connection.execute(
                    schema.revisions.insert().values(
                        id=revision_id,
                        work_id=work_id,
                        ordinal=1,
                        parent_id=None,
                        state=request.state.model_dump(mode="json"),
                        body_artifact_id=artifact.ref.id,
                        recorded_at=now.isoformat(),
                    )
                )
                rendered = await self.artifacts.register_in(
                    tx,
                    rendered_content,
                    media_type="text/markdown",
                    display_name="work-view.md",
                    source=None,
                    received_at=None,
                    formed_at=now,
                )
                await tx.connection.execute(
                    schema.representations.insert().values(
                        revision_id=revision_id, artifact_id=rendered.ref.id, fields=fields
                    )
                )
                await self._insert_references(tx, revision_id, request.state)
                if self.execution.snapshot.values.cognition.search.include_work:
                    await self.scope.advance_in(tx, namespace.id)
                ref = WorkRef(
                    instance_id=self.scope.instance_id,
                    namespace_id=namespace.id,
                    id=work_id,
                    identifier=identifier,
                )
                work, revision, head = await self._resolve_in(tx, ref, namespace)
            result = await self._view(work, revision, head)
            await call.saved(result.revision_ref)
            return result

    async def _revise(
        self,
        target: str | WorkRef | WorkRevisionRef,
        expected: str | int | WorkRevisionRef | None,
        state: WorkState,
        body: str,
        namespace: NamespaceRef,
    ) -> WorkView:
        await self._validate_state(state, namespace)
        published = await self.artifacts.content.publish_bytes(body.encode("utf-8"))
        rendered_bytes, fields = render_work(
            state,
            body,
            select_language(configured=self.execution.snapshot.values.localization.language),
        )
        rendered_content = await self.artifacts.content.publish_bytes(rendered_bytes)
        async with self.database.write() as tx:
            work, chosen, head = await self._resolve_in(tx, target, namespace)
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
            expected_work, base, _ = await self._resolve_in(tx, expected_target, namespace)
            if expected_work["id"] != work["id"] or explicit and chosen["id"] != base["id"]:
                raise NousError(
                    "input.conflicting_expected", "Target and expected work revisions differ"
                )
            if base["id"] != head["id"]:
                raise NousError(
                    "cognition.revision_conflict",
                    "Work changed since the expected revision",
                    details={
                        "current_revision": self._ref(work, head).identifier,
                        "expected_revision": self._ref(work, base).identifier,
                    },
                )
            now, revision_id = self.clock.utc_now(), str(uuid4())
            artifact = await self.artifacts.register_in(
                tx,
                published,
                media_type="text/markdown",
                display_name="work.md",
                source=None,
                received_at=None,
                formed_at=now,
            )
            await tx.connection.execute(
                schema.revisions.insert().values(
                    id=revision_id,
                    work_id=work["id"],
                    ordinal=int(head["ordinal"]) + 1,
                    parent_id=head["id"],
                    state=state.model_dump(mode="json"),
                    body_artifact_id=artifact.ref.id,
                    recorded_at=now.isoformat(),
                )
            )
            rendered = await self.artifacts.register_in(
                tx,
                rendered_content,
                media_type="text/markdown",
                display_name="work-view.md",
                source=None,
                received_at=None,
                formed_at=now,
            )
            await tx.connection.execute(
                schema.representations.insert().values(
                    revision_id=revision_id, artifact_id=rendered.ref.id, fields=fields
                )
            )
            await self._insert_references(tx, revision_id, state)
            await tx.connection.execute(
                sa.update(schema.works)
                .where(schema.works.c.id == work["id"])
                .values(head_revision_id=revision_id)
            )
            if self.execution.snapshot.values.cognition.search.include_work:
                await self.scope.advance_in(tx, namespace.id)
            updated, revision, head = await self._resolve_in(
                tx, cast(str, work["identifier"]), namespace
            )
        return await self._view(updated, revision, head)

    async def capture_materials_in(
        self, tx: ReadTx, refs: tuple[WorkRevisionRef, ...]
    ) -> tuple[MaterialSnapshot, ...]:
        """Capture exact Work facts without reading or parsing content bytes."""
        if not refs:
            return ()
        if any(ref.work_ref.instance_id != self.scope.instance_id for ref in refs):
            raise NousError("cognition.instance_mismatch", "Work belongs to another instance")
        rows = (
            (
                await tx.connection.execute(
                    sa.select(
                        schema.works.c.id.label("work_id"),
                        schema.works.c.namespace_id,
                        schema.works.c.identifier,
                        schema.revisions.c.id.label("revision_id"),
                        schema.revisions.c.ordinal,
                        schema.revisions.c.state,
                        schema.revisions.c.recorded_at,
                        schema.revisions.c.body_artifact_id,
                        schema.representations.c.artifact_id.label("rendered_id"),
                        schema.representations.c.fields,
                    )
                    .join(schema.revisions, schema.revisions.c.work_id == schema.works.c.id)
                    .join(
                        schema.representations,
                        schema.representations.c.revision_id == schema.revisions.c.id,
                    )
                    .where(schema.revisions.c.id.in_([ref.revision_id for ref in refs]))
                )
            )
            .mappings()
            .all()
        )
        by_id = {str(row["revision_id"]): row for row in rows}
        if any(ref.revision_id not in by_id for ref in refs):
            raise NousError("cognition.revision_not_found", "Work revision does not exist")
        artifact_ids = tuple(
            dict.fromkeys(
                str(row[key]) for row in rows for key in ("body_artifact_id", "rendered_id")
            )
        )
        artifacts = {
            item.ref.id: item.ref for item in await self.artifacts.get_many_in(tx, artifact_ids)
        }
        rendered_ids = tuple(dict.fromkeys(str(row["rendered_id"]) for row in rows))
        blobs = await self.artifacts.content.resolve_many_in(tx, rendered_ids)
        digests = {
            identifier: blob.digest for identifier, blob in zip(rendered_ids, blobs, strict=True)
        }
        output: list[MaterialSnapshot] = []
        for expected in refs:
            row = by_id[expected.revision_id]
            actual = WorkRevisionRef(
                work_ref=WorkRef(
                    instance_id=self.scope.instance_id,
                    namespace_id=str(row["namespace_id"]),
                    id=str(row["work_id"]),
                    identifier=str(row["identifier"]),
                ),
                revision_id=str(row["revision_id"]),
                ordinal=int(row["ordinal"]),
            )
            if expected != actual:
                raise NousError(
                    "cognition.reference_mismatch", "Work revision identity does not match"
                )
            state = WorkState.model_validate(row["state"])
            body, rendered = (
                artifacts[str(row["body_artifact_id"])],
                artifacts[str(row["rendered_id"])],
            )
            fields = {
                key: ContentSelection(
                    artifact=rendered,
                    selection=SourceSelection(start_byte=value[0], end_byte=value[1]),
                )
                for key, value in row["fields"].items()
            }
            decision_states: dict[str, str] = {item.key: item.state for item in state.decisions}

            def relation_kind(role: str, states: dict[str, str] = decision_states) -> str:
                kind, _, key = role.partition(":")
                if kind == "decision":
                    return {
                        "active": "current_basis",
                        "superseded": "superseded_basis",
                        "withdrawn": "withdrawn_basis",
                    }[states[key]]
                return kind

            relations = tuple(
                MaterialRelation(
                    kind=relation_kind(role), target=item.material, evidence=item.selections
                )
                for role, item in self._references(state)
            )
            material_sources = tuple(
                MaterialSource(
                    key=f"{role}:{index}",
                    material=item.material,
                    selections=item.selections,
                    kind=relation_kind(role),
                )
                for index, (role, item) in enumerate(self._references(state))
                if item.selections
            )
            if "body" in fields:
                relations += (
                    MaterialRelation(
                        kind="representation",
                        target=body,
                        scope=(fields["body"],),
                        evidence=(ContentSelection(artifact=body),),
                    ),
                )
            recorded_at = datetime.fromisoformat(str(row["recorded_at"]))
            point = parse_explicit_time(recorded_at.isoformat())
            assert isinstance(point, Instant)
            view = CognitiveMaterialView(
                ref=actual,
                kind="work",
                title=state.title,
                body=ContentSelection(artifact=rendered),
                material_sources=material_sources,
                relations=relations,
                times=CognitiveTimes(recorded=point.at),
                attributes=MaterialAttributes(
                    schema_key="work",
                    values=WorkAttributes(
                        state=state, recorded_at=recorded_at, fields=fields
                    ).model_dump(mode="json"),
                ),
            )
            output.append(
                MaterialSnapshot(
                    view=view,
                    body_digest=digests[rendered.id],
                    index=MaterialIndex(
                        key=self.material_key(actual),
                        group_key=actual.identifier,
                        namespace=self.material_namespace(actual),
                        available=True,
                        opt_in="work",
                        support_relations=tuple(
                            relation for relation in relations if relation.kind == "current_basis"
                        ),
                    ),
                )
            )
        return tuple(output)

    async def current_materials_in(
        self, tx: ReadTx, namespace: NamespaceRef
    ) -> tuple[MaterialSnapshot, ...]:
        """Capture current Work revisions in the caller's namespace read view."""
        await self.scope.get_in(tx, namespace)
        rows = (
            (
                await tx.connection.execute(
                    sa.select(
                        schema.works.c.id,
                        schema.works.c.identifier,
                        schema.revisions.c.id.label("revision_id"),
                        schema.revisions.c.ordinal,
                    )
                    .join(
                        schema.revisions, schema.revisions.c.id == schema.works.c.head_revision_id
                    )
                    .where(schema.works.c.namespace_id == namespace.id)
                )
            )
            .mappings()
            .all()
        )
        refs = tuple(
            WorkRevisionRef(
                work_ref=WorkRef(
                    instance_id=namespace.instance_id,
                    namespace_id=namespace.id,
                    id=str(row["id"]),
                    identifier=str(row["identifier"]),
                ),
                revision_id=str(row["revision_id"]),
                ordinal=int(row["ordinal"]),
            )
            for row in rows
        )
        return await self.capture_materials_in(tx, refs)

    async def prepare_material(self, view: CognitiveMaterialView) -> CognitiveMaterialView:
        """Expose the saved Work field ranges without reparsing the body."""
        attributes = WorkAttributes.model_validate(view.attributes.values)
        return view.model_copy(
            update={
                "sections": tuple(
                    MaterialSection(key=key, selection=selection)
                    for key, selection in attributes.fields.items()
                )
            }
        )

    async def revise(self, request: ReviseWork, call: CallContext) -> WorkView:
        async with self.execution.operation("work.revise"):
            namespace = await self._namespace(request.namespace, request.target)
            result = await self._revise(
                request.target, request.expected, request.state, request.body, namespace
            )
            await call.saved(result.revision_ref)
            return result

    async def set_active(self, request: SetActiveWork, call: CallContext) -> WorkView:
        async with self.execution.operation("work.set_active"):
            namespace = await self._namespace(request.namespace, request.target)
            async with self.database.read() as tx:
                work, revision, head = await self._resolve_in(tx, request.target, namespace)
            current = await self._view(work, revision, head)
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
            view = await self.read_revision(request.revision)
            namespace = NamespaceRef(instance_id=view.ref.instance_id, id=view.ref.namespace_id)
            if request.decision_key is not None and request.decision_key not in {
                item.key for item in view.state.decisions
            }:
                raise NousError(
                    "cognition.decision_not_found", "Decision does not exist in this work revision"
                )
            await self.materials.read_many(request.selections, namespace=namespace)
            evidence = await self.results.get(request.evidence)
            if evidence.operation != "retrieval.query":
                raise NousError(
                    "cognition.invalid_evidence", "Result is not a query evidence delivery"
                )
            output = next((item for item in evidence.outputs if item.name == "evidence"), None)
            if output is None:
                raise NousError("cognition.invalid_evidence", "Evidence output is missing")
            from nous.cognition.retrieval.contracts import EvidenceDelivery

            delivery = EvidenceDelivery.model_validate_json(
                await self.artifacts.read_text(output.artifact)
            )
            if delivery.intent.constraints.namespace != namespace:
                raise NousError(
                    "cognition.namespace_mismatch", "Evidence belongs to another namespace"
                )
            if request.reading is not None:
                reading = await self.results.get(request.reading)
                output = next((item for item in reading.outputs if item.name == "reading"), None)
                if request.evidence not in reading.dependencies or output is None:
                    raise NousError(
                        "cognition.invalid_reading", "Reading was not produced from this evidence"
                    )
                try:
                    ReadingBrief.model_validate_json(
                        await self.artifacts.read_text(output.artifact)
                    )
                except ValidationError as error:
                    raise NousError(
                        "cognition.invalid_reading", "Reading output is not a valid reading brief"
                    ) from error
            use = WorkUse(
                id=str(uuid4()),
                revision=request.revision,
                evidence=request.evidence,
                reading=request.reading,
                selections=request.selections,
                purpose=request.purpose,
                decision_key=request.decision_key,
                recorded_at=self.clock.utc_now(),
            )
            async with self.database.write() as tx:
                await self._resolve_in(tx, request.revision, namespace)
                await tx.connection.execute(
                    schema.uses.insert().values(
                        id=use.id,
                        revision_id=use.revision.revision_id,
                        evidence_id=use.evidence.id,
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
            namespace = await self._namespace(request.namespace, request.ref)
            exact = isinstance(request.ref, WorkRevisionRef) or (
                isinstance(request.ref, str) and split_revision(request.ref)[1] is not None
            )
            async with self.database.read() as tx:
                work, revision, _ = await self._resolve_in(tx, request.ref, namespace)
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
                values: list[WorkUse] = []
                for row in rows:
                    _, used_revision, _ = await self._resolve_in(
                        tx, f"{work['identifier']}@{row['revision_ordinal']}", namespace
                    )
                    evidence = await self.results.get_in(tx, str(row["evidence_id"]))
                    reading = (
                        await self.results.get_in(tx, str(row["reading_id"]))
                        if row["reading_id"]
                        else None
                    )
                    values.append(
                        WorkUse(
                            id=str(row["id"]),
                            revision=self._ref(work, used_revision),
                            evidence=evidence.ref,
                            reading=reading.ref if reading else None,
                            selections=tuple(
                                MaterialSelection.model_validate(item) for item in row["selections"]
                            ),
                            purpose=str(row["purpose"]),
                            decision_key=row["decision_key"],
                            recorded_at=datetime.fromisoformat(str(row["recorded_at"])),
                        )
                    )
            return tuple(values)
