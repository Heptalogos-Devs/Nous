"""Written Note/Journal owner: immutable bodies, exact sources and short transactions."""

from collections.abc import Sequence
from dataclasses import dataclass
from typing import TYPE_CHECKING, Literal, TypedDict, cast
from uuid import uuid4

import sqlalchemy as sa

from nous.common.errors import NousError
from nous.common.identifier import insert_identifier, split_revision
from nous.content import schema as content_schema
from nous.content.artifacts import ArtifactStore
from nous.content.citations import parse_citations
from nous.content.contracts import (
    ArtifactRef,
    ContentSelection,
    InputGroup,
    InputPart,
)
from nous.content.results import PreparedOutput, PreparedOutputs, ResultStore
from nous.storage.content import PublishedContent
from nous.storage.database import Database, ReadTx, WriteTx
from nous.temporal.domains import TimeService
from nous.temporal.values import CognitiveTimes, Instant, parse_explicit_time

from .. import basis, schema
from ..contracts import (
    Namespace,
    RecordRevisionRef,
    SourceBinding,
    TitleSource,
)
from ..scope import NamespaceOperations
from .contracts import CommittedRecord
from .reading import RecordReader, RecordRow, RevisionRow, Target

if TYPE_CHECKING:
    from ..sources import SourceResolver


class RevisionValues(TypedDict):
    title: str | None
    title_source: TitleSource
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
    title: str | None
    purpose: str | None
    author: str | None
    viewpoint: str | None
    times: CognitiveTimes | None


@dataclass(frozen=True)
class PreparedRevision:
    """A checked exact-base change and the membership observed with it."""

    row: RecordRow
    previous: RevisionRow
    namespace: Namespace
    bindings: tuple[SourceBinding, ...]
    values: RevisionValues
    times: CognitiveTimes
    changed: bool


class WrittenRecords:
    """Own immutable record revisions and membership changes within caller transactions.

    The caller publishes body bytes before write operations and owns commit or
    rollback. Use RecordOperations for the complete public write and follow-up.
    """

    def __init__(
        self,
        database: Database,
        artifacts: ArtifactStore,
        time: TimeService,
        scope: NamespaceOperations,
        results: ResultStore,
        reader: RecordReader,
        sources: SourceResolver,
    ):
        self.database, self.artifacts, self.time = database, artifacts, time
        self.scope, self.results, self.reader, self.sources = scope, results, reader, sources

    @staticmethod
    def resolve_title(
        title: str | None, heading: str | None, previous: RevisionRow | None = None
    ) -> tuple[str | None, TitleSource]:
        """Choose an explicit title, retained user title, or body heading in that order."""
        if title is not None:
            return title, "user"
        if previous is not None and previous["title_source"] == "user":
            return previous["title"], "user"
        return heading, "heading" if heading is not None else "none"

    @staticmethod
    def validate_body(body: str, sources: Sequence[SourceBinding]) -> str | None:
        """Validate nonempty Markdown and its source keys; return its parsed heading.

        Raises:
            NousError: The body is empty, source keys repeat, or a citation has no
                corresponding source binding.
        """
        if not body.strip():
            raise NousError("input.empty_body", "Write some Markdown text before saving")
        keys = [source.key for source in sources]
        if len(set(keys)) != len(keys):
            raise NousError("input.duplicate_source", "Source keys must be unique")
        parsed = parse_citations(body)
        for citation in parsed.citations:
            if citation.key not in keys:
                raise NousError(
                    "cognition.source_unknown",
                    "Markdown source link has no binding",
                    details={"key": citation.key},
                )
        return parsed.title

    async def validate_sources_in(
        self, tx: ReadTx, namespace: Namespace, sources: Sequence[SourceBinding]
    ) -> None:
        """Validate source artifacts and exact owner revisions in the caller's read view."""
        await self.sources.validate_in(tx, namespace.ref, sources)

    async def prepare_times(self, times: CognitiveTimes | None) -> CognitiveTimes:
        """Set the recorded time and validate cognitive time coordinates."""
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
    ) -> CommittedRecord:
        """Create a record and initial revision in the caller's write transaction.

        ``published`` must contain the already-published UTF-8 body. The caller owns
        commit and any subsequent annotation or retrieval maintenance.
        """
        ns = await self.scope.get_in(tx, namespace)
        resolved_title, title_source = self.resolve_title(title, self.validate_body(body, sources))
        await self.validate_sources_in(tx, ns, sources)
        object_id, revision_id = str(uuid4()), str(uuid4())
        identifier = await insert_identifier(
            tx,
            schema.records,
            "note" if kind == "note" else "jrn",
            {
                "id": object_id,
                "namespace_id": ns.id,
                "kind": kind,
                "head_revision_id": revision_id,
                "archived": False,
                "recall": recall,
            },
        )
        row: RecordRow = {
            "id": object_id,
            "namespace_id": ns.id,
            "kind": kind,
            "identifier": identifier,
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
            title_source=title_source,
            purpose=purpose,
            author=author,
            viewpoint=viewpoint,
            times=times,
            sources=sources,
            change_kind="create",
            reason=None,
            origin_result_id=origin_result_id,
            body_artifact=body_artifact,
            committed_generation=ns.content_generation + int(recall),
        )
        if recall:
            ns = await self.scope.advance_in(tx, ns.id)
        return self.reader.committed(row, revision, ns)

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
                slot_key=source.key,
                name=source.name or source.key,
                role=source.usage,
                parts=tuple(InputPart(content=selection) for selection in source.selections),
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
                    slot_key=key,
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
        title: str | None,
        title_source: TitleSource,
        purpose: str | None,
        author: str | None,
        viewpoint: str | None,
        times: CognitiveTimes,
        sources: Sequence[SourceBinding],
        change_kind: str,
        reason: str | None,
        origin_result_id: str | None,
        body_artifact: ArtifactRef | None = None,
        committed_generation: int,
        write_key: str | None = None,
        write_fingerprint: str | None = None,
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
                        (
                            PreparedOutput(
                                "output", published, "text/markdown", (title or row["kind"]) + ".md"
                            ),
                        ),
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
                    display_name=(title or row["kind"]) + ".md",
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
            "committed_generation": committed_generation,
            "parent_id": parent_id,
            "title": title,
            "title_source": title_source,
            "body_artifact_id": artifact.ref.id,
            "purpose": purpose,
            "author": author,
            "viewpoint": viewpoint,
            "cognitive_times": times.model_dump(mode="json"),
            "change_kind": change_kind,
            "reason": reason,
            "write_key": write_key,
            "write_fingerprint": write_fingerprint,
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
                    target_revision_id=source.owner_revision.revision_id
                    if source.owner_revision
                    else None,
                    usage=source.usage,
                    name=source.name,
                )
            )
            await tx.connection.execute(
                sa.insert(schema.source_targets),
                [
                    {
                        "revision_id": revision_id,
                        "key": source.key,
                        "ordinal": ordinal,
                        "artifact_id": selection.artifact.id,
                        "selection": selection.selection.model_dump(mode="json"),
                    }
                    for ordinal, selection in enumerate(source.selections)
                ],
            )
        return revision

    async def resolve_expected_in(
        self,
        tx: ReadTx,
        target: Target,
        expected: str | int | RecordRevisionRef | None,
        namespace: str | None = None,
        body: str | None = None,
        *,
        require_current: bool = True,
    ) -> tuple[RecordRow, RevisionRow, Namespace]:
        """Resolve the target and its explicit base without changing either identity."""
        row, selected, ns = await self.reader.resolve_in(tx, target, namespace)
        target_exact = isinstance(target, RecordRevisionRef) or (
            isinstance(target, str) and split_revision(target)[1] is not None
        )
        if expected is None:
            if not target_exact:
                raise NousError(
                    "input.expected_required", "Provide an exact revision or expected ordinal"
                )
            expected_row, base = row, selected
        else:
            if isinstance(expected, str) and expected.isdecimal():
                expected = int(expected)
            if isinstance(expected, int):
                if expected < 1:
                    raise NousError("input.invalid", "Expected ordinal must be positive")
                expected_target: str | RecordRevisionRef = f"{row['identifier']}@{expected}"
            else:
                expected_target = expected
                if isinstance(expected, str) and split_revision(expected)[1] is None:
                    raise NousError("input.expected_required", "Expected revision must be exact")
            expected_row, base, _ = await self.reader.resolve_in(tx, expected_target, ns.id)
            if expected_row["id"] != row["id"]:
                raise NousError(
                    "cognition.target_mismatch", "Expected revision belongs to another record"
                )
            if target_exact and selected["id"] != base["id"]:
                raise NousError(
                    "input.conflicting_expected", "Target and expected revisions differ"
                )
        head = cast(
            RevisionRow,
            dict(
                (
                    await tx.connection.execute(
                        sa.select(schema.revisions).where(
                            schema.revisions.c.id == row["head_revision_id"]
                        )
                    )
                )
                .mappings()
                .one()
            ),
        )
        if require_current and base["id"] != head["id"]:
            raise NousError(
                "cognition.revision_conflict",
                "Record has changed; keep the edited text and use the current revision as a new explicit base",
                details={
                    "current_revision": f"{row['identifier']}@{head['ordinal']}",
                    "expected_revision": f"{row['identifier']}@{base['ordinal']}",
                    "edited_body": body,
                },
            )
        return row, base, ns

    async def replay_in(
        self,
        tx: ReadTx,
        target: Target,
        expected: str | int | RecordRevisionRef | None,
        namespace: str,
        write_key: str,
        fingerprint: str,
    ) -> CommittedRecord | None:
        """Resolve a previously committed write without redoing its maintenance."""
        row, _, ns = await self.reader.resolve_in(tx, target, namespace)
        saved = (
            (
                await tx.connection.execute(
                    sa.select(schema.revisions).where(
                        schema.revisions.c.record_id == row["id"],
                        schema.revisions.c.write_key == write_key,
                    )
                )
            )
            .mappings()
            .one_or_none()
        )
        if saved is None:
            return None
        _, base, _ = await self.resolve_expected_in(
            tx, target, expected, namespace, require_current=False
        )
        if saved["parent_id"] != base["id"] or saved["write_fingerprint"] != fingerprint:
            raise NousError(
                "cognition.write_key_conflict",
                "The write key belongs to a different record change",
            )
        return self.reader.committed(row, cast(RevisionRow, dict(saved)), ns, False)

    async def prepare_revision_in(
        self,
        tx: ReadTx,
        target: Target,
        expected_revision: str | int | RecordRevisionRef | None,
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
    ) -> PreparedRevision:
        """Resolve and compare a proposed revision in the caller's consistent view."""
        row, old, ns = await self.resolve_expected_in(
            tx, target, expected_revision, namespace, body
        )
        bindings = (
            await self.reader.sources_in(tx, old["id"]) if sources is None else tuple(sources)
        )
        resolved_title, title_source = self.resolve_title(
            title, self.validate_body(body, bindings), old
        )
        await self.validate_sources_in(tx, ns, bindings)
        values: RevisionValues = {
            "title": resolved_title,
            "title_source": title_source,
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
        unchanged = (
            old_blob.digest == published.blob.digest
            and (
                values["title"],
                values["title_source"],
                values["purpose"],
                values["author"],
                values["viewpoint"],
            )
            == (old["title"], old["title_source"], old["purpose"], old["author"], old["viewpoint"])
            and bindings == await self.reader.sources_in(tx, old["id"])
            and equal_times
        )
        return PreparedRevision(row, old, ns, bindings, values, effective_times, not unchanged)

    async def append_in(
        self,
        tx: WriteTx,
        target: Target,
        expected_revision: str | int | RecordRevisionRef | None,
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
        write_key: str | None = None,
        write_fingerprint: str | None = None,
    ) -> CommittedRecord:
        """Append an exact-base revision in the caller's write transaction.

        The expected revision must still be the head. Omitted sources inherit the base;
        unchanged content and metadata return the existing revision with changed false.
        Corrections also invalidate dependent current-basis relationships. Published
        body bytes are supplied by the caller before opening the transaction.
        """
        prepared = await self.prepare_revision_in(
            tx,
            target,
            expected_revision,
            body,
            published,
            namespace=namespace,
            title=title,
            purpose=purpose,
            author=author,
            viewpoint=viewpoint,
            sources=sources,
            times=times,
        )
        row, old, ns = prepared.row, prepared.previous, prepared.namespace
        if not prepared.changed:
            return self.reader.committed(row, old, ns, False)
        now = parse_explicit_time(self.time.clock.utc_now().isoformat())
        assert isinstance(now, Instant)
        effective_times = prepared.times.model_copy(update={"recorded": now.at})
        revision_id = str(uuid4())
        revision = await self._insert_revision_in(
            tx,
            row,
            revision_id,
            old["ordinal"] + 1,
            old["id"],
            published,
            **prepared.values,
            times=effective_times,
            sources=prepared.bindings,
            change_kind=change_kind,
            reason=reason,
            origin_result_id=origin_result_id,
            body_artifact=body_artifact,
            committed_generation=ns.content_generation,
            write_key=write_key,
            write_fingerprint=write_fingerprint,
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
            revision["committed_generation"] = ns.content_generation
            await tx.connection.execute(
                sa.update(schema.revisions)
                .where(schema.revisions.c.id == revision_id)
                .values(committed_generation=ns.content_generation)
            )
        return self.reader.committed(row, revision, ns)

    async def membership_in(
        self,
        tx: WriteTx,
        target: Target,
        expected_revision: str | int | RecordRevisionRef | None,
        *,
        namespace: str | None = None,
        archived: bool | None = None,
        recall: bool | None = None,
    ) -> CommittedRecord:
        """Update archive or recall membership against the expected current revision.

        The caller owns the write transaction. Recall membership can be changed only
        for a Note. A no-op returns changed false; a change in effective recall
        membership advances the namespace content generation.
        """
        row, revision, ns = await self.resolve_expected_in(tx, target, expected_revision, namespace)
        if recall is not None and row["kind"] != "note":
            raise NousError("input.invalid_kind", "Recall membership can be changed only for Note")
        changed = {
            key: value
            for key, value in {"archived": archived, "recall": recall}.items()
            if value is not None and row[key] != value
        }
        updated = cast(RecordRow, dict(row) | changed)
        if changed:
            await tx.connection.execute(
                sa.update(schema.records).where(schema.records.c.id == row["id"]).values(**changed)
            )
            if (row["recall"] and not row["archived"]) != (
                updated["recall"] and not updated["archived"]
            ):
                ns = await self.scope.advance_in(tx, ns.id)
        return self.reader.committed(updated, revision, ns, bool(changed))
