"""Shared Namespace admission and enabled feature enforcement."""
import sqlalchemy as sa
from nous_sdk.contracts import CallContext, DomainError
from nous_sdk.contracts.identity import IdentityPurpose, Namespace
from nous.platform.persistence.authority import ReadView, WriteTx
from .schema import namespaces

def normalise_capabilities(values: list[str] | tuple[str, ...]) -> list[str]:
    return list(dict.fromkeys(values))

async def require_namespace_in(
    view: ReadView | WriteTx, ctx: CallContext, capability: str | None = None, *,
    purpose: IdentityPurpose = 'active',
) -> Namespace:
    row = (await view.connection.execute(sa.select(namespaces).where(namespaces.c.id == ctx.namespace_id))).mappings().first()
    if row is None:
        raise DomainError('namespace_not_found', 'Namespace does not exist')
    value = Namespace.model_validate(row)
    if purpose != 'management':
        if value.purging:
            raise DomainError('namespace_purging', 'Namespace permanent removal is in progress')
        if value.state != 'active':
            raise DomainError('namespace_' + value.state, 'Namespace is ' + value.state, id=value.id)
        if capability and capability not in value.enabled_features:
            raise DomainError('capability_unavailable', f'Namespace does not provide {capability}')
    return value
