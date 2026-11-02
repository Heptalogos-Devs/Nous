"""Transfer namespace invocation custody without changing retained provider output."""

from collections.abc import Mapping

import sqlalchemy as sa
from nous_sdk.contracts import CallContext, DomainError
from nous_sdk.contracts.model_calls import InvocationBinding, InvocationRestore
from sqlalchemy.dialects.sqlite import insert

from nous.platform.persistence import ReadView, WriteTx
from nous.platform.persistence.schema import metadata

from .invocations import invocation_key, invocations, retire_in, retirements


async def export_in(view: ReadView | WriteTx, ctx: CallContext) -> tuple[InvocationBinding, ...]:
    result: list[InvocationBinding] = []
    for table in (invocations, retirements):
        statement = sa.select(table).where(table.c.namespace_id == ctx.namespace_id)
        for row in (await view.connection.execute(statement)).mappings():
            result.append(
                InvocationBinding.model_validate(
                    {
                        **{
                            name: row[name]
                            for name in (
                                "key",
                                "namespace_id",
                                "caller_id",
                                "role",
                                "invocation_id",
                            )
                        },
                        "call_id": row.get("call_id"),
                        "reason": row.get("reason"),
                        "recorded_at": row.get("recorded_at"),
                        "retired": table is retirements,
                    }
                )
            )
    return tuple(result)


async def restore_in(
    tx: WriteTx,
    ctx: CallContext,
    bindings: tuple[InvocationBinding, ...],
    *,
    namespace_mapping: Mapping[str, str],
    caller_mapping: Mapping[str, str],
    call_mapping: Mapping[str, str],
) -> InvocationRestore:
    if ctx.namespace_id != tx.ctx.namespace_id:
        raise DomainError(
            "invocation_archive_invalid", "Invocation custody must stay in the receiving namespace"
        )
    from .history import imported_calls

    calls = metadata.tables["nous_model_calls"]
    restored = frozen = 0
    for binding in bindings:
        original = CallContext(
            caller_id=binding.caller_id,
            namespace_id=binding.namespace_id,
            credential_source_ids=None,
        )
        if invocation_key(original, binding.role, binding.invocation_id) != binding.key:
            raise DomainError(
                "invocation_archive_invalid", "Invocation identity does not match its retained key"
            )
        namespace = namespace_mapping.get(binding.namespace_id)
        caller = caller_mapping.get(binding.caller_id)
        if namespace is None or caller is None:
            raise DomainError(
                "invocation_archive_invalid",
                "Invocation requires explicit namespace and caller mappings",
                key=binding.key,
            )
        if namespace != ctx.namespace_id:
            raise DomainError(
                "invocation_archive_invalid", "Invocation mapping exceeds the receiving namespace"
            )
        receiving = ctx.model_copy(update={"caller_id": caller})
        changed = (namespace, caller) != (binding.namespace_id, binding.caller_id)
        if changed or binding.retired:
            await retire_in(
                tx,
                receiving,
                binding.role,
                binding.invocation_id,
                reason="namespace_migrated" if changed else binding.reason or "source_purged",
            )
            frozen += 1
            continue
        call = call_mapping.get(binding.call_id or "")
        if call is None:
            raise DomainError(
                "invocation_call_unavailable",
                "Invocation has no retained receiving call",
                key=binding.key,
            )
        present = await tx.connection.scalar(
            sa.select(calls.c.id).where(calls.c.id == call, calls.c.namespace_id == namespace)
        )
        if present is None:
            present = await tx.connection.scalar(
                sa.select(imported_calls.c.id).where(
                    imported_calls.c.id == call, imported_calls.c.namespace_id == namespace
                )
            )
        if present is None:
            raise DomainError(
                "invocation_call_unavailable",
                "Invocation has no retained call in its receiving namespace",
            )
        existing = await tx.connection.scalar(
            sa.select(invocations.c.call_id).where(invocations.c.key == binding.key)
        )
        if existing is not None and existing != call:
            raise DomainError(
                "model_invocation_conflict", "Receiving invocation already names a different call"
            )
        await tx.connection.execute(
            insert(invocations)
            .values(
                key=binding.key,
                call_id=call,
                namespace_id=namespace,
                caller_id=caller,
                role=binding.role,
                invocation_id=binding.invocation_id,
                commit_seq=tx.commit_seq,
            )
            .on_conflict_do_nothing(index_elements=[invocations.c.key])
        )
        restored += 1
    return InvocationRestore(restored=restored, frozen=frozen)
