"""Capture formal revisions and adopted annotations in the caller's consistent read view."""

from dataclasses import dataclass
from typing import cast

import sqlalchemy as sa

from nous.content.artifacts import ArtifactStore
from nous.content.contracts import ArtifactRef
from nous.storage.database import ReadTx

from .. import basis, concept_schema, schema
from ..annotation.operations import AnnotationOperations
from ..concept_contracts import AnnotationView
from ..contracts import Namespace, RecordRevisionRef, SourceBinding
from ..scope import NamespaceOperations
from ..writing.reading import RecordReader, RecordRow, RevisionRow


@dataclass(frozen=True)
class CapturedRecord:
    record: RecordRow
    revision: RevisionRow
    ref: RecordRevisionRef
    body: ArtifactRef
    sources: tuple[SourceBinding, ...]


@dataclass(frozen=True)
class CapturedCorpus:
    namespace: Namespace
    records: dict[str, CapturedRecord]
    current: tuple[str, ...]
    annotations: dict[str, AnnotationView]
    needs_review: frozenset[str]
    origins: dict[str, ArtifactRef]


@dataclass(frozen=True)
class DirectoryEntry:
    record: RecordRow
    revision: RevisionRow
    ref: RecordRevisionRef
    body_digest: str
    annotation_id: str | None
    needs_review: bool


@dataclass(frozen=True)
class CaptureDirectory:
    namespace: Namespace
    entries: dict[str, DirectoryEntry]
    needs_review_count: int


class CorpusCapture:
    """Capture retrieval inputs from formal records in a caller-owned read view."""

    def __init__(
        self,
        scope: NamespaceOperations,
        records: RecordReader,
        artifacts: ArtifactStore,
        annotations: AnnotationOperations,
    ):
        self.scope, self.records, self.artifacts, self.annotations = (
            scope,
            records,
            artifacts,
            annotations,
        )

    async def directory_in(
        self, tx: ReadTx, namespace: str, *, include_review: bool, include_exploration: bool
    ) -> CaptureDirectory:
        """Read lightweight immutable identities and authoritative eligibility facts."""
        ns = await self.scope.get_in(tx, namespace)
        current_rows = (
            (
                await tx.connection.execute(
                    sa.select(schema.records).where(
                        schema.records.c.namespace_id == ns.id,
                        schema.records.c.archived.is_(False),
                        schema.records.c.recall.is_(True),
                    )
                )
            )
            .mappings()
            .all()
        )
        current = tuple(str(row["head_revision_id"]) for row in current_rows)
        if not current:
            return CaptureDirectory(ns, {}, 0)
        revision_rows = (
            (
                await tx.connection.execute(
                    sa.select(schema.revisions).where(schema.revisions.c.id.in_(current))
                )
            )
            .mappings()
            .all()
        )
        revisions = {str(row["id"]): cast(RevisionRow, dict(row)) for row in revision_rows}
        records = {str(row["id"]): cast(RecordRow, dict(row)) for row in current_rows}
        body_ids = [revisions[revision]["body_artifact_id"] for revision in current]
        blobs = await self.artifacts.content.resolve_many_in(tx, body_ids)
        annotation_rows = (
            (
                await tx.connection.execute(
                    sa.select(concept_schema.annotation_heads).where(
                        concept_schema.annotation_heads.c.record_revision_id.in_(current)
                    )
                )
            )
            .mappings()
            .all()
        )
        annotations = {
            str(row["record_revision_id"]): str(row["annotation_id"]) for row in annotation_rows
        }
        needs_review = await basis.needs_review_in(tx, current)
        entries: dict[str, DirectoryEntry] = {}
        for identifier, blob in zip(current, blobs, strict=True):
            revision = revisions[identifier]
            record = records[revision["record_id"]]
            if (identifier in needs_review and not include_review) or (
                revision["purpose"] == "exploration" and not include_exploration
            ):
                continue
            entries[identifier] = DirectoryEntry(
                record=record,
                revision=revision,
                ref=self.records.revision_ref(record, revision),
                body_digest=blob.digest,
                annotation_id=annotations.get(identifier),
                needs_review=identifier in needs_review,
            )
        return CaptureDirectory(ns, entries, len(needs_review))

    async def expand_in(
        self, tx: ReadTx, directory: CaptureDirectory, needed_keys: tuple[str, ...]
    ) -> CapturedCorpus:
        """Expand selected groups and their exact source ancestors in the same view."""
        ns = directory.namespace
        current = needed_keys
        if not current:
            return CapturedCorpus(ns, {}, (), {}, frozenset(), {})

        ancestors = (
            sa.select(schema.revisions.c.id.label("id"))
            .where(schema.revisions.c.id.in_(current))
            .cte("cognition_ancestors", recursive=True)
        )
        ancestors = ancestors.union(
            sa.select(schema.sources.c.target_revision_id.label("id"))
            .join(ancestors, schema.sources.c.revision_id == ancestors.c.id)
            .where(
                schema.sources.c.target_revision_id.is_not(None),
                schema.sources.c.usage != "inspiration",
            )
        )
        revision_rows = (
            (
                await tx.connection.execute(
                    sa.select(schema.revisions).where(
                        schema.revisions.c.id.in_(sa.select(ancestors.c.id))
                    )
                )
            )
            .mappings()
            .all()
        )
        revisions = {str(row["id"]): cast(RevisionRow, dict(row)) for row in revision_rows}
        record_rows = (
            (
                await tx.connection.execute(
                    sa.select(schema.records).where(
                        schema.records.c.id.in_([row["record_id"] for row in revisions.values()])
                    )
                )
            )
            .mappings()
            .all()
        )
        records = {str(row["id"]): cast(RecordRow, dict(row)) for row in record_rows}
        sources = await self.records.sources_many_in(tx, list(revisions))
        # A formal original keeps its first accepted body as the immutable witness
        # while each later revision supplies only the current text and graph.
        origin_rows = (
            (
                await tx.connection.execute(
                    sa.select(
                        schema.revisions.c.record_id, schema.revisions.c.body_artifact_id
                    ).where(
                        schema.revisions.c.record_id.in_(list(records)),
                        schema.revisions.c.ordinal == 1,
                        ~sa.select(1)
                        .where(schema.sources.c.revision_id == schema.revisions.c.id)
                        .exists(),
                    )
                )
            )
            .mappings()
            .all()
        )
        artifact_ids = {row["body_artifact_id"] for row in revisions.values()}
        artifact_ids.update(row["body_artifact_id"] for row in origin_rows)
        artifact_ids.update(
            selection.artifact.id
            for bindings in sources.values()
            for binding in bindings
            for selection in binding.selections
        )
        artifact_values = await self.artifacts.get_many_in(tx, list(artifact_ids))
        artifacts = {item.ref.id: item.ref for item in artifact_values}
        captured: dict[str, CapturedRecord] = {}
        for identifier, revision in revisions.items():
            record = records[revision["record_id"]]
            captured[identifier] = CapturedRecord(
                record=record,
                revision=revision,
                ref=self.records.revision_ref(record, revision),
                body=artifacts[revision["body_artifact_id"]],
                sources=tuple(sources[identifier]),
            )
        annotations = await self.annotations.read_many_in(
            tx, [captured[identifier].ref for identifier in current]
        )
        needs_review = {
            identifier for identifier in current if directory.entries[identifier].needs_review
        }
        origins = {
            str(row["record_id"]): artifacts[str(row["body_artifact_id"])] for row in origin_rows
        }
        return CapturedCorpus(ns, captured, current, annotations, frozenset(needs_review), origins)
