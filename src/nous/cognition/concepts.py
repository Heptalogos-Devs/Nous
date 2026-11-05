"""Stable Concept identity, explicit discovery and immutable meaning revisions."""

from collections.abc import Sequence
from uuid import uuid4

import sqlalchemy as sa

from nous.common.errors import NousError
from nous.common.identifier import insert_identifier, split_revision
from nous.common.operations import record_completed
from nous.runtime.execution import ExecutionManager
from nous.storage.database import Database, ReadTx, WriteTx
from nous.temporal.values import Clock

from . import concept_schema as schema
from .concept_contracts import ConceptRef, ConceptRevisionRef, ConceptView
from .contracts import Namespace
from .scope import NamespaceOperations

ConceptTarget = str | ConceptRef | ConceptRevisionRef


class ConceptOperations:
    """Manage stable Concepts and their immutable meaning revisions.

    A bound instance accepts only its namespace; an unbound instance uses the
    configured default when no namespace is supplied. Methods ending in ``_in``
    use a caller-owned transaction.
    """
    def __init__(
        self,
        database: Database,
        execution: ExecutionManager,
        scope: NamespaceOperations,
        instance_id: str,
        clock: Clock,
        namespace: Namespace | None = None,
    ):
        self.database, self.execution, self.scope = database, execution, scope
        self.instance_id, self.clock = instance_id, clock
        self.bound_namespace = namespace

    def namespace(self, namespace: str | None) -> str:
        if self.bound_namespace is not None:
            if namespace is not None and namespace not in {
                self.bound_namespace.id,
                self.bound_namespace.name,
            }:
                raise NousError(
                    "cognition.namespace_mismatch", "Operation belongs to another namespace"
                )
            return self.bound_namespace.id
        return namespace or self.execution.snapshot.typed_values().cognition.default_namespace

    async def read_in(
        self, tx: ReadTx, target: ConceptTarget, namespace: str | None = None
    ) -> ConceptView:
        """Resolve a Concept or exact meaning revision in the caller's read view.

        An unqualified target selects the current meaning; an exact revision preserves
        the historical meaning and reports the current head separately.
        """
        ordinal = None
        exact_id = None
        if isinstance(target, ConceptRevisionRef):
            ordinal, exact_id, ref = target.ordinal, target.revision_id, target.concept_ref
        elif isinstance(target, ConceptRef):
            ref = target
        else:
            ref = None
            target, ordinal = split_revision(target)
        if ref is not None:
            if ref.instance_id != self.instance_id:
                raise NousError(
                    "cognition.instance_mismatch", "Concept belongs to another instance"
                )
            target = ref.id
        identity = (
            (
                await tx.connection.execute(
                    sa.select(schema.concepts).where(
                        sa.or_(schema.concepts.c.id == target, schema.concepts.c.identifier == target)
                    )
                )
            )
            .mappings()
            .one_or_none()
        )
        if identity is None:
            raise NousError("cognition.concept_not_found", "Concept does not exist")
        ns = await self.scope.get_in(
            tx, self.namespace(namespace or (ref.namespace_id if ref else None))
        )
        if identity["namespace_id"] != ns.id or (ref is not None and ref.namespace_id != ns.id):
            raise NousError("cognition.namespace_mismatch", "Concept belongs to another namespace")
        head = (
            (
                await tx.connection.execute(
                    sa.select(schema.concept_revisions).where(
                        schema.concept_revisions.c.id == identity["head_revision_id"]
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
                    sa.select(schema.concept_revisions).where(
                        schema.concept_revisions.c.concept_id == identity["id"],
                        schema.concept_revisions.c.ordinal == ordinal,
                    )
                )
            )
            .mappings()
            .one_or_none()
        )
        if revision is None or (exact_id is not None and revision["id"] != exact_id):
            raise NousError(
                "cognition.concept_revision_not_found", "Exact Concept revision does not exist"
            )
        concept_ref = ConceptRef(
            instance_id=self.instance_id,
            namespace_id=ns.id,
            id=identity["id"],
            identifier=identity["identifier"],
        )
        return ConceptView(
            ref=ConceptRevisionRef(
                concept_ref=concept_ref, revision_id=revision["id"], ordinal=revision["ordinal"]
            ),
            current_revision=ConceptRevisionRef(
                concept_ref=concept_ref, revision_id=head["id"], ordinal=head["ordinal"]
            ),
            label=revision["label"],
            description=revision["description"],
            aliases=tuple(revision["aliases"]),
            author=revision["author"],
            recorded_at=revision["recorded_at"],
        )

    async def read_revisions_in(
        self, tx: ReadTx, revision_ids: Sequence[str]
    ) -> dict[str, ConceptView]:
        """Read adopted exact meanings and their current heads in the caller's view."""
        requested = list(dict.fromkeys(revision_ids))
        output: dict[str, ConceptView] = {}
        head = schema.concept_revisions.alias("current_meaning")
        for start in range(0, len(requested), 400):
            rows = (
                await tx.connection.execute(
                    sa.select(
                        schema.concept_revisions,
                        schema.concepts.c.identifier.label("concept_identifier"),
                        schema.concepts.c.namespace_id,
                        head.c.id.label("current_id"),
                        head.c.ordinal.label("current_ordinal"),
                    )
                    .join(
                        schema.concepts,
                        schema.concepts.c.id == schema.concept_revisions.c.concept_id,
                    )
                    .join(head, head.c.id == schema.concepts.c.head_revision_id)
                    .where(schema.concept_revisions.c.id.in_(requested[start : start + 400]))
                )
            ).mappings()
            for row in rows:
                ref = ConceptRef(
                    instance_id=self.instance_id,
                    namespace_id=row["namespace_id"],
                    id=row["concept_id"],
                    identifier=row["concept_identifier"],
                )
                output[row["id"]] = ConceptView(
                    ref=ConceptRevisionRef(
                        concept_ref=ref, revision_id=row["id"], ordinal=row["ordinal"]
                    ),
                    current_revision=ConceptRevisionRef(
                        concept_ref=ref,
                        revision_id=row["current_id"],
                        ordinal=row["current_ordinal"],
                    ),
                    label=row["label"],
                    description=row["description"],
                    aliases=tuple(row["aliases"]),
                    author=row["author"],
                    recorded_at=row["recorded_at"],
                )
        missing = set(requested) - output.keys()
        if missing:
            raise NousError(
                "cognition.concept_revision_not_found",
                "Exact Concept revision does not exist",
                details={"revision_ids": sorted(missing)},
            )
        return output

    @staticmethod
    def validate(label: str, description: str, aliases: Sequence[str]) -> None:
        """Reject blank labels, descriptions, or aliases before adopting a meaning."""
        if (
            not label.strip()
            or not description.strip()
            or any(not alias.strip() for alias in aliases)
        ):
            raise NousError(
                "input.invalid_concept", "Concept label, description and aliases must not be empty"
            )

    async def create_in(
        self,
        tx: WriteTx,
        namespace: str,
        label: str,
        description: str,
        aliases: Sequence[str] = (),
        *,
        author: str | None = None,
        producer_result_id: str | None = None,
        body_revision_id: str | None = None,
    ) -> ConceptView:
        """Create a Concept and its first meaning in the caller's write transaction.

        Aliases are deduplicated in input order. Producer and body revision IDs record
        the origin of an adopted meaning when supplied.
        """
        self.validate(label, description, aliases)
        ns = await self.scope.get_in(tx, self.namespace(namespace))
        identity, revision_id = str(uuid4()), str(uuid4())
        identifier = await insert_identifier(
            tx,
            schema.concepts,
            "con",
            {
                "id": identity,
                "namespace_id": ns.id,
                "head_revision_id": revision_id,
            },
        )
        await tx.connection.execute(
            schema.concept_revisions.insert().values(
                id=revision_id,
                concept_id=identity,
                ordinal=1,
                label=label,
                description=description,
                aliases=list(dict.fromkeys(aliases)),
                author=author,
                recorded_at=self.clock.utc_now().isoformat(),
                producer_result_id=producer_result_id,
                body_revision_id=body_revision_id,
            )
        )
        return await self.read_in(tx, identifier, ns.id)

    async def create(
        self,
        namespace: str | None,
        label: str,
        description: str,
        aliases: Sequence[str] = (),
        *,
        author: str | None = None,
    ) -> ConceptView:
        """Commit a new Concept and first meaning in the selected namespace.

        Raises:
            NousError: The label, description, or an alias is blank, or the namespace
                cannot be resolved.
        """
        async with self.execution.operation("concept.create"):
            async with self.database.write() as tx:
                view = await self.create_in(
                    tx, self.namespace(namespace), label, description, aliases, author=author
                )
            record_completed(view.ref.identifier)
            return view

    async def read(self, target: ConceptTarget, namespace: str | None = None) -> ConceptView:
        """Read the current meaning or an explicitly addressed historical revision."""
        async with self.execution.operation("concept.read"):
            async with self.database.read() as tx:
                return await self.read_in(tx, target, namespace)

    async def revise(
        self,
        target: ConceptTarget,
        expected_revision: ConceptRevisionRef | str,
        label: str,
        description: str,
        aliases: Sequence[str] = (),
        *,
        namespace: str | None = None,
        author: str | None = None,
    ) -> ConceptView:
        """Commit a new meaning if the supplied exact revision is still the head.

        ``expected_revision`` must identify a revision, including ``@N`` for a string.
        Aliases are deduplicated in input order.

        Raises:
            NousError: The expected revision is not exact or the current head changed
                (``cognition.concept_conflict``); reread before choosing a new base.
        """
        self.validate(label, description, aliases)
        if isinstance(expected_revision, str) and "@" not in expected_revision:
            raise NousError("input.expected_required", "Expected Concept revision must be exact")
        async with self.execution.operation("concept.revise"):
            async with self.database.write() as tx:
                current = await self.read_in(tx, target, namespace)
                expected = await self.read_in(
                    tx, expected_revision, current.ref.concept_ref.namespace_id
                )
                if (
                    current.ref.concept_ref != expected.ref.concept_ref
                    or current.current_revision != expected.ref
                ):
                    raise NousError(
                        "cognition.concept_conflict",
                        "Concept meaning changed since it was read",
                        details={
                            "current_revision": current.current_revision.identifier,
                            "expected_revision": expected.ref.identifier,
                        },
                    )
                new_id = str(uuid4())
                await tx.connection.execute(
                    schema.concept_revisions.insert().values(
                        id=new_id,
                        concept_id=current.ref.concept_ref.id,
                        ordinal=current.current_revision.ordinal + 1,
                        label=label,
                        description=description,
                        aliases=list(dict.fromkeys(aliases)),
                        author=author,
                        recorded_at=self.clock.utc_now().isoformat(),
                        producer_result_id=None,
                        body_revision_id=None,
                    )
                )
                await tx.connection.execute(
                    schema.concepts.update()
                    .where(schema.concepts.c.id == current.ref.concept_ref.id)
                    .values(head_revision_id=new_id)
                )
                updated = await self.read_in(
                    tx, current.ref.concept_ref, current.ref.concept_ref.namespace_id
                )
            record_completed(updated.ref.identifier)
            return updated

    async def find(
        self, namespace: str | None = None, text: str = "", *, offset: int = 0, limit: int = 20
    ) -> tuple[ConceptView, ...]:
        """Find current meanings by case-insensitive text in label, description or aliases.

        An empty text lists all meanings. Results are ordered by label and identifier;
        ``offset`` must be nonnegative and ``limit`` must be between 1 and 100.
        """
        if offset < 0 or not 1 <= limit <= 100:
            raise NousError("input.invalid", "Use a nonnegative offset and a limit from 1 to 100")
        async with self.execution.operation("concept.find"):
            async with self.database.read() as tx:
                ns = await self.scope.get_in(tx, self.namespace(namespace))
                query = (
                    sa.select(schema.concepts.c.identifier)
                    .join(
                        schema.concept_revisions,
                        schema.concept_revisions.c.id == schema.concepts.c.head_revision_id,
                    )
                    .where(schema.concepts.c.namespace_id == ns.id)
                )
                if text:
                    # SQLite JSON1 searches complete alias values without treating the JSON syntax as content.
                    alias = (
                        sa.func.json_each(schema.concept_revisions.c.aliases)
                        .table_valued("value")
                        .alias("alias")
                    )
                    query = query.where(
                        sa.or_(
                            schema.concept_revisions.c.label.icontains(text, autoescape=True),
                            schema.concept_revisions.c.description.icontains(text, autoescape=True),
                            sa.select(1)
                            .select_from(alias)
                            .where(alias.c.value.icontains(text, autoescape=True))
                            .exists(),
                        )
                    )
                identifiers = (
                    (
                        await tx.connection.execute(
                            query.order_by(
                                schema.concept_revisions.c.label, schema.concepts.c.identifier
                            )
                            .offset(offset)
                            .limit(limit)
                        )
                    )
                    .scalars()
                    .all()
                )
                return tuple([await self.read_in(tx, identifier, ns.id) for identifier in identifiers])

    async def list(
        self, namespace: str | None = None, *, offset: int = 0, limit: int = 20
    ) -> tuple[ConceptView, ...]:
        """List current meanings in label and identifier order using the given page."""
        return await self.find(namespace, offset=offset, limit=limit)

    async def candidates(self, namespace: str, body: str, limit: int) -> tuple[ConceptView, ...]:
        """Discover current meanings whose label or alias occurs in the body.

        Matching is case-insensitive and does not require a retrieval snapshot. A
        nonpositive limit returns an empty tuple; results use label and identifier order.
        """
        if limit <= 0:
            return ()
        async with self.execution.operation("concept.candidates"):
            async with self.database.read() as tx:
                ns = await self.scope.get_in(tx, self.namespace(namespace))
                alias = (
                    sa.func.json_each(schema.concept_revisions.c.aliases)
                    .table_valued("value")
                    .alias("alias")
                )
                query = (
                    sa.select(schema.concepts.c.identifier)
                    .join(
                        schema.concept_revisions,
                        schema.concept_revisions.c.id == schema.concepts.c.head_revision_id,
                    )
                    .where(
                        schema.concepts.c.namespace_id == ns.id,
                        sa.or_(
                            sa.func.instr(
                                sa.func.lower(body), sa.func.lower(schema.concept_revisions.c.label)
                            )
                            > 0,
                            sa.select(1)
                            .select_from(alias)
                            .where(
                                sa.func.instr(sa.func.lower(body), sa.func.lower(alias.c.value)) > 0
                            )
                            .exists(),
                        ),
                    )
                    .order_by(schema.concept_revisions.c.label, schema.concepts.c.identifier)
                    .limit(limit)
                )
                identifiers = (await tx.connection.execute(query)).scalars().all()
                return tuple([await self.read_in(tx, identifier, ns.id) for identifier in identifiers])

    async def status(self) -> dict[str, int]:
        """Count Concepts and meaning revisions across the instance."""
        async with self.execution.operation("concept.status"):
            async with self.database.read() as tx:
                return {
                    "concepts": int(
                        await tx.connection.scalar(
                            sa.select(sa.func.count()).select_from(schema.concepts)
                        )
                        or 0
                    ),
                    "meaning_revisions": int(
                        await tx.connection.scalar(
                            sa.select(sa.func.count()).select_from(schema.concept_revisions)
                        )
                        or 0
                    ),
                }
