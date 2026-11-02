"""One Namespace binding for complete SDK operations."""
from __future__ import annotations

from collections.abc import Mapping
from pathlib import Path
from typing import TYPE_CHECKING, Literal
from uuid import uuid4

from pydantic import JsonValue, TypeAdapter

from .contracts import CognitiveRecord, ImportDocument, QueryRequest, QueryResult
from .contracts.bundles import BundlePlacement
from .contracts.common import FormationPurpose, TimePoint
from .contracts.corrections import CorrectionResult
from .contracts.identity import (
    BindSubject,
    ChangeNamespace,
    CreateNamespace,
    IdentityDirectoryQuery,
    Namespace,
    NamespacePage,
)
from .contracts.management import (
    BundleInstallation,
    BundleRemoval,
    BundleRequest,
    InstallBundle,
    NamespaceBundle,
)
from .contracts.media import MediaReference
from .contracts.public import CorrectionInput, ReadRequest, ReadView, RememberInput
from .contracts.query_context import QueryContext
from .contracts.text_import import TextImport, TextImportResult, TextImportSubmission
from .operations import encoded, segment
from .types import QueryParam

if TYPE_CHECKING:
    from .client import NousClient

class NamespaceDirectory:
    def __init__(self, client: NousClient):
        self.client=client

    async def create(self, slug: str, *, display_name: str | None=None, mode: Literal['knowledge','subject']='knowledge', subject_key: str | None=None, enabled_features: tuple[str,...] | None=None, operation_id: str | None=None) -> Namespace:
        command=CreateNamespace(slug=slug,display_name=display_name,mode=mode,subject_key=subject_key,enabled_features=enabled_features,operation_id=operation_id or str(uuid4()))
        return Namespace.model_validate(await self.client.request('POST','/api/manage/namespaces',body=encoded(command)))

    async def get(self, name_or_id: str) -> Namespace:
        return Namespace.model_validate(await self.client.request('GET',f'/api/manage/namespaces/{segment(name_or_id)}'))

    async def list(self, *, subject_key: str | None=None) -> tuple[Namespace,...]:
        command=IdentityDirectoryQuery(limit=1000,subject_key=subject_key)
        values=[]
        while True:
            page=NamespacePage.model_validate(await self.client.request('POST','/api/manage/namespaces/query',body=encoded(command)))
            values.extend(page.items)
            if page.next_cursor is None:
                return tuple(values)
            command=command.model_copy(update={'cursor':page.next_cursor})

    async def bind_subject(self, namespace_id: str, external_id: str, *, operation_id: str | None=None):
        from .contracts.identity import BoundSubject
        command=BindSubject(external_id=external_id,operation_id=operation_id or str(uuid4()))
        return BoundSubject.model_validate(await self.client.request('POST',f'/api/manage/namespaces/{segment(namespace_id)}/subject-bindings',body=encoded(command)))

    async def change(self, namespace_id: str, command: ChangeNamespace) -> Namespace:
        return Namespace.model_validate(await self.client.request('POST',f'/api/manage/namespaces/{segment(namespace_id)}/change',body=encoded(command)))

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
        self.bundles: BundleResource = BundleResource(self)
        from .settings import SettingsResource
        self.settings=SettingsResource(client,self.namespace_id)
        from .runtime import RuntimeResource
        self.runtime=RuntimeResource(self)

    async def request(self, method: str, path: str, *, body: JsonValue = None, params: Mapping[str, QueryParam] | None = None) -> JsonValue:
        return await self.transport.request(method,path,body=body,params=params,source_read=self._source_read)

    async def query(self, query: str | QueryRequest, *, context: QueryContext | str | None=None, media: tuple[MediaReference,...]=(), profile: str | None=None, limit: int=8, strict: bool=False) -> QueryResult:
        command=query if isinstance(query,QueryRequest) else QueryRequest(text=query,context=context,media=media,wave_profile=profile,limit=limit,capability_policy='strict' if strict else None)
        namespace=self.namespace_id
        return QueryResult.model_validate(await self.request('POST',f'/api/knowledge/namespaces/{segment(namespace)}/query',body=encoded(command),params={'view':'raw'}))

    async def read(self, ref: str, *, sources: bool=False, cursor: str | None=None, offset: int=0, max_chars: int=12000) -> ReadView:
        namespace=self.namespace_id
        return ReadView.model_validate(await self.request('POST',f'/api/knowledge/namespaces/{segment(namespace)}/read',body=encoded(ReadRequest(ref=ref,sources=sources,cursor=cursor,offset=offset,max_chars=max_chars))))

    async def resolve_reference(self, reference: str):
        return await self.client.resolve_reference(self.namespace_id, reference)

    async def history(self, ref: str, *, limit: int=100) -> tuple[CognitiveRecord,...]:
        namespace=self.namespace_id
        return TypeAdapter(tuple[CognitiveRecord,...]).validate_python(await self.request('GET',f'/api/knowledge/namespaces/{segment(namespace)}/objects/{segment(ref)}/history',params={'limit':limit}))

    async def remember(self, content: str | RememberInput, *, source: str | None=None, tags: tuple[str,...]=(), pin: bool=False, operation_id: str | None=None) -> CognitiveRecord:
        command=content if isinstance(content,RememberInput) else RememberInput(content=content,source=source,tags=tags,pin=pin,operation_id=operation_id or str(uuid4()))
        namespace=self.namespace_id
        return CognitiveRecord.model_validate(await self.request('POST',f'/api/knowledge/namespaces/{segment(namespace)}/memories',body=encoded(command)))

    async def correct(self, ref: str, content: str, *, reason: str='', source: str | None=None, operation_id: str | None=None) -> CorrectionResult:
        namespace=self.namespace_id
        command=CorrectionInput(ref=ref,content=content,reason=reason,source=source,operation_id=operation_id or str(uuid4()))
        return CorrectionResult.model_validate(await self.request('POST',f'/api/knowledge/namespaces/{segment(namespace)}/corrections',body=encoded(command)))

    async def ingest(self, content: str | TextImport, *, source: str | None=None, formation_purpose: FormationPurpose | None=None, operation_id: str | None=None) -> IngestionJob:
        command=content if isinstance(content,TextImport) else TextImport(inputs=(ImportDocument(text=content,source=source),),formation_purpose=formation_purpose,operation_id=operation_id or str(uuid4()))
        namespace=self.namespace_id
        submission=TextImportSubmission.model_validate(await self.request('POST',f'/api/knowledge/namespaces/{segment(namespace)}/ingestions',body=encoded(command)))
        return IngestionJob(self,submission)

class IngestionJob:
    def __init__(self,namespace: NamespaceClient,submission: TextImportSubmission):
        self.namespace,self.submission=namespace,submission
        self.ref='job:'+submission.job_id

    async def wait(self, *, timeout_ms: int=45000) -> TextImportResult:
        await self.namespace.jobs.wait(self.submission.job_id, timeout_ms=timeout_ms)
        return await self.namespace.client.text_import_result(
            self.namespace.namespace_id, self.submission.request_id
        )

class JobResource:
    def __init__(self, namespace: NamespaceClient):self.namespace=namespace
    async def get(self, ref: str, *, purpose: Literal["actor", "management"] = "actor"):
        return await self.namespace.client.job(
            ref.removeprefix("job:"), namespace_id=self.namespace.namespace_id, purpose=purpose
        )
    async def list(
        self,
        *,
        limit: int = 50,
        scope: str | None = None,
        kind: str | None = None,
        state: str | None = None,
        purpose: Literal["actor", "management"] = "actor",
    ):
        return await self.namespace.client.jobs(
            namespace_id=self.namespace.namespace_id,
            limit=limit,
            scope=scope,
            kind=kind,
            state=state,
            purpose=purpose,
        )
    async def page(
        self,
        *,
        limit: int = 50,
        cursor: str | None = None,
        scope: str | None = None,
        kind: str | None = None,
        state: str | None = None,
        purpose: Literal["actor", "management"] = "actor",
    ):
        return await self.namespace.client.job_page(
            namespace_id=self.namespace.namespace_id,
            limit=limit,
            cursor=cursor,
            scope=scope,
            kind=kind,
            state=state,
            purpose=purpose,
        )
    async def cancel(self, ref: str, *, purpose: Literal["actor", "management"] = "actor"):
        return await self.namespace.client.cancel_job(
            ref.removeprefix("job:"), namespace_id=self.namespace.namespace_id, purpose=purpose
        )
    async def resume(self, ref: str, *, purpose: Literal["actor", "management"] = "actor"):
        return await self.namespace.request(
            'POST',
            f'/api/runtime/namespaces/{segment(self.namespace.namespace_id)}/jobs/{segment(ref.removeprefix("job:"))}/resume',
            params={"purpose": purpose},
        )
    async def wait(
        self,
        ref: str,
        *,
        timeout_ms: int = 45000,
        purpose: Literal["actor", "management"] = "actor",
    ):
        return await self.namespace.client.wait_success(
            (ref.removeprefix("job:"),),
            namespace_id=self.namespace.namespace_id,
            timeout_ms=timeout_ms,
            purpose=purpose,
        )

class MemoryResource:
    def __init__(self, namespace: NamespaceClient):self.namespace=namespace
    async def get(self, ref: str) -> CognitiveRecord:
        value=await self.namespace.read(ref)
        if value.kind!='memory':
            raise ValueError('Reference selects another cognitive kind')
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
        return TypeAdapter(tuple[DerivedRepresentation,...]).validate_python(await self.namespace.request('GET',f'/api/knowledge/namespaces/{segment(self.namespace.namespace_id)}/materials/{segment(ref.removeprefix("material:"))}/representations'))
    async def observe(self, material_id: str, *, observed_at: TimePoint | None = None, origin: dict[str, str] | None = None):
        body: dict[str, JsonValue] = {
            "material_id": material_id,
            "observed_at": encoded(observed_at) if observed_at is not None else None,
            "origin": dict(origin or {}),
        }
        return await self.namespace.request(
            "POST",
            f"/api/knowledge/namespaces/{segment(self.namespace.namespace_id)}/observations",
            body=body,
        )
    async def download(self, ref: str, destination: str | Path):
        material=await self.get(ref)
        return await self.namespace.client.download_material(material.id,destination,namespace_id=self.namespace.namespace_id)


class BundleResource:
    def __init__(self, namespace: NamespaceClient) -> None:
        self.namespace: NamespaceClient = namespace

    @property
    def path(self) -> str:
        return f"/api/knowledge/namespaces/{segment(self.namespace.namespace_id)}/bundles"

    async def export(self, command: BundleRequest | None = None) -> NamespaceBundle:
        return NamespaceBundle.model_validate(
            await self.namespace.request(
                "POST", self.path + "/export", body=encoded(command or BundleRequest())
            )
        )

    async def install(
        self,
        bundle: NamespaceBundle,
        *,
        mode: Literal["merge", "restore"] = "merge",
        placement: BundlePlacement | None = None,
    ) -> BundleInstallation:
        command = InstallBundle(
            bundle=bundle.model_dump(mode="json"),
            mode=mode,
            placement=placement or BundlePlacement(),
        )
        return BundleInstallation.model_validate(
            await self.namespace.request("POST", self.path + "/install", body=encoded(command))
        )

    async def installations(self) -> tuple[BundleInstallation, ...]:
        return TypeAdapter(tuple[BundleInstallation, ...]).validate_python(
            await self.namespace.request("GET", self.path + "/installations")
        )

    async def installation(self, installation_id: str) -> BundleInstallation:
        return BundleInstallation.model_validate(
            await self.namespace.request(
                "GET", self.path + f"/installations/{segment(installation_id)}"
            )
        )

    async def uninstall(self, installation_id: str) -> BundleRemoval:
        return BundleRemoval.model_validate(
            await self.namespace.request(
                "POST", self.path + f"/installations/{segment(installation_id)}/uninstall"
            )
        )
