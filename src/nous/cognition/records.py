"""Written Note/Journal owner: immutable bodies, exact sources and short transactions."""

from collections.abc import Sequence
from typing import TYPE_CHECKING, Literal, TypedDict, Unpack, cast
from uuid import uuid4

import sqlalchemy as sa
from markdown_it import MarkdownIt

from nous.common.address import insert_address
from nous.common.errors import NousError, failure_code, failure_message
from nous.common.operations import add_error_context, exception_context, record_completed
from nous.content import schema as content_schema
from nous.content.artifacts import ArtifactStore
from nous.content.contracts import (
    ArtifactRef,
    ContentSelection,
    InputGroup,
    InputPart,
)
from nous.content.results import PreparedOutput, PreparedOutputs, ResultStore
from nous.runtime.execution import ExecutionManager
from nous.storage.content import PublishedContent
from nous.storage.database import Database, ReadTx, WriteTx
from nous.temporal.domains import TimeService
from nous.temporal.values import CognitiveTimes, Instant, parse_explicit_time

from . import basis, schema
from .contracts import (
    Namespace,
    RecordRevisionRef,
    SavedRecord,
    SourceBinding,
)
from .record_reading import RecordRow, RevisionRow, Target, WrittenRecordReader
from .scope import NamespaceOperations

if TYPE_CHECKING:
    from .annotations import AnnotationOperations
    from .retrieval import CognitiveRetrieval, Mode


class RevisionValues(TypedDict):
    title: str
    purpose: str | None
    author: str | None
    viewpoint: str | None


class CreateMetadata(TypedDict, total=False):
    title: str | None
    purpose: str | None
    author: str | None
    viewpoint: str | None
    sources: Sequence[SourceBinding]
    times: CognitiveTimes | None


class RevisionMetadata(TypedDict, total=False):
    namespace: str | None
    title: str | None
    purpose: str | None
    author: str | None
    viewpoint: str | None
    sources: Sequence[SourceBinding] | None
    times: CognitiveTimes | None
    change_kind: str
    reason: str | None
    origin_result_id: str | None


class WrittenRecords(WrittenRecordReader):
    def __init__(
        self,
        database: Database,
        execution: ExecutionManager,
        artifacts: ArtifactStore,
        time: TimeService,
        scope: NamespaceOperations,
        results: ResultStore,
        annotations: AnnotationOperations,
        retrieval: CognitiveRetrieval,
    ):
        super().__init__(database, execution, artifacts, scope)
        self.time, self.annotations = time, annotations
        self.results, self.retrieval = results, retrieval

    @staticmethod
    def validate_body(body: str, sources: Sequence[SourceBinding]) -> str | None:
        if not body.strip():
            raise NousError("input.empty_body", "Write some Markdown text before saving")
        keys = [source.key for source in sources]
        if len(set(keys)) != len(keys):
            raise NousError("input.duplicate_source", "Source keys must be unique")
        tokens = MarkdownIt("commonmark").parse(body)
        title = None
        for index, token in enumerate(tokens):
            if title is None and token.type == "heading_open" and token.tag == "h1":
                title = tokens[index + 1].content
            for child in token.children or ():
                href = child.attrGet("href") if child.type == "link_open" else None
                if isinstance(href, str) and href.startswith("source:") and href[7:] not in keys:
                    raise NousError(
                        "cognition.source_unknown",
                        "Markdown source link has no binding",
                        details={"key": href[7:], "lines": token.map},
                    )
        return title

    async def validate_sources_in(
        self, tx: ReadTx, namespace: Namespace, sources: Sequence[SourceBinding]
    ) -> None:
        for source in sources:
            artifact = await self.artifacts.get_in(tx, source.content_selection.artifact)
            if source.owner_revision:
                _, revision, _ = await self.resolve_in(tx, source.owner_revision, namespace.id)
                if revision["body_artifact_id"] != artifact.ref.id:
                    raise NousError(
                        "cognition.source_mismatch",
                        "Source content does not match its exact owner revision",
                    )

    async def prepare_times(self, times: CognitiveTimes | None) -> CognitiveTimes:
        now = parse_explicit_time(self.time.clock.utc_now().isoformat())
        assert isinstance(now, Instant)
        result = (times or CognitiveTimes()).model_copy(
            update={
                "recorded": now.at,
                "formed": times.formed if times and times.formed is not None else now,
            }
        )
        await self.time.validate([result])
        return result

    async def create_in(
        self,
        tx: WriteTx,
        namespace: str,
        body: str,
        published: PublishedContent,
        *,
        kind: Literal["note", "journal"],
        title: str | None = None,
        purpose: str | None = None,
        author: str | None = None,
        viewpoint: str | None = None,
        sources: Sequence[SourceBinding] = (),
        times: CognitiveTimes,
        recall: bool = True,
        origin_result_id: str | None = None,
        body_artifact: ArtifactRef | None = None,
    ) -> SavedRecord:
        ns = await self.scope.get_in(tx, namespace)
        resolved_title = (
            title
            or self.validate_body(body, sources)
            or ("未命名笔记" if kind == "note" else "未命名日记")
        )
        self.validate_body(body, sources)
        await self.validate_sources_in(tx, ns, sources)
        identifier, revision_id = str(uuid4()), str(uuid4())
        address = await insert_address(
            tx,
            schema.records,
            "note" if kind == "note" else "jrn",
            {
                "id": identifier,
                "namespace_id": ns.id,
                "kind": kind,
                "head_revision_id": revision_id,
                "archived": False,
                "recall": recall,
            },
        )
        row: RecordRow = {
            "id": identifier,
            "namespace_id": ns.id,
            "kind": kind,
            "address": address,
            "head_revision_id": revision_id,
            "archived": False,
            "recall": recall,
        }
        revision = await self._insert_revision_in(
            tx,
            row,
            revision_id,
            1,
            None,
            published,
            title=resolved_title,
            purpose=purpose,
            author=author,
            viewpoint=viewpoint,
            times=times,
            sources=sources,
            change_kind="create",
            reason=None,
            origin_result_id=origin_result_id,
            body_artifact=body_artifact,
        )
        if recall:
            ns = await self.scope.advance_in(tx, ns.id)
        return self._saved(row, revision, ns)

    async def manual_inputs_in(
        self,
        tx: ReadTx,
        sources: Sequence[SourceBinding],
        *,
        parent_id: str | None,
        origin_result_id: str | None,
    ) -> tuple[InputGroup, ...]:
        """Keep source support and editing lineage distinct in content provenance."""
        inputs = [
            InputGroup(
                key=source.key,
                name=source.name or source.key,
                role=source.usage,
                parts=(InputPart(content=source.content_selection),),
            )
            for source in sources
        ]
        bases: list[str] = []
        if parent_id:
            body = await tx.connection.scalar(
                sa.select(schema.revisions.c.body_artifact_id).where(
                    schema.revisions.c.id == parent_id
                )
            )
            if body is not None:
                bases.append(str(body))
        if origin_result_id:
            bases.extend(
                str(value)
                for value in (
                    await tx.connection.scalars(
                        sa.select(content_schema.outputs.c.artifact_id).where(
                            content_schema.outputs.c.result_id == origin_result_id,
                            content_schema.outputs.c.name == "output",
                        )
                    )
                ).all()
            )
        keys = {group.key for group in inputs}
        for body in dict.fromkeys(bases):
            artifact = await self.artifacts.get_in(tx, body)
            index = len(inputs) + 1
            key = f"R{index}"
            while key in keys:
                index += 1
                key = f"R{index}"
            keys.add(key)
            inputs.append(
                InputGroup(
                    key=key,
                    name=artifact.display_name,
                    role="revision_base",
                    parts=(InputPart(content=ContentSelection(artifact=artifact.ref)),),
                )
            )
        return tuple(inputs)

    async def _insert_revision_in(
        self,
        tx: WriteTx,
        row: RecordRow,
        revision_id: str,
        ordinal: int,
        parent_id: str | None,
        published: PublishedContent,
        *,
        title: str,
        purpose: str | None,
        author: str | None,
        viewpoint: str | None,
        times: CognitiveTimes,
        sources: Sequence[SourceBinding],
        change_kind: str,
        reason: str | None,
        origin_result_id: str | None,
        body_artifact: ArtifactRef | None = None,
    ) -> RevisionRow:
        if body_artifact is not None:
            artifact = await self.artifacts.get_in(tx, body_artifact)
            blob = await self.artifacts.content.resolve_in(tx, artifact.ref.id)
            if blob != published.blob:
                raise NousError(
                    "cognition.body_mismatch", "Prepared body does not match its immutable artifact"
                )
        else:
            inputs = await self.manual_inputs_in(
                tx, sources, parent_id=parent_id, origin_result_id=origin_result_id
            )
            if inputs:
                result = await self.results.adopt_in(
                    tx,
                    operation="cognition.manual_write",
                    producer={"kind": "manual", "author": author},
                    inputs=inputs,
                    prepared_outputs=PreparedOutputs(
                        (PreparedOutput("output", published, "text/markdown", title + ".md"),),
                        self.time.clock.utc_now(),
                    ),
                    parameters={"change_kind": change_kind, "reason": reason},
                )
                artifact = await self.artifacts.get_in(tx, result.outputs[0].artifact)
                origin_result_id = result.ref.id
            else:
                artifact = await self.artifacts.register_in(
                    tx,
                    published,
                    media_type="text/markdown",
                    display_name=title + ".md",
                    source=None,
                    received_at=None,
                    formed_at=self.time.clock.utc_now(),
                )
        recorded_at = self.time.clock.utc_now()
        recorded = parse_explicit_time(recorded_at.isoformat())
        assert isinstance(recorded, Instant)
        times = times.model_copy(update={"recorded": recorded.at})
        revision: RevisionRow = {
            "id": revision_id,
            "record_id": row["id"],
            "ordinal": ordinal,
            "parent_id": parent_id,
            "title": title,
            "body_artifact_id": artifact.ref.id,
            "purpose": purpose,
            "author": author,
            "viewpoint": viewpoint,
            "cognitive_times": times.model_dump(mode="json"),
            "change_kind": change_kind,
            "reason": reason,
            "origin_result_id": origin_result_id,
            "recorded_at": recorded_at.isoformat(),
        }
        await tx.connection.execute(sa.insert(schema.revisions).values(**revision))
        for index, source in enumerate(sources):
            await tx.connection.execute(
                sa.insert(schema.sources).values(
                    revision_id=revision_id,
                    key=source.key,
                    ordinal=index,
                    artifact_id=source.content_selection.artifact.id,
                    selection=source.content_selection.selection.model_dump(mode="json"),
                    target_revision_id=source.owner_revision.revision_id
                    if source.owner_revision
                    else None,
                    usage=source.usage,
                    name=source.name,
                )
            )
        return revision

    async def _create(
        self,
        namespace: str | None,
        body: str | ArtifactRef,
        *,
        kind: Literal["note", "journal"],
        title: str | None = None,
        purpose: str | None = None,
        author: str | None = None,
        viewpoint: str | None = None,
        sources: Sequence[SourceBinding] = (),
        times: CognitiveTimes | None = None,
        recall: bool = True,
        refresh: bool = False,
        refresh_mode: Mode | None = None,
    ) -> SavedRecord:
        async with self.execution.operation("records.create"):
            if isinstance(body, ArtifactRef):
                body = await self.artifacts.read_text(body)
            self.validate_body(body, sources)
            prepared_times = await self.prepare_times(times)
            published = await self.artifacts.content.publish_bytes(body.encode("utf-8"))
            async with self.database.write() as tx:
                saved = await self.create_in(
                    tx,
                    self._namespace(namespace),
                    body,
                    published,
                    kind=kind,
                    title=title,
                    purpose=purpose,
                    author=author,
                    viewpoint=viewpoint,
                    sources=sources,
                    times=prepared_times,
                    recall=recall,
                )
            record_completed(saved.revision_ref.address)
            return await self.complete_save(saved, refresh=refresh, refresh_mode=refresh_mode)

    async def record_note(
        self,
        namespace: str | None,
        body: str | ArtifactRef,
        *,
        remember: bool | None = None,
        refresh: bool = False,
        refresh_mode: Mode | None = None,
        **metadata: Unpack[CreateMetadata],
    ) -> SavedRecord:
        async with self.execution.operation("note.record"):
            recall = (
                self.execution.snapshot.typed_values().cognition.notes.default_recall
                if remember is None
                else remember
            )
            return await self._create(
                namespace,
                body,
                kind="note",
                recall=recall,
                refresh=refresh,
                refresh_mode=refresh_mode,
                **metadata,
            )

    async def create_journal(
        self,
        namespace: str | None,
        body: str | ArtifactRef,
        *,
        refresh: bool = False,
        refresh_mode: Mode | None = None,
        **metadata: Unpack[CreateMetadata],
    ) -> SavedRecord:
        metadata.setdefault("purpose", "record")
        return await self._create(
            namespace, body, kind="journal", refresh=refresh, refresh_mode=refresh_mode, **metadata
        )

    @staticmethod
    def check_expected(
        row: RecordRow,
        revision: RevisionRow,
        expected: str | int | RecordRevisionRef | None,
        body: str | None = None,
    ) -> None:
        if isinstance(expected, RecordRevisionRef):
            matches = (
                expected.revision_id == row["head_revision_id"]
                and expected.record_ref.id == row["id"]
                and expected.ordinal == revision["ordinal"]
            )
        else:
            matches = expected is not None and (
                expected == revision["ordinal"]
                or expected == revision["id"]
                or expected == f"{row['address']}@{revision['ordinal']}"
            )
        if not matches or row["head_revision_id"] != revision["id"]:
            raise NousError(
                "cognition.revision_conflict",
                "Record has changed; keep the edited text and use the current revision as a new explicit base",
                details={
                    "current_revision": f"{row['address']}@{revision['ordinal']}",
                    "edited_body": body,
                },
            )

    async def append_in(
        self,
        tx: WriteTx,
        target: Target,
        expected_revision: str | int | RecordRevisionRef,
        body: str,
        published: PublishedContent,
        *,
        namespace: str | None = None,
        title: str | None = None,
        purpose: str | None = None,
        author: str | None = None,
        viewpoint: str | None = None,
        sources: Sequence[SourceBinding] | None = None,
        times: CognitiveTimes | None = None,
        change_kind: str = "edit",
        reason: str | None = None,
        origin_result_id: str | None = None,
        body_artifact: ArtifactRef | None = None,
    ) -> SavedRecord:
        row, old, ns = await self.resolve_in(tx, target, namespace)
        head = (
            (
                await tx.connection.execute(
                    sa.select(schema.revisions).where(
                        schema.revisions.c.id == row["head_revision_id"]
                    )
                )
            )
            .mappings()
            .one()
        )
        self.check_expected(row, cast(RevisionRow, dict(head)), expected_revision, body)
        if old["id"] != head["id"]:
            self.check_expected(row, cast(RevisionRow, dict(head)), old["id"], body)
        bindings = await self.sources_in(tx, old["id"]) if sources is None else tuple(sources)
        self.validate_body(body, bindings)
        await self.validate_sources_in(tx, ns, bindings)
        values: RevisionValues = {
            "title": title if title is not None else old["title"],
            "purpose": purpose if purpose is not None else old["purpose"],
            "author": author if author is not None else old["author"],
            "viewpoint": viewpoint if viewpoint is not None else old["viewpoint"],
        }
        old_blob = await self.artifacts.content.resolve_in(tx, old["body_artifact_id"])
        previous_times = CognitiveTimes.model_validate(old["cognitive_times"])
        effective_times = times or previous_times
        equal_times = effective_times.model_dump(exclude={"recorded"}) == previous_times.model_dump(
            exclude={"recorded"}
        )
        if (
            old_blob.digest == published.blob.digest
            and (values["title"], values["purpose"], values["author"], values["viewpoint"])
            == (old["title"], old["purpose"], old["author"], old["viewpoint"])
            and bindings == await self.sources_in(tx, old["id"])
            and equal_times
            and origin_result_id is None
        ):
            return self._saved(row, old, ns, False)
        now = parse_explicit_time(self.time.clock.utc_now().isoformat())
        assert isinstance(now, Instant)
        effective_times = effective_times.model_copy(update={"recorded": now.at})
        revision_id = str(uuid4())
        revision = await self._insert_revision_in(
            tx,
            row,
            revision_id,
            head["ordinal"] + 1,
            head["id"],
            published,
            **values,
            times=effective_times,
            sources=bindings,
            change_kind=change_kind,
            reason=reason,
            origin_result_id=origin_result_id,
            body_artifact=body_artifact,
        )
        await tx.connection.execute(
            sa.update(schema.records)
            .where(schema.records.c.id == row["id"])
            .values(head_revision_id=revision_id)
        )
        affects_retrieval = bool(row["recall"] and not row["archived"])
        if change_kind == "correction":
            affects_retrieval = (
                await basis.invalidate_in(
                    tx, old["id"], revision_id, reason, self.time.clock.utc_now().isoformat()
                )
                or affects_retrieval
            )
        if affects_retrieval:
            ns = await self.scope.advance_in(tx, ns.id)
        return self._saved(row, revision, ns)

    async def revise(
        self,
        target: Target,
        expected_revision: str | int | RecordRevisionRef,
        body: str,
        *,
        refresh: bool = False,
        refresh_mode: Mode | None = None,
        **metadata: Unpack[RevisionMetadata],
    ) -> SavedRecord:
        async with self.execution.operation("records.revise"):
            times = metadata.get("times")
            if times:
                await self.time.validate([times])
            published = await self.artifacts.content.publish_bytes(body.encode("utf-8"))
            async with self.database.write() as tx:
                saved = await self.append_in(
                    tx, target, expected_revision, body, published, **metadata
                )
            record_completed(saved.revision_ref.address)
            return await self.complete_save(saved, refresh=refresh, refresh_mode=refresh_mode)

    async def _membership(
        self,
        target: Target,
        expected_revision: str | int | RecordRevisionRef,
        *,
        namespace: str | None = None,
        archived: bool | None = None,
        recall: bool | None = None,
    ) -> SavedRecord:
        async with self.execution.operation("records.membership"):
            async with self.database.write() as tx:
                row, revision, ns = await self.resolve_in(tx, target, namespace)
                head = (
                    (
                        await tx.connection.execute(
                            sa.select(schema.revisions).where(
                                schema.revisions.c.id == row["head_revision_id"]
                            )
                        )
                    )
                    .mappings()
                    .one()
                )
                self.check_expected(row, cast(RevisionRow, dict(head)), expected_revision)
                if revision["id"] != head["id"]:
                    self.check_expected(row, cast(RevisionRow, dict(head)), revision["id"])
                if recall is not None and row["kind"] != "note":
                    raise NousError(
                        "input.invalid_kind", "Recall membership can be changed only for Note"
                    )
                changed = {
                    key: value
                    for key, value in {"archived": archived, "recall": recall}.items()
                    if value is not None and row[key] != value
                }
                updated = cast(RecordRow, dict(row) | changed)
                if changed:
                    await tx.connection.execute(
                        sa.update(schema.records)
                        .where(schema.records.c.id == row["id"])
                        .values(**changed)
                    )
                    if (row["recall"] and not row["archived"]) != (
                        updated["recall"] and not updated["archived"]
                    ):
                        ns = await self.scope.advance_in(tx, ns.id)
                return self._saved(updated, revision, ns, bool(changed))

    async def archive(
        self,
        target: Target,
        expected_revision: str | int | RecordRevisionRef,
        *,
        namespace: str | None = None,
    ) -> SavedRecord:
        return await self._membership(target, expected_revision, namespace=namespace, archived=True)

    async def restore(
        self,
        target: Target,
        expected_revision: str | int | RecordRevisionRef,
        *,
        namespace: str | None = None,
    ) -> SavedRecord:
        return await self._membership(
            target, expected_revision, namespace=namespace, archived=False
        )

    async def set_recall(
        self,
        target: Target,
        expected_revision: str | int | RecordRevisionRef,
        enabled: bool,
        *,
        namespace: str | None = None,
    ) -> SavedRecord:
        return await self._membership(
            target, expected_revision, namespace=namespace, recall=enabled
        )

    @staticmethod
    def _followup_error(error: Exception) -> NousError:
        if isinstance(error, NousError):
            return error
        context = exception_context(error)
        return NousError(
            failure_code(error),
            failure_message(error),
            details=context,
            operation_id=cast(str | None, context.get("operation_id")),
            trace_id=cast(str | None, context.get("trace_id")),
            cause=error,
        )

    async def complete_save(
        self, saved: SavedRecord, *, refresh: bool = False, refresh_mode: Mode | None = None
    ) -> SavedRecord:
        """Run requested follow-up work after commit and preserve its independent outcome."""
        async with self.execution.operation("records.complete_save"):
            if (
                saved.changed
                and self.execution.snapshot.typed_values().cognition.annotation.on_save
            ):
                try:
                    annotation = await self.annotations.annotate(saved.revision_ref)
                    from .concept_contracts import AnnotationView

                    assert isinstance(annotation, AnnotationView)
                    saved = saved.model_copy(
                        update={
                            "annotation": "adopted",
                            "annotation_ref": annotation.ref.address,
                            "annotation_result": annotation.proposal_result.address,
                            "generation": annotation.generation,
                            "namespace": saved.namespace.model_copy(
                                update={"content_generation": annotation.generation}
                            ),
                        }
                    )
                except Exception as error:
                    failure = self._followup_error(error)
                    saved = saved.model_copy(
                        update={
                            "status": "partial",
                            "annotation": "failed",
                            "annotation_error": failure.to_dict(),
                        }
                    )
            if not (refresh or refresh_mode is not None) or saved.retrieval_state == "not_in_scope":
                return saved
            try:
                published = await self.retrieval.refresh(saved.ref.namespace_id, mode=refresh_mode)
                covered = cast(int, published["covered_generation"])
                async with self.database.read() as tx:
                    namespace = await self.scope.get_in(tx, saved.ref.namespace_id)
                current = published.get("current", True) and covered == namespace.content_generation
                snapshot = cast(dict[str, str], published["snapshot"])["address"]
                return saved.model_copy(
                    update={
                        "retrieval_state": "ready" if current else "pending",
                        "indexed": bool(current),
                        "retrieval_snapshot": snapshot,
                        "covered_generation": covered,
                        "generation": namespace.content_generation,
                        "namespace": namespace,
                    }
                )
            except Exception as error:
                failure = self._followup_error(error)
                add_error_context(
                    failure,
                    {
                        "saved": True,
                        "saved_revision": saved.revision_ref.address,
                        "retry": "journal refresh --namespace " + saved.namespace.name,
                    },
                )
                return saved.model_copy(
                    update={
                        "status": "partial",
                        "retrieval_state": "failed",
                        "indexed": False,
                        "retrieval_error": failure.to_dict(),
                    }
                )
