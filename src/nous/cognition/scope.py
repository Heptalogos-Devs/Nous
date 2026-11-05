"""Namespace membership and generation changes share the adopting transaction."""

from collections.abc import Mapping
from typing import TYPE_CHECKING, cast
from uuid import uuid4

import sqlalchemy as sa
from sqlalchemy.dialects.sqlite import insert

from nous.common.errors import NousError
from nous.common.operations import add_error_context
from nous.configuration.service import get_path
from nous.runtime.execution import ExecutionManager
from nous.storage.database import Database, ReadTx, WriteTx

from .contracts import Namespace
from .schema import namespaces

if TYPE_CHECKING:
    from nous.application.configuration import ConfigurationOperations


async def initialize_in(tx: WriteTx) -> None:
    """Ensure the default namespace exists in the caller's write transaction."""
    await tx.connection.execute(
        insert(namespaces)
        .values(id=str(uuid4()), name="default", content_generation=0)
        .on_conflict_do_nothing(index_elements=[namespaces.c.name])
    )


class NamespaceOperations:
    """Create and select namespaces and update their content generations.

    Methods ending in ``_in`` use the supplied transaction; its caller owns commit
    or rollback. Other methods open their own operation and database transaction.
    """
    def __init__(
        self,
        database: Database,
        execution: ExecutionManager,
        configuration: ConfigurationOperations,
    ):
        self.database, self.execution, self.configuration = database, execution, configuration

    @staticmethod
    async def get_in(tx: ReadTx, name: str) -> Namespace:
        """Resolve a namespace ID or name in the supplied read view.

        Raises:
            NousError: The namespace does not exist (``cognition.namespace_not_found``).
        """
        row = (
            (
                await tx.connection.execute(
                    sa.select(namespaces).where(
                        sa.or_(namespaces.c.id == name, namespaces.c.name == name)
                    )
                )
            )
            .mappings()
            .one_or_none()
        )
        if row is None:
            raise NousError(
                "cognition.namespace_not_found",
                "Namespace does not exist",
                details={"namespace": name},
            )
        return Namespace.model_validate(dict(row))

    async def advance_in(self, tx: WriteTx, namespace_id: str) -> Namespace:
        """Increment a namespace's content generation in the caller's transaction."""
        await tx.connection.execute(
            sa.update(namespaces)
            .where(namespaces.c.id == namespace_id)
            .values(content_generation=namespaces.c.content_generation + 1)
        )
        return await self.get_in(tx, namespace_id)

    async def create(self, name: str) -> Namespace:
        """Create a namespace with generation zero and return it after commit.

        Raises:
            NousError: The name is blank or already exists.
        """
        async with self.execution.operation("namespace.create"):
            if not name.strip():
                raise NousError("input.invalid", "Namespace name must not be empty")
            async with self.database.write() as tx:
                result = await tx.connection.execute(
                    insert(namespaces)
                    .values(id=str(uuid4()), name=name, content_generation=0)
                    .on_conflict_do_nothing(index_elements=[namespaces.c.name])
                )
                if not result.rowcount:
                    raise NousError("cognition.namespace_exists", "Namespace name already exists")
                return await self.get_in(tx, name)

    async def list(self) -> tuple[Namespace, ...]:
        """Return namespaces ordered by name."""
        async with self.execution.operation("namespace.list"):
            async with self.database.read() as tx:
                rows = (
                    (await tx.connection.execute(sa.select(namespaces).order_by(namespaces.c.name)))
                    .mappings()
                    .all()
                )
                return tuple(Namespace.model_validate(dict(row)) for row in rows)

    async def select(self, namespace: str) -> Namespace:
        """Save the selected namespace ID as the default configuration.

        If a higher-priority setting shadows the saved value, raises
        ``configuration.shadowed`` with ``selection_saved`` set to true; update the
        override to make the selection effective.
        """
        async with self.execution.operation("namespace.select"):
            async with self.database.read() as tx:
                selected = await self.get_in(tx, namespace)
            await self.configuration.update({"cognition.default_namespace": selected.id})
            effective = self.configuration.snapshot.typed_values().cognition.default_namespace
            if effective != selected.id:
                async with self.database.read() as tx:
                    actual = await self.get_in(tx, effective)
                if actual.id != selected.id:
                    raise NousError(
                        "configuration.shadowed",
                        "Default namespace selection is overridden",
                        details={
                            "selected_namespace": selected.id,
                            "effective_namespace": effective,
                            "selection_saved": True,
                            "source": self.configuration.explain("cognition.default_namespace")[
                                "source"
                            ],
                            "recovery": "Remove or update the higher-priority default_namespace override",
                        },
                    )
            return selected

    async def rename(self, namespace: str, name: str) -> Namespace:
        """Rename a namespace while preserving its stable ID.

        The saved default may first be changed from the old name to the ID. If an
        exception occurs, its context reports the requested name and saved default;
        resolve a shadowing override or concurrent rename before retrying.

        Raises:
            NousError: The name is blank or taken, an override blocks the rename,
                or the namespace changed since it was read.
        """
        async with self.execution.operation("namespace.rename"):
            if not name.strip():
                raise NousError("input.invalid", "Namespace name must not be empty")
            async with self.database.read() as tx:
                current = await self.get_in(tx, namespace)
            if current.name == name:
                return current
            selected = self.execution.snapshot.typed_values().cognition.default_namespace
            raw_source = self.execution.snapshot.sources.get("cognition.default_namespace")
            source: Mapping[str, object] = (
                cast(Mapping[str, object], raw_source) if isinstance(raw_source, Mapping) else {}
            )
            overridden = source.get("layer") in {
                "environment",
                "startup",
                "namespace",
                "task",
                "call",
            }
            try:
                if (
                    self.configuration.snapshot.typed_values().cognition.default_namespace
                    == current.name
                ):
                    await self.configuration.update({"cognition.default_namespace": current.id})
                effective = self.configuration.snapshot.typed_values().cognition.default_namespace
                if effective == current.name or selected == current.name and overridden:
                    raise NousError(
                        "configuration.shadowed",
                        "The old namespace name is selected by a higher-priority override",
                        details={
                            "effective_default_namespace": effective,
                            "source": dict(source),
                            "recovery": "Remove the default_namespace override or set it to "
                            + current.id
                            + ", then retry rename",
                        },
                    )
                async with self.database.write() as tx:
                    actual = await self.get_in(tx, current.id)
                    if actual.name != current.name:
                        raise NousError(
                            "cognition.namespace_conflict",
                            "Namespace was renamed concurrently",
                            details={"current_name": actual.name},
                        )
                    existing = await tx.connection.scalar(
                        sa.select(namespaces.c.id).where(
                            namespaces.c.name == name, namespaces.c.id != current.id
                        )
                    )
                    if existing:
                        raise NousError(
                            "cognition.namespace_exists", "Namespace name already exists"
                        )
                    await tx.connection.execute(
                        sa.update(namespaces).where(namespaces.c.id == current.id).values(name=name)
                    )
                    return await self.get_in(tx, current.id)
            except BaseException as error:
                add_error_context(
                    error,
                    {
                        "rename_applied": False,
                        "namespace_id": current.id,
                        "previous_name": current.name,
                        "requested_name": name,
                        "saved_default_namespace": get_path(
                            self.configuration.saved(), "cognition.default_namespace"
                        ),
                    },
                )
                raise
