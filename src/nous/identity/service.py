"""Subject ownership and capability admission."""

from uuid import uuid4

from nous_wave.contracts import CallContext, DomainError
from sqlalchemy import JSON, Column, Integer, String, Table, select

from nous.platform.persistence import Authority, ReadView, WriteTx, metadata

namespaces = Table(
    "namespaces",
    metadata,
    Column("id", String, primary_key=True),
    Column("name", String, nullable=False),
)
subjects = Table(
    "subjects",
    metadata,
    Column("id", String, primary_key=True),
    Column("namespace_id", String, nullable=False, index=True),
    Column("name", String, nullable=False),
    Column("capabilities", JSON, nullable=False),
    Column("commit_seq", Integer, nullable=False),
)


async def require_subject_in(
    view: ReadView | WriteTx, ctx: CallContext, capability: str | None = None
):
    row = (
        (
            await view.connection.execute(
                select(subjects).where(
                    subjects.c.id == ctx.subject_id, subjects.c.namespace_id == ctx.namespace_id
                )
            )
        )
        .mappings()
        .first()
    )
    if row is None:
        raise DomainError("subject_not_found", "Subject does not exist in this namespace")
    if capability and capability not in row["capabilities"]:
        raise DomainError("capability_unavailable", f"Subject does not provide {capability}")
    return row


class IdentityService:
    def __init__(self, authority: Authority) -> None:
        self.authority = authority

    async def create_namespace(self, ctx: CallContext, name: str, operation_id: str):
        async with self.authority.write(
            ctx,
            operation_id,
            operation_kind="namespace.create",
            scope=ctx.namespace_id,
            payload={"name": name},
        ) as tx:
            if tx.replayed:
                return tx.replay_result
            await tx.connection.execute(namespaces.insert().values(id=ctx.namespace_id, name=name))
            return tx.result({"id": ctx.namespace_id, "name": name})

    async def create_subject(
        self,
        ctx: CallContext,
        name: str,
        operation_id: str,
        capabilities: tuple[str, ...] = ("memory",),
        subject_id: str | None = None,
    ):
        command = {"name": name, "capabilities": capabilities, "subject_id": subject_id}
        async with self.authority.write(
            ctx,
            operation_id,
            operation_kind="subject.create",
            scope=ctx.namespace_id,
            payload=command,
        ) as tx:
            if tx.replayed:
                return tx.replay_result
            if not (
                await tx.connection.execute(
                    select(namespaces.c.id).where(namespaces.c.id == ctx.namespace_id)
                )
            ).first():
                raise DomainError("namespace_not_found", "Namespace does not exist")
            value = {
                "id": subject_id or str(uuid4()),
                "namespace_id": ctx.namespace_id,
                "name": name,
                "capabilities": list(dict.fromkeys(capabilities)),
                "commit_seq": tx.commit_seq,
            }
            await tx.connection.execute(subjects.insert().values(**value))
            return tx.result(value)

    async def list_subjects(self, ctx: CallContext):
        async with self.authority.read(ctx) as view:
            return [
                dict(row)
                for row in (
                    await view.connection.execute(
                        select(subjects).where(subjects.c.namespace_id == ctx.namespace_id)
                    )
                ).mappings()
            ]
