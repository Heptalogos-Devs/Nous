"""Address metadata is written only inside its owning semantic transaction."""

import secrets

import sqlalchemy as sa
from nous_wave.contracts import CallContext, CognitiveRef, MediaReference
from nous_wave.contracts.common import ReferenceAddresses
from nous_wave.contracts.context import CognitiveTarget, MediaTarget
from nous_wave.contracts.references import (
    CognitiveIdentity,
    ObjectTarget,
    ReferenceBinding,
    SemanticTarget,
)
from pydantic import TypeAdapter
from sqlalchemy.dialects.sqlite import insert

from nous.platform.persistence import ReadView, WriteTx
from nous.platform.persistence.authority import canonical_json

from .codec import decode, encode
from .schema import addresses, imported

_TARGET = TypeAdapter(SemanticTarget)
_PREFIXES = {
    "memory": "mem",
    "memory_semantics": "sem",
    "concept": "tag",
    "entity": "ent",
    "episode": "ep",
    "journal": "journal",
    "schema": "schema",
    "association": "assoc",
    "self_facet": "self",
    "narrative_thread": "thread",
    "domain_knowledge": "know",
    "social_relation": "rel",
    "language_convention": "lang",
    "desire": "desire",
    "concern": "concern",
}


def prefix(target: SemanticTarget) -> str:
    if isinstance(target, ObjectTarget):
        return _PREFIXES[target.identity.kind]
    if isinstance(target, CognitiveTarget):
        return _PREFIXES[target.reference.kind] + "rev"
    return {
        "material": "art",
        "source_region": "src",
        "representation": "repr",
        "derived_region": "region",
    }[target.reference.kind]


def validate_address(address: str, target: SemanticTarget | None = None) -> None:
    kind, separator, value = address.partition(":")
    allowed = (
        set(_PREFIXES.values())
        | {v + "rev" for v in _PREFIXES.values()}
        | {"art", "src", "repr", "region"}
    )
    if not separator or kind not in allowed or (target is not None and kind != prefix(target)):
        raise ValueError("Readable reference has an invalid type prefix")
    decode(value)


def targets(reference: CognitiveRef | MediaReference) -> tuple[SemanticTarget, ...]:
    if isinstance(reference, CognitiveRef):
        return (
            ObjectTarget(identity=CognitiveIdentity(kind=reference.kind, id=reference.id)),
            CognitiveTarget(reference=reference),
        )
    return (MediaTarget(reference=reference.model_copy(update={"observation_id": None})),)


async def _ensure(tx: WriteTx, ctx: CallContext, target: SemanticTarget) -> str:
    encoded = canonical_json(target)
    existing = await tx.connection.scalar(
        sa.select(addresses.c.address).where(
            addresses.c.namespace_id == ctx.namespace_id, addresses.c.target == encoded
        )
    )
    if existing is not None:
        return str(existing)
    for _ in range(32):
        address = prefix(target) + ":" + encode(secrets.randbits(48))
        result = await tx.connection.execute(
            insert(addresses)
            .values(
                address=address,
                namespace_id=ctx.namespace_id,
                target=encoded,
                commit_seq=tx.commit_seq,
            )
            .on_conflict_do_nothing()
        )
        if result.rowcount == 1:
            return address
    raise RuntimeError("Readable address collision retry limit exceeded")


async def allocate_in(
    tx: WriteTx, ctx: CallContext, reference: CognitiveRef | MediaReference
) -> ReferenceAddresses:
    selected = targets(reference)
    allocated = [await _ensure(tx, ctx, target) for target in selected]
    return ReferenceAddresses(
        object=allocated[0] if len(allocated) == 2 else None, exact=allocated[-1]
    )


async def recover_in(
    tx: WriteTx,
    ctx: CallContext,
    reference: CognitiveRef | MediaReference,
    known: ReferenceAddresses | None,
) -> ReferenceAddresses:
    """Recover saved names without taking a name already bound to another identity."""
    if known is None:
        return await allocate_in(tx, ctx, reference)
    selected = targets(reference)
    names = (known.object, known.exact) if len(selected) == 2 else (known.exact,)
    for target, address in zip(selected, names, strict=True):
        if address is None:
            continue
        validate_address(address, target)
        await tx.connection.execute(
            insert(addresses)
            .values(
                address=address,
                namespace_id=ctx.namespace_id,
                target=canonical_json(target),
                commit_seq=tx.commit_seq,
            )
            .on_conflict_do_nothing()
        )
    return await allocate_in(tx, ctx, reference)


async def addresses_in(
    view: ReadView | WriteTx, ctx: CallContext, reference: CognitiveRef | MediaReference
) -> ReferenceAddresses | None:
    selected = targets(reference)
    resolved: list[str] = []
    for target in selected:
        address = await view.connection.scalar(
            sa.select(addresses.c.address).where(
                addresses.c.namespace_id == ctx.namespace_id,
                addresses.c.target == canonical_json(target),
            )
        )
        if address is None:
            return None
        resolved.append(str(address))
    return ReferenceAddresses(
        object=resolved[0] if len(resolved) == 2 else None, exact=resolved[-1]
    )


async def export_bindings_in(
    view: ReadView | WriteTx,
    ctx: CallContext,
    references: tuple[CognitiveRef | MediaReference, ...],
) -> tuple[ReferenceBinding, ...]:
    encoded = {canonical_json(target) for ref in references for target in targets(ref)}
    rows = (
        await view.connection.execute(
            sa.select(addresses.c.address, addresses.c.target).where(
                addresses.c.namespace_id == ctx.namespace_id, addresses.c.target.in_(encoded)
            )
        )
    ).all()
    inherited = (
        await view.connection.execute(
            sa.select(imported.c.address, imported.c.target).where(
                imported.c.namespace_id == ctx.namespace_id,
                imported.c.subject_id == ctx.subject_id,
                imported.c.target.in_(encoded),
            )
        )
    ).all()
    unique = {(str(row.address), str(row.target)) for row in (*rows, *inherited)}
    return tuple(
        ReferenceBinding(address=address, target=_TARGET.validate_json(target))
        for address, target in sorted(unique)
    )


async def import_bindings_in(
    tx: WriteTx, ctx: CallContext, bundle_id: str, bindings: tuple[ReferenceBinding, ...]
) -> None:
    for binding in bindings:
        validate_address(binding.address, binding.target)
        await tx.connection.execute(
            insert(imported)
            .values(
                namespace_id=ctx.namespace_id,
                subject_id=ctx.require_subject(),
                bundle_id=bundle_id,
                address=binding.address,
                target=canonical_json(binding.target),
                commit_seq=tx.commit_seq,
            )
            .on_conflict_do_nothing()
        )
