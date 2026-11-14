"""Read immutable Work revisions and capture their material facts."""

from __future__ import annotations

from datetime import datetime

import sqlalchemy as sa
from sqlalchemy.engine import RowMapping

from nous.application.contracts import CallContext
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
from nous.common.errors import NousError
from nous.common.identifier import split_revision
from nous.content.artifacts import ArtifactStore
from nous.content.contracts import ContentSelection, SourceSelection
from nous.runtime.execution import ExecutionManager
from nous.storage.database import Database, ReadTx
from nous.temporal.values import CognitiveTimes, Instant, parse_explicit_time

from . import schema
from .contracts import ReadWork, WorkAttributes, WorkState, WorkView
from .refs import WorkRef, WorkRevisionRef


class WorkReader:
    """Read exact Work revisions and capture immutable material facts in one transaction."""

    def __init__(
        self,
        database: Database,
        execution: ExecutionManager,
        scope: NamespaceOperations,
        artifacts: ArtifactStore,
    ) -> None:
        self.database, self.execution, self.scope, self.artifacts = (
            database,
            execution,
            scope,
            artifacts,
        )

    @staticmethod
    def material_key(ref: WorkRevisionRef) -> str:
        return "work:" + ref.revision_id

    @staticmethod
    def material_namespace(ref: WorkRevisionRef) -> NamespaceRef:
        return NamespaceRef(instance_id=ref.work_ref.instance_id, id=ref.work_ref.namespace_id)

    @staticmethod
    def references(state: WorkState) -> tuple[tuple[str, MaterialSelection], ...]:
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

    async def resolve_in(
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

    def ref(self, work: RowMapping, revision: RowMapping) -> WorkRevisionRef:
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

    async def view(self, work: RowMapping, revision: RowMapping, head: RowMapping) -> WorkView:
        ref = self.ref(work, revision)
        async with self.database.read() as tx:
            material = (await self.capture_materials_in(tx, (ref,)))[0].view
            artifact = await self.artifacts.get_in(tx, str(revision["body_artifact_id"]))
        attributes = WorkAttributes.model_validate(material.attributes.values)
        assert material.body is not None
        return WorkView(
            ref=ref.work_ref,
            revision_ref=ref,
            current_revision=self.ref(work, head),
            state=attributes.state,
            body_artifact=artifact.ref,
            rendered_artifact=material.body.artifact,
            fields=attributes.fields,
            body=await self.artifacts.read_text(artifact.ref),
            recorded_at=attributes.recorded_at,
        )

    async def namespace(
        self, selector: NamespaceSelector, target: str | WorkRef | WorkRevisionRef
    ) -> NamespaceRef:
        ref = target.work_ref if isinstance(target, WorkRevisionRef) else target
        owner = (
            NamespaceRef(instance_id=ref.instance_id, id=ref.namespace_id)
            if isinstance(ref, WorkRef)
            else None
        )
        return await self.scope.resolve_for(selector, owner)

    async def resolve_identifier(
        self, ref: str, *, namespace: NamespaceSelector = None
    ) -> WorkRevisionRef:
        selected = await self.namespace(namespace, ref)
        async with self.database.read() as tx:
            work, revision, _ = await self.resolve_in(tx, ref, selected)
            return self.ref(work, revision)

    async def read(self, request: ReadWork, call: CallContext) -> WorkView:
        async with self.execution.operation("work.read"):
            namespace = await self.namespace(request.namespace, request.ref)
            async with self.database.read() as tx:
                work, revision, head = await self.resolve_in(tx, request.ref, namespace)
            return await self.view(work, revision, head)

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

            references = self.references(state)
            relations = tuple(
                MaterialRelation(
                    kind=relation_kind(role), target=item.material, evidence=item.selections
                )
                for role, item in references
            )
            material_sources = tuple(
                MaterialSource(
                    key=f"{role}:{index}",
                    material=item.material,
                    selections=item.selections,
                    kind=relation_kind(role),
                )
                for index, (role, item) in enumerate(references)
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
