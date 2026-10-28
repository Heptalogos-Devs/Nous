"""Asynchronous typed client for Nous domain services."""

from collections.abc import AsyncGenerator, AsyncIterable, Mapping, Sequence
from copy import copy
from datetime import datetime
from pathlib import Path
from typing import Literal, Self
from uuid import uuid4

import anyio
from pydantic import JsonValue, TypeAdapter

from .contracts import (
    CatalogView,
    CognitiveRecord,
    CognitiveRef,
    FreezeCatalog,
    ImportDocument,
    ImportResult,
    MediaReference,
    ProcessCatalog,
    QueryRequest,
    QueryResult,
    TimePoint,
)
from .contracts.context import ContextReport, ContextReportResult, ContextUse
from .errors import NousError
from .operations import TypedOperations, encoded, object_value, segment
from .source_scope import SourceReadScope
from .transport import ClientOptions, TokenProvider, Transport, token_file
from .types import (
    Download,
    ImportSubmission,
    Job,
    JobDetail,
    JsonObject,
    Material,
    MaterialReceipt,
    Namespace,
    QueryParam,
    SkippedFiles,
    Subject,
)


class NousClient(TypedOperations):
    def __init__(
        self,
        url: str,
        *,
        token: TokenProvider | None = None,
        credential_file: str | Path | None = None,
        options: ClientOptions | None = None,
        namespace_id: str | None = None,
        subject_id: str | None = None,
    ) -> None:
        if (token is None) == (credential_file is None):
            raise ValueError("supply token or credential_file")
        if token is None:
            assert credential_file is not None
            token = token_file(credential_file)
        self.transport = Transport(url, token, options or ClientOptions())
        self.namespace_id, self.subject_id = namespace_id, subject_id
        self._source_read: tuple[SourceReadScope, ...] = ()
        self._owns_transport = True
        self._closed = False

    def with_source_read(self, scope: SourceReadScope | Mapping[str, JsonValue]) -> Self:
        parsed = (
            scope if isinstance(scope, SourceReadScope) else SourceReadScope.model_validate(scope)
        )
        bound = copy(self)
        bound._source_read = (*self._source_read, parsed)
        bound._owns_transport = False
        return bound

    async def close(self) -> None:
        self._closed = True
        if self._owns_transport:
            await self.transport.close()

    async def __aenter__(self) -> Self:
        return self

    async def __aexit__(self, exc_type: object, exc: object, traceback: object) -> None:
        await self.close()

    async def request(
        self,
        method: str,
        path: str,
        *,
        body: JsonValue = None,
        params: Mapping[str, QueryParam] | None = None,
        trace_id: str | None = None,
    ) -> JsonValue:
        if self._closed:
            raise RuntimeError("Nous client is closed")
        return await self.transport.request(
            method, path, body=body, params=params, trace_id=trace_id, source_read=self._source_read
        )

    async def create_namespace(self, name: str, *, operation_id: str | None = None) -> Namespace:
        return Namespace.model_validate(
            await self.request(
                "POST",
                "/api/namespaces",
                body={"name": name, "operation_id": operation_id or str(uuid4())},
            )
        )

    async def namespaces(self) -> tuple[Namespace, ...]:
        return TypeAdapter(tuple[Namespace, ...]).validate_python(
            await self.request("GET", "/api/namespaces")
        )

    async def create_subject(
        self,
        namespace_id: str,
        *,
        name: str = "default",
        capabilities: Sequence[str] = ("memory",),
        operation_id: str | None = None,
    ) -> Subject:
        return Subject.model_validate(
            await self.request(
                "POST",
                f"/api/namespaces/{segment(namespace_id)}/subjects",
                body={
                    "name": name,
                    "capabilities": list(capabilities),
                    "operation_id": operation_id or str(uuid4()),
                },
            )
        )

    async def subjects(self, namespace_id: str) -> tuple[Subject, ...]:
        return TypeAdapter(tuple[Subject, ...]).validate_python(
            await self.request("GET", f"/api/namespaces/{segment(namespace_id)}/subjects")
        )

    async def subject_diagnostics(self, subject_id: str) -> JsonObject:
        return object_value(
            await self.request("GET", f"/api/subjects/{segment(subject_id)}/diagnostics")
        )

    async def put_text(
        self,
        namespace_id: str,
        text: str,
        *,
        owner_subject_id: str | None = None,
        source: str | None = None,
        event_time: TimePoint | None = None,
        operation_id: str | None = None,
    ) -> Material:
        return Material.model_validate(
            await self.request(
                "POST",
                "/api/materials",
                body={
                    "namespace_id": namespace_id,
                    "subject_id": owner_subject_id or self.subject_id,
                    "text": text,
                    "source": source,
                    "event_time": encoded(event_time) if event_time is not None else None,
                    "operation_id": operation_id or str(uuid4()),
                },
            )
        )

    async def upload_file(
        self,
        namespace_id: str,
        path: str | Path,
        *,
        owner_subject_id: str | None = None,
        media_type: str = "application/octet-stream",
        source: str | None = None,
        event_time: TimePoint | None = None,
        expected_content_hash: str | None = None,
        operation_id: str | None = None,
    ) -> MaterialReceipt:
        path = Path(path)
        async with await anyio.open_file(path, "rb") as file:
            length = (await anyio.Path(path).stat()).st_size

            async def chunks() -> AsyncGenerator[bytes]:
                while data := await file.read(self.transport.options.chunk_bytes):
                    yield data

            return await self.upload_stream(
                namespace_id,
                chunks(),
                byte_len=length,
                owner_subject_id=owner_subject_id,
                media_type=media_type,
                source=source or path.name,
                event_time=event_time,
                expected_content_hash=expected_content_hash,
                operation_id=operation_id,
            )

    async def upload_stream(
        self,
        namespace_id: str,
        content: AsyncIterable[bytes],
        *,
        byte_len: int,
        owner_subject_id: str | None = None,
        media_type: str = "application/octet-stream",
        source: str | None = None,
        event_time: TimePoint | None = None,
        expected_content_hash: str | None = None,
        operation_id: str | None = None,
    ) -> MaterialReceipt:
        if self._closed:
            raise RuntimeError("Nous client is closed")
        owner = owner_subject_id or self.subject_id
        if owner is None:
            raise ValueError("upload requires owner_subject_id")
        params: dict[str, QueryParam] = {
            "namespace_id": namespace_id,
            "owner_subject_id": owner,
            "operation_id": operation_id or str(uuid4()),
        }
        if source is not None:
            params["source"] = source
        if event_time is not None:
            params["event_time"] = event_time.model_dump_json()
        if expected_content_hash is not None:
            params["expected_content_hash"] = expected_content_hash
        return MaterialReceipt.model_validate(
            await self.transport.upload(params, content, media_type, byte_len, self._source_read)
        )

    async def download_material(
        self,
        material_id: str,
        destination: str | Path,
        *,
        namespace_id: str | None = None,
        subject_id: str | None = None,
    ) -> Download:
        namespace, subject = namespace_id or self.namespace_id, subject_id or self.subject_id
        if namespace is None or subject is None:
            metadata = await self.material(material_id)
            namespace = metadata.namespace_id
            subject = subject or await self._material_subject(material_id)
        return await self.transport.download(
            material_id,
            Path(destination),
            self._source_read,
            namespace_id=namespace,
            subject_id=subject,
        )

    async def _material_subject(self, material_id: str) -> str:
        value = object_value(
            await self.request("GET", f"/api/materials/{segment(material_id)}/scope")
        )
        subject = value.get("subject_id")
        if not isinstance(subject, str):
            raise NousError(
                {"code": "invalid_response", "message": "Material scope has no subject"}
            )
        return subject

    async def material(self, material_id: str) -> Material:
        return Material.model_validate(
            await self.request("GET", f"/api/materials/{segment(material_id)}")
        )

    async def observe(
        self,
        subject_id: str,
        material_id: str,
        *,
        observed_at: TimePoint | None = None,
        origin: Mapping[str, str] | None = None,
        operation_id: str | None = None,
    ) -> JsonObject:
        return object_value(
            await self.request(
                "POST",
                "/api/observations",
                body={
                    "subject_id": subject_id,
                    "material_id": material_id,
                    "observed_at": encoded(observed_at) if observed_at is not None else None,
                    "origin": dict(origin or {}),
                    "operation_id": operation_id or str(uuid4()),
                },
            )
        )

    async def query(
        self,
        subject_id: str,
        text: str = "",
        *,
        namespace_id: str | None = None,
        mode: Literal["lexical", "dense", "hybrid", "vcp"] = "lexical",
        kinds: Sequence[str] = (),
        domains: Sequence[str] = ("cognition",),
        source_ids: Sequence[str] | None = None,
        reference: str | None = None,
        history: bool = False,
        as_of_commit: int | None = None,
        limit: int = 10,
    ) -> QueryResult:
        values: JsonObject = {
            "text": text,
            "mode": mode,
            "kinds": list(kinds),
            "domains": list(domains),
            "source_ids": list(source_ids) if source_ids is not None else None,
            "reference": reference,
            "history": history,
            "as_of_commit": as_of_commit,
            "limit": limit,
        }
        command = QueryRequest.model_validate(values)
        namespace = namespace_id or self.namespace_id
        if namespace is not None:
            return await self.query_cognition(namespace, subject_id, command)
        return QueryResult.model_validate(
            await self.request(
                "POST", f"/api/subjects/{segment(subject_id)}/query", body=encoded(command)
            )
        )

    async def memory(self, memory_id: str) -> CognitiveRecord:
        return CognitiveRecord.model_validate(
            await self.request("GET", f"/api/memories/{segment(memory_id)}")
        )

    async def query_manual(self) -> str:
        value = await self.request("GET", "/api/query/manual")
        if not isinstance(value, str):
            raise NousError({"code": "invalid_response", "message": "Query manual is not text"})
        return value

    async def source(
        self,
        subject_id: str,
        reference: CognitiveRef | MediaReference | str,
        *,
        observation_id: str | None = None,
    ) -> JsonObject:
        return object_value(
            await self.request(
                "POST",
                "/api/sources/read",
                body={
                    "subject_id": subject_id,
                    "reference": reference if isinstance(reference, str) else encoded(reference),
                    "observation_id": observation_id,
                },
            )
        )

    async def memory_source(self, memory_id: str, *, revision: int | None = None) -> JsonObject:
        return object_value(
            await self.request(
                "GET", f"/api/memories/{segment(memory_id)}/source", params={"revision": revision}
            )
        )

    async def resolve_reference(
        self,
        subject_id: str,
        reference: CognitiveRef | MediaReference | str,
        *,
        as_of_commit: int | None = None,
    ) -> JsonObject:
        return object_value(
            await self.request(
                "POST",
                "/api/references",
                body={
                    "subject_id": subject_id,
                    "reference": reference if isinstance(reference, str) else encoded(reference),
                    "as_of_commit": as_of_commit,
                },
            )
        )

    async def jobs(
        self, *, namespace_id: str | None = None, subject_id: str | None = None, limit: int = 50
    ) -> tuple[Job, ...]:
        return TypeAdapter(tuple[Job, ...]).validate_python(
            await self.request(
                "GET",
                "/api/jobs",
                params={
                    "namespace_id": namespace_id or self.namespace_id,
                    "subject_id": subject_id or self.subject_id,
                    "limit": limit,
                },
            )
        )

    async def job(
        self, job_id: str, *, namespace_id: str | None = None, subject_id: str | None = None
    ) -> JobDetail:
        return JobDetail.model_validate(
            await self.request(
                "GET",
                f"/api/jobs/{segment(job_id)}",
                params={
                    "namespace_id": namespace_id or self.namespace_id,
                    "subject_id": subject_id or self.subject_id,
                },
            )
        )

    async def cancel_job(
        self, job_id: str, *, namespace_id: str | None = None, subject_id: str | None = None
    ) -> JsonObject:
        return object_value(
            await self.request(
                "POST",
                f"/api/jobs/{segment(job_id)}/cancel",
                params={
                    "namespace_id": namespace_id or self.namespace_id,
                    "subject_id": subject_id or self.subject_id,
                },
            )
        )

    async def wait_jobs(
        self,
        ids: Sequence[str],
        *,
        namespace_id: str | None = None,
        subject_id: str | None = None,
        timeout_ms: int = 45000,
    ) -> tuple[Job, ...]:
        async def wait(job_id: str) -> Job:
            await self.request(
                "POST",
                f"/api/jobs/{segment(job_id)}/wait",
                params={
                    "namespace_id": namespace_id or self.namespace_id,
                    "subject_id": subject_id or self.subject_id,
                    "wait_seconds": timeout_ms / 1000,
                },
            )
            return (await self.job(job_id, namespace_id=namespace_id, subject_id=subject_id)).job

        values: list[Job] = []
        for identity in ids:
            values.append(await wait(identity))
        return tuple(values)

    async def wait_success(
        self,
        ids: Sequence[str],
        *,
        namespace_id: str | None = None,
        subject_id: str | None = None,
        timeout_ms: int = 45000,
    ) -> tuple[Job, ...]:
        jobs = await self.wait_jobs(
            ids, namespace_id=namespace_id, subject_id=subject_id, timeout_ms=timeout_ms
        )
        for job in jobs:
            if job.status != "SUCCESS":
                raise NousError(
                    {
                        "code": "job_failed",
                        "message": f"Job {job.workflow_id} ended as {job.status}",
                        "context": {"job_id": job.workflow_id, "error": job.error},
                    }
                )
        return jobs

    async def process_import_units(
        self,
        namespace_id: str,
        subject_id: str,
        request: ProcessCatalog | Mapping[str, JsonValue],
        *,
        operation_id: str | None = None,
    ) -> ImportSubmission:
        command = (
            request
            if isinstance(request, ProcessCatalog)
            else ProcessCatalog.model_validate(request)
        )
        return await self.submit_catalog(
            namespace_id,
            subject_id,
            command.model_copy(update={"operation_id": operation_id})
            if operation_id is not None
            else command,
        )

    async def freeze_import_catalog(
        self,
        namespace_id: str,
        subject_id: str,
        inputs: Sequence[ImportDocument],
        *,
        operation_id: str | None = None,
        chunk_size: int = 2400,
        chunk_overlap: int = 200,
    ) -> CatalogView:
        return await self.freeze_catalog(
            namespace_id,
            subject_id,
            FreezeCatalog(
                inputs=tuple(inputs),
                operation_id=operation_id or str(uuid4()),
                chunk_size=chunk_size,
                chunk_overlap=chunk_overlap,
            ),
        )

    async def import_files(
        self,
        namespace_id: str,
        subject_id: str,
        files: Sequence[str | Path | Mapping[str, JsonValue]],
        *,
        transfer_root: str | Path,
        input_format: Literal["text", "json", "jsonl"] | None = None,
        form_memory: bool = False,
        completion: Literal["accepted", "formed"] = "accepted",
        origin: Mapping[str, str] | None = None,
        operation_id: str | None = None,
    ) -> ImportResult | SkippedFiles:
        from .imports import import_files

        return await import_files(
            self,
            namespace_id,
            subject_id,
            files,
            transfer_root=transfer_root,
            input_format=input_format,
            form_memory=form_memory,
            completion=completion,
            origin=origin,
            operation_id=operation_id,
        )

    async def import_directory(
        self,
        namespace_id: str,
        subject_id: str,
        *,
        transfer_root: str | Path,
        alias: str = "raw",
        relative_path: str = "",
        pattern: str = "*",
        recursive: bool = True,
        input_format: Literal["text", "json", "jsonl"] | None = None,
        form_memory: bool = False,
        completion: Literal["accepted", "formed"] = "accepted",
        origin: Mapping[str, str] | None = None,
        operation_id: str | None = None,
    ) -> ImportResult | SkippedFiles:
        from .imports import import_directory

        return await import_directory(
            self,
            namespace_id,
            subject_id,
            transfer_root=transfer_root,
            alias=alias,
            relative_path=relative_path,
            pattern=pattern,
            recursive=recursive,
            input_format=input_format,
            form_memory=form_memory,
            completion=completion,
            origin=origin,
            operation_id=operation_id,
        )

    async def report_context(
        self,
        admission_id: str,
        *,
        namespace_id: str,
        subject_id: str,
        invocation_id: str,
        outcome: Literal["completed", "failed", "cancelled"],
        occurred_at: datetime,
        uses: Sequence[ContextUse] = (),
    ) -> ContextReportResult:
        from .managed import event_id

        return await self.report_managed_context(
            namespace_id,
            subject_id,
            ContextReport(
                operation_id=event_id("report", invocation_id, admission_id),
                admission_id=admission_id,
                invocation_id=invocation_id,
                outcome=outcome,
                occurred_at=occurred_at,
                uses=tuple(uses),
            ),
        )

    def managed(
        self,
        *,
        namespace_id: str | None = None,
        subject_id: str,
        session_id: str,
        consumer_id: str,
        instance_id: str,
        source_read: SourceReadScope | Mapping[str, JsonValue] | None = None,
    ):
        from .managed import ManagedBinding

        namespace = namespace_id or self.namespace_id
        if namespace is None:
            raise ValueError("managed binding requires namespace_id")
        return ManagedBinding(
            self,
            namespace_id=namespace,
            subject_id=subject_id,
            session_id=session_id,
            consumer_id=consumer_id,
            instance_id=instance_id,
            source_read=source_read,
        )
