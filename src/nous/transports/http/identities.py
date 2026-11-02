"""Namespace metadata, lifecycle and host identity bindings."""
from fastapi import APIRouter, Request
from nous_sdk.contracts import CallContext, DomainError
from nous_sdk.contracts.identity import BindSubject, BoundSubject, ChangeNamespace, CreateNamespace, IdentityCapabilities, IdentityDirectoryQuery, IdentityPurpose, Namespace, NamespacePage, NamespacePurgeResult, PurgeNamespace, ReadSubjectBinding
from nous_sdk.contracts.namespace_archives import NamespaceArchive, NamespaceRestoreResult, RestoreNamespace
from nous.identity.credentials import Principal
from .security import application, authorize
router = APIRouter()

async def metadata_context(request: Request, namespace_id: str = 'instance', purpose: IdentityPurpose = 'active') -> tuple[Principal, CallContext]:
    actor = await authorize(request, capability='manage' if purpose=='management' else 'read')
    if namespace_id != 'instance': actor.require_scope(namespace_id)
    return actor, CallContext(caller_id=actor.caller_id,namespace_id=namespace_id,credential_source_ids=actor.source_ids)

async def manager_context(request: Request, namespace_id: str) -> CallContext:
    actor = await authorize(request,capability='manage')
    actor.require_scope(namespace_id)
    if actor.source_ids is not None: raise DomainError('identity_management_scope_forbidden','Identity management requires the complete target scope')
    return CallContext(caller_id=actor.caller_id,namespace_id=namespace_id,credential_source_ids=actor.source_ids)

@router.post('/namespaces/query')
async def namespace_page(request: Request, command: IdentityDirectoryQuery) -> NamespacePage:
    actor,ctx = await metadata_context(request,purpose=command.purpose)
    return await application(request).identity.namespace_page(ctx,command,allowed_namespaces=actor.namespace_ids)

@router.get('/namespaces')
async def namespaces(request: Request,purpose: IdentityPurpose='active') -> tuple[Namespace,...]:
    actor,ctx=await metadata_context(request,purpose=purpose)
    values=[];command=IdentityDirectoryQuery(purpose=purpose,limit=1000)
    while True:
        page=await application(request).identity.namespace_page(ctx,command,allowed_namespaces=actor.namespace_ids)
        values.extend(page.items)
        if page.next_cursor is None:return tuple(values)
        command=command.model_copy(update={'cursor':page.next_cursor})

@router.post('/namespaces')
async def create_namespace(request: Request, command: CreateNamespace) -> Namespace:
    return await application(request).identity.create_namespace(await manager_context(request,command.id or 'instance'),command.slug,command.operation_id,namespace_id=command.id,display_name=command.display_name,mode=command.mode,subject_key=command.subject_key,enabled_features=command.enabled_features)

@router.get('/namespaces/{namespace_id}')
async def namespace(request: Request,namespace_id: str,purpose: IdentityPurpose='active') -> Namespace:
    value=await application(request).identity.resolve(namespace_id,purpose=purpose)
    _,ctx=await metadata_context(request,value.id,purpose)
    return await application(request).identity.namespace(ctx,purpose)

@router.post('/namespaces/{namespace_id}/change')
async def change_namespace(request: Request,namespace_id: str,command: ChangeNamespace) -> Namespace:
    return await application(request).identity.change_namespace(await manager_context(request,namespace_id),command)

@router.get('/namespaces/{namespace_id}/capabilities')
async def capabilities(request: Request,namespace_id: str,purpose: IdentityPurpose='active') -> IdentityCapabilities:
    _,ctx=await metadata_context(request,namespace_id,purpose)
    return await application(request).identity.capabilities(ctx,purpose)

@router.post('/namespaces/{namespace_id}/subject-bindings')
async def bind_subject(request: Request,namespace_id: str,command: BindSubject) -> BoundSubject:
    return await application(request).identity.bind(await manager_context(request,namespace_id),command)

@router.post('/namespaces/{namespace_id}/subject-bindings/read')
async def subject_binding(request: Request,namespace_id: str,command: ReadSubjectBinding) -> BoundSubject:
    _,ctx=await metadata_context(request,namespace_id,command.purpose)
    return await application(request).identity.binding(ctx,command.external_id,command.purpose)

@router.post('/namespaces/{namespace_id}/purge')
async def purge_namespace(request: Request,namespace_id: str,command: PurgeNamespace) -> NamespacePurgeResult:
    return await application(request).namespace_data.purge_namespace(await manager_context(request,namespace_id),command)

@router.post('/namespaces/{namespace_id}/archive')
async def archive_namespace(request: Request,namespace_id: str) -> NamespaceArchive:
    return await application(request).namespace_archives.export(await manager_context(request,namespace_id))

@router.post('/namespaces/{namespace_id}/restore-archive')
async def restore_namespace(request: Request,namespace_id: str,command: RestoreNamespace) -> NamespaceRestoreResult:
    return await application(request).namespace_archives.restore(await manager_context(request,namespace_id),command)
