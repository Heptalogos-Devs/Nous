import json
from collections.abc import Mapping, Sequence
from copy import copy, deepcopy
from pathlib import Path
from typing import Any, Self
from uuid import uuid4

import anyio

from .errors import NousError
from .transport import ClientOptions, TokenProvider, Transport, token_file


class NousClient:
    def __init__(
        self,
        url: str,
        *,
        token: TokenProvider | None = None,
        credential_file: str | Path | None = None,
        options: ClientOptions | None = None,
    ) -> None:
        if (token is None) == (credential_file is None):
            raise ValueError("supply token or credential_file")
        self.transport = Transport(
            url,
            token if token is not None else token_file(credential_file),
            options or ClientOptions(),
        )
        self._source_read: tuple[Mapping, ...] = ()
        self._owns_transport = True
        self._closed = False

    def with_source_read(self, scope: Mapping) -> "NousClient":
        bound = copy(self)
        bound._source_read = (*self._source_read, deepcopy(dict(scope)))
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
        body: Any = None,
        params: Mapping[str, Any] | None = None,
        trace_id: str | None = None,
    ) -> Any:
        if self._closed:
            raise RuntimeError("Nous client is closed")
        return await self.transport.request(
            method,
            path,
            body=body,
            params=params,
            trace_id=trace_id,
            source_read=self._source_read,
        )

    async def create_namespace(
        self, name: str, *, operation_id: str | None = None
    ) -> dict:
        return await self.request(
            "POST",
            "/api/namespaces",
            body={"name": name, "operation_id": operation_id or str(uuid4())},
        )

    async def namespaces(self) -> list:
        return await self.request("GET", "/api/namespaces")

    async def create_subject(
        self,
        namespace_id: str,
        *,
        name: str = "default",
        capabilities: Sequence[str] = ("memory",),
        operation_id: str | None = None,
    ) -> dict:
        return await self.request(
            "POST",
            f"/api/namespaces/{namespace_id}/subjects",
            body={
                "name": name,
                "capabilities": list(capabilities),
                "operation_id": operation_id or str(uuid4()),
            },
        )

    async def subjects(self, namespace_id: str) -> list:
        return await self.request("GET", f"/api/namespaces/{namespace_id}/subjects")

    async def put_text(
        self,
        namespace_id: str,
        text: str,
        *,
        source: str | None = None,
        event_time: Mapping | None = None,
        operation_id: str | None = None,
    ) -> dict:
        return await self.request(
            "POST",
            "/api/materials",
            body={
                "namespace_id": namespace_id,
                "text": text,
                "source": source,
                "event_time": event_time,
                "operation_id": operation_id or str(uuid4()),
            },
        )

    async def upload_file(
        self,
        namespace_id: str,
        path: str | Path,
        *,
        media_type: str = "application/octet-stream",
        source: str | None = None,
        event_time: Mapping | None = None,
        expected_content_hash: str | None = None,
        operation_id: str | None = None,
    ) -> dict:
        path = Path(path)
        params = {
            "namespace_id": namespace_id,
            "source": source or str(path),
            "operation_id": operation_id or str(uuid4()),
        }
        if event_time is not None:
            params["event_time"] = json.dumps(
                event_time, ensure_ascii=False, separators=(",", ":")
            )
        if expected_content_hash is not None:
            params["expected_content_hash"] = expected_content_hash
        async with await anyio.open_file(path, "rb") as file:
            length = (await anyio.Path(path).stat()).st_size

            async def chunks():
                while data := await file.read(self.transport.options.chunk_bytes):
                    yield data

            return await self.transport.upload(
                params, chunks(), media_type, length, self._source_read
            )

    async def download_material(
        self, material_id: str, destination: str | Path
    ) -> dict:
        return await self.transport.download(
            material_id, Path(destination), self._source_read
        )

    async def material(self, material_id: str) -> dict:
        return await self.request("GET", f"/api/materials/{material_id}")

    async def observe(
        self,
        subject_id: str,
        material_id: str,
        *,
        observed_at: Mapping | None = None,
        origin: Mapping | None = None,
        operation_id: str | None = None,
    ) -> dict:
        return await self.request(
            "POST",
            "/api/observations",
            body={
                "subject_id": subject_id,
                "material_id": material_id,
                "observed_at": observed_at,
                "origin": origin,
                "operation_id": operation_id or str(uuid4()),
            },
        )

    async def import_text(self, request: Mapping[str, Any]) -> dict:
        return await self.request("POST", "/api/imports/text", body=dict(request))

    async def query(
        self,
        subject_id: str,
        text: str = "",
        *,
        media: Sequence[Mapping] = (),
        mode: str | None = None,
        domains: Sequence[str] = (),
        **selection: Any,
    ) -> dict:
        return await self.request(
            "POST",
            "/api/query",
            body={
                "subject_id": subject_id,
                "mode": mode or ("dense" if media else "lexical"),
                "input": {"text": text, "media": list(media)},
                "domains": list(domains),
                **selection,
            },
        )

    async def memory(self, memory_id: str) -> dict:
        return await self.request("GET", f"/api/memories/{memory_id}")

    async def memory_source(self, memory_id: str) -> dict:
        return await self.request("GET", f"/api/memories/{memory_id}/source")

    async def form_memory(
        self,
        subject_id: str,
        observation_id: str,
        *,
        source: Mapping | None = None,
        operation_id: str | None = None,
    ) -> dict:
        return await self.request(
            "POST",
            "/api/memories/form",
            body={
                "subject_id": subject_id,
                "observation_id": observation_id,
                "source": source,
                "operation_id": operation_id or str(uuid4()),
            },
        )

    async def change_memory(self, memory_id: str, request: Mapping[str, Any]) -> dict:
        return await self.request(
            "POST", f"/api/memories/{memory_id}/change", body=dict(request)
        )

    async def embedding_sync(
        self, subject_id: str, *, limit: int = 5000, operation_id: str | None = None
    ) -> dict:
        return await self.request(
            "POST",
            "/api/embeddings/sync",
            body={
                "subject_id": subject_id,
                "limit": limit,
                "operation_id": operation_id or str(uuid4()),
            },
        )

    async def job(self, job_id: str) -> dict:
        return await self.request("GET", f"/api/jobs/{job_id}")

    async def cancel_job(self, job_id: str, *, operation_id: str | None = None) -> dict:
        return await self.request(
            "POST",
            f"/api/jobs/{job_id}/cancel",
            body={"operation_id": operation_id or str(uuid4())},
        )

    async def wait_jobs(
        self, ids: Sequence[str], *, timeout_ms: int = 45000
    ) -> list[dict]:
        return await self.request(
            "POST", "/api/jobs/wait", body={"ids": list(ids), "timeout_ms": timeout_ms}
        )

    async def wait_success(
        self, ids: Sequence[str], *, timeout_ms: int = 45000
    ) -> list[dict]:
        jobs = await self.wait_jobs(ids, timeout_ms=timeout_ms)
        for job in jobs:
            if job["state"] != "succeeded":
                raise NousError(
                    job.get("error")
                    or {
                        "code": "dependency_unavailable",
                        "message": f"job {job['id']} is {job['state']}",
                        "context": {"job": job},
                    }
                )
        return jobs

    async def contexts(self, request: Mapping[str, Any]) -> dict:
        return await self.request("POST", "/api/contexts", body=dict(request))

    async def media(self, request: Mapping[str, Any]) -> dict:
        return await self.request("POST", "/api/media", body=dict(request))

    async def cognition(self, request: Mapping[str, Any]) -> dict:
        return await self.request("POST", "/api/cognition", body=dict(request))

    async def configuration(self, request: Mapping[str, Any]) -> dict:
        return await self.request("POST", "/api/config", body=dict(request))

    async def prompts(self, request: Mapping[str, Any]) -> dict:
        return await self.request("POST", "/api/prompts", body=dict(request))

    async def identity(self, request: Mapping[str, Any]) -> dict:
        return await self.request("POST", "/api/identity", body=dict(request))

    async def model_contract(self, request: Mapping[str, Any]) -> dict:
        return await self.request("POST", "/api/models/contract", body=dict(request))

    async def model_preview(self, request: Mapping[str, Any]) -> dict:
        return await self.request("POST", "/api/models/preview", body=dict(request))

    async def model_call(self, call_id: str) -> dict:
        return await self.request("GET", f"/api/models/calls/{call_id}")

    async def backup(self) -> dict:
        return await self.request("POST", "/api/backups")

    def managed(
        self,
        *,
        subject_id: str,
        session_id: str,
        consumer_id: str,
        instance_id: str,
        source_read: Mapping | None = None,
    ):
        from .managed import ManagedBinding

        return ManagedBinding(
            self,
            subject_id=subject_id,
            session_id=session_id,
            consumer_id=consumer_id,
            instance_id=instance_id,
            source_read=source_read,
        )
