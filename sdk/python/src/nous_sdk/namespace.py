"""One Namespace binding for complete SDK operations."""
from __future__ import annotations
from pathlib import Path
from typing import TYPE_CHECKING, Literal
from uuid import uuid4
from pydantic import TypeAdapter
from .contracts import CognitiveRecord, ImportDocument, QueryRequest, QueryResult
from .contracts.identity import BindSubject, ChangeNamespace, CreateNamespace, IdentityDirectoryQuery, Namespace, NamespacePage
from .contracts.public import CorrectionInput, ReadRequest, ReadView, RememberInput
from .contracts.corrections import CorrectionResult
from .contracts.query_context import QueryContext
from .contracts.media import MediaReference
from .contracts.text_import import TextImport, TextImportResult, TextImportSubmission
from .operations import encoded, segment
if TYPE_CHECKING:
    from .client import NousClient

class NamespaceDirectory:
    def __init__(self, client: NousClient):
        self.client=client

    async def create(self, slug: str, *, display_name: str | None=None, mode: Literal['knowledge','subject']='knowledge', subject_key: str | None=None, enabled_features: tuple[str,...] | None=None, operation_id: str | None=None) -> Namespace:
        command=CreateNamespace(slug=slug,display_name=display_name,mode=mode,subject_key=subject_key,enabled_features=enabled_features,operation_id=operation_id or str(uuid4()))
        return Namespace.model_validate(await self.client.request('POST','/api/v1/namespaces',body=encoded(command)))

    async def get(self, name_or_id: str) -> Namespace:
        return Namespace.model_validate(await self.client.request('GET',f'/api/v1/namespaces/{segment(name_or_id)}'))

    async def list(self, *, subject_key: str | None=None) -> tuple[Namespace,...]:
        command=IdentityDirectoryQuery(limit=1000,subject_key=subject_key);values=[]
        while True:
            page=NamespacePage.model_validate(await self.client.request('POST','/api/v1/namespaces/query',body=encoded(command)))
            values.extend(page.items)
            if page.next_cursor is None:return tuple(values)
            command=command.model_copy(update={'cursor':page.next_cursor})

    async def bind_subject(self, namespace_id: str, external_id: str, *, operation_id: str | None=None):
        from .contracts.identity import BoundSubject
        command=BindSubject(external_id=external_id,operation_id=operation_id or str(uuid4()))
        return BoundSubject.model_validate(await self.client.request('POST',f'/api/v1/namespaces/{segment(namespace_id)}/subject-bindings',body=encoded(command)))

    async def change(self, namespace_id: str, command: ChangeNamespace) -> Namespace:
        return Namespace.model_validate(await self.client.request('POST',f'/api/v1/namespaces/{segment(namespace_id)}/change',body=encoded(command)))

class NamespaceClient:
    def __init__(self, client: NousClient, identity: Namespace):
        self.client=client
        self.transport=client.transport
        self.identity=identity
        self.namespace_id=identity.id
        self._source_read=client._source_read
        self.materials=MaterialResource(self)
        self.memories=MemoryResource(self)
        self.jobs=JobResource(self)
        from .settings import SettingsResource
        self.settings=SettingsResource(client,self.namespace_id)
        from .runtime import RuntimeResource
        self.runtime=RuntimeResource(self)

    async def request(self, method: str, path: str, *, body=None, params=None):
        return await self.transport.request(method,path,body=body,params=params,source_read=self._source_read)

    async def query(self, query: str | QueryRequest, *, context: QueryContext | str | None=None, media: tuple[MediaReference,...]=(), profile: str | None=None, limit: int=8, strict: bool=False) -> QueryResult:
        command=query if isinstance(query,QueryRequest) else QueryRequest(text=query,context=context,media=media,wave_profile=profile,limit=limit,capability_policy='strict' if strict else None)
        namespace=self.namespace_id
        return QueryResult.model_validate(await self.request('POST',f'/api/v1/namespaces/{segment(namespace)}/query',body=encoded(command),params={'view':'raw'}))

    async def read(self, ref: str, *, sources: bool=False, cursor: str | None=None, offset: int=0, max_chars: int=12000) -> ReadView:
        namespace=self.namespace_id
        return ReadView.model_validate(await self.request('POST',f'/api/v1/namespaces/{segment(namespace)}/read',body=encoded(ReadRequest(ref=ref,sources=sources,cursor=cursor,offset=offset,max_chars=max_chars))))

    async def sources(self, ref: str) -> ReadView:
        return await self.read(ref,sources=True)

    async def history(self, ref: str, *, limit: int=100) -> tuple[CognitiveRecord,...]:
        namespace=self.namespace_id
        return TypeAdapter(tuple[CognitiveRecord,...]).validate_python(await self.request('GET',f'/api/v1/namespaces/{segment(namespace)}/objects/{segment(ref)}/history',params={'limit':limit}))

    async def remember(self, content: str | RememberInput, *, source: str | None=None, tags: tuple[str,...]=(), pin: bool=False, operation_id: str | None=None) -> CognitiveRecord:
        command=content if isinstance(content,RememberInput) else RememberInput(content=content,source=source,tags=tags,pin=pin,operation_id=operation_id or str(uuid4()))
        namespace=self.namespace_id
        return CognitiveRecord.model_validate(await self.request('POST',f'/api/v1/namespaces/{segment(namespace)}/memories',body=encoded(command)))

    async def correct(self, ref: str, content: str, *, reason: str='', source: str | None=None, operation_id: str | None=None) -> CorrectionResult:
        namespace=self.namespace_id;command=CorrectionInput(ref=ref,content=content,reason=reason,source=source,operation_id=operation_id or str(uuid4()))
        return CorrectionResult.model_validate(await self.request('POST',f'/api/v1/namespaces/{segment(namespace)}/corrections',body=encoded(command)))

    async def ingest(self, content: str | TextImport, *, source: str | None=None, form_memory: bool=False, operation_id: str | None=None) -> IngestionJob:
        command=content if isinstance(content,TextImport) else TextImport(inputs=(ImportDocument(text=content,source=source),),form_memory=form_memory,operation_id=operation_id or str(uuid4()))
        namespace=self.namespace_id
        submission=TextImportSubmission.model_validate(await self.request('POST',f'/api/v1/namespaces/{segment(namespace)}/ingestions',body=encoded(command)))
        return IngestionJob(self,submission)

class IngestionJob:
    def __init__(self,namespace: NamespaceClient,submission: TextImportSubmission):
        self.namespace,self.submission=namespace,submission
        self.ref='job:'+submission.job_id

    async def wait(self, *, timeout_ms: int=45000) -> TextImportResult:
        namespace=self.namespace.namespace_id
        await self.namespace.client.wait_success((self.submission.job_id,),namespace_id=namespace,timeout_ms=timeout_ms)
        return await self.namespace.client.text_import_result(namespace,self.submission.request_id)

class JobResource:
    def __init__(self, namespace: NamespaceClient):self.namespace=namespace
    async def get(self, ref: str):
        return await self.namespace.client.job(ref.removeprefix('job:'),namespace_id=self.namespace.namespace_id)
    async def list(self, *, limit: int=50):
        return await self.namespace.client.jobs(namespace_id=self.namespace.namespace_id,limit=limit)
    async def cancel(self, ref: str):
        return await self.namespace.client.cancel_job(ref.removeprefix('job:'),namespace_id=self.namespace.namespace_id)
    async def resume(self,ref: str):
        return await self.namespace.request('POST',f'/api/v1/jobs/{segment(ref.removeprefix("job:"))}/resume',params={'namespace_id':self.namespace.namespace_id})

class MemoryResource:
    def __init__(self, namespace: NamespaceClient):self.namespace=namespace
    async def get(self, ref: str) -> CognitiveRecord:
        value=await self.namespace.read(ref)
        if value.kind!='memory':raise ValueError('Reference selects another cognitive kind')
        return CognitiveRecord.model_validate(value.value)
    async def list(self, command=None):
        from .contracts.records import RecordsQuery
        selected=command or RecordsQuery()
        return await self.namespace.client.records(self.namespace.namespace_id,selected.model_copy(update={'kinds':('memory',)}))
    async def revise(self, ref: str, content: str, *, reason: str='') -> CorrectionResult:
        return await self.namespace.correct(ref,content,reason=reason)
    async def change_retention(self, command):
        return await self.namespace.client.change_retention(self.namespace.namespace_id,command)
    async def recycle_bin(self, *, limit: int=50):
        from .contracts.records import RecordsQuery
        return await self.namespace.client.records(self.namespace.namespace_id,RecordsQuery(kinds=('memory',),states=('trashed',),purpose='management',limit=limit))

class MaterialResource:
    def __init__(self, namespace: NamespaceClient):self.namespace=namespace
    async def get(self, ref: str):
        from .types import Material
        selected=ref if ':' in ref else 'material:'+ref
        return Material.model_validate((await self.namespace.read(selected)).value)
    async def upload(self, path: str | Path, *, media_type: str='application/octet-stream'):
        return await self.namespace.client.upload_file(self.namespace.namespace_id,path,media_type=media_type)
    async def representations(self,ref: str):
        from .contracts.derivation import DerivedRepresentation
        return TypeAdapter(tuple[DerivedRepresentation,...]).validate_python(await self.namespace.request('GET',f'/api/v1/namespaces/{segment(self.namespace.namespace_id)}/materials/{segment(ref.removeprefix("material:"))}/representations'))
    async def download(self, ref: str, destination: str | Path):
        material=await self.get(ref)
        return await self.namespace.client.download_material(material.id,destination,namespace_id=self.namespace.namespace_id)
