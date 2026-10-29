"""Historical call custody stays separate from local paid invocation admission."""

from collections.abc import Mapping
from uuid import uuid4

import sqlalchemy as sa
from nous_wave.contracts import CallContext, DomainError
from nous_wave.contracts.model_calls import ArchivedCall, CallOrigin
from pydantic import JsonValue, TypeAdapter
from sqlalchemy.engine import RowMapping

from nous.platform.persistence import ReadView, WriteTx, metadata

from .source_fence import source_identities

imported_calls = sa.Table(
    "nous_imported_model_calls",
    metadata,
    sa.Column("id", sa.String, primary_key=True),
    sa.Column("namespace_id", sa.String, nullable=False),
    sa.Column("subject_id", sa.String, nullable=False),
    sa.Column("installation_id", sa.String, nullable=False),
    sa.Column("archive_id", sa.String, nullable=False),
    sa.Column("record", sa.JSON, nullable=False),
    sa.UniqueConstraint("namespace_id", "subject_id", "installation_id", "archive_id"),
)


def require_visible(ctx: CallContext, sources: tuple[str, ...]) -> None:
    groups = ctx.source_groups()
    if groups and (not sources or any(not set(sources).issubset(group) for group in groups)):
        raise DomainError("model_call_forbidden", "Model call inputs exceed the bound source scope")


def provider_record(row: RowMapping) -> ArchivedCall:
    request = TypeAdapter(dict[str, JsonValue]).validate_python(row["request"])
    return ArchivedCall(
        id=str(row["id"]),
        origin=CallOrigin(
            namespace_id=str(row["namespace_id"]),
            subject_id=str(row["subject_id"]) if row["subject_id"] is not None else None,
            call_id=str(row["id"]),
        ),
        purpose=str(row["purpose"]),
        request_hash=str(row["request_hash"]),
        day=str(row["day"]),
        status=row["status"],
        request=request,
        result=TypeAdapter(JsonValue).validate_python(row["result"]),
        usage=TypeAdapter(dict[str, JsonValue] | None).validate_python(row["usage"]),
        error_code=str(row["error_code"]) if row["error_code"] is not None else None,
        source_ids=tuple(sorted(source_identities(request))),
    )


async def export_in(
    view: ReadView | WriteTx,
    ctx: CallContext,
    call_ids: tuple[str, ...],
    provider_calls: sa.Table,
) -> tuple[ArchivedCall, ...]:
    result: list[ArchivedCall] = []
    for identifier in dict.fromkeys(call_ids):
        row = (
            (
                await view.connection.execute(
                    sa.select(provider_calls).where(
                        provider_calls.c.id == identifier,
                        provider_calls.c.namespace_id == ctx.namespace_id,
                        provider_calls.c.subject_id == ctx.subject_id,
                    )
                )
            )
            .mappings()
            .first()
        )
        if row is not None:
            archived = provider_record(row)
        else:
            historical = await view.connection.scalar(
                sa.select(imported_calls.c.record).where(
                    imported_calls.c.id == identifier,
                    imported_calls.c.namespace_id == ctx.namespace_id,
                    imported_calls.c.subject_id == ctx.subject_id,
                )
            )
            if historical is None:
                raise DomainError(
                    "model_call_unavailable",
                    "The referenced model call is not retained in this subject",
                    call_id=identifier,
                )
            archived = ArchivedCall.model_validate(historical)
        require_visible(ctx, archived.source_ids)
        result.append(archived)
    return tuple(result)


async def import_in(
    tx: WriteTx,
    ctx: CallContext,
    records: tuple[ArchivedCall, ...],
    mapping: Mapping[str, str],
    installation_id: str,
) -> dict[str, str]:
    if (ctx.namespace_id, ctx.subject_id) != (tx.ctx.namespace_id, tx.ctx.subject_id):
        raise DomainError(
            "model_call_forbidden", "Imported calls require the receiving transaction"
        )
    subject = ctx.require_subject()
    identities: dict[str, str] = {}
    for record in records:
        if record.id in identities:
            raise ValueError("Native package repeats a model call identity")
        missing = set(record.source_ids).difference(mapping)
        if missing:
            raise DomainError(
                "bundle_call_source_missing",
                "A model call omits part of its actual source closure",
                call_id=record.id,
                sources=sorted(missing),
            )
        identifier = str(uuid4())
        value = record.model_copy(
            update={
                "id": identifier,
                "source_ids": tuple(mapping[source] for source in record.source_ids),
            }
        )
        await tx.connection.execute(
            imported_calls.insert().values(
                id=identifier,
                namespace_id=ctx.namespace_id,
                subject_id=subject,
                installation_id=installation_id,
                archive_id=record.id,
                record=value.model_dump(mode="json"),
            )
        )
        identities[record.id] = identifier
    return identities


async def list_in(view: ReadView, ctx: CallContext) -> list[dict[str, object]]:
    records = (
        await view.connection.execute(
            sa.select(imported_calls.c.record).where(
                imported_calls.c.namespace_id == ctx.namespace_id,
                imported_calls.c.subject_id == ctx.subject_id,
            )
        )
    ).scalars()
    return [
        {
            **ArchivedCall.model_validate(value).model_dump(mode="json"),
            "namespace_id": ctx.namespace_id,
            "subject_id": ctx.subject_id,
            "record_type": "imported",
        }
        for value in records
    ]


async def purge_in(
    tx: WriteTx,
    *,
    call_ids: tuple[str, ...] = (),
    source_ids: set[str] | None = None,
) -> None:
    predicate = [imported_calls.c.namespace_id == tx.ctx.namespace_id]
    if source_ids is None:
        predicate.extend(
            (
                imported_calls.c.subject_id == tx.ctx.require_subject(),
                imported_calls.c.id.in_(call_ids),
            )
        )
    rows = (await tx.connection.execute(sa.select(imported_calls).where(*predicate))).mappings()
    for row in rows:
        record = ArchivedCall.model_validate(row["record"])
        if source_ids is not None and not set(record.source_ids).intersection(source_ids):
            continue
        erased = record.model_copy(
            update={
                "status": "purged",
                "request": {"purged": True},
                "result": None,
                "usage": None,
            }
        )
        await tx.connection.execute(
            imported_calls.update()
            .where(imported_calls.c.id == row["id"])
            .values(record=erased.model_dump(mode="json"))
        )
