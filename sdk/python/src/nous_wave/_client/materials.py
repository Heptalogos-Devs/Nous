"""Typed materials client operations."""

from __future__ import annotations

from collections.abc import AsyncGenerator, AsyncIterable, Mapping
from pathlib import Path
from uuid import uuid4

import anyio

from ..contracts import (
    TimePoint,
)
from ..errors import NousError
from ..operations import encoded, object_value, segment
from ..types import (
    Download,
    JsonObject,
    Material,
    MaterialReceipt,
    QueryParam,
)
from .identity import IdentityClient


class MaterialClient(IdentityClient):
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

    async def download_region(
        self, namespace_id: str, subject_id: str, region_id: str, destination: str | Path
    ) -> Download:
        from ..contracts import SourceRegion

        region = SourceRegion.model_validate(
            await self.request(
                "GET",
                self.subject_path(namespace_id, subject_id) + f"/regions/{segment(region_id)}",
            )
        )
        return await self.transport.download(
            region.material_id,
            Path(destination),
            self._source_read,
            namespace_id=namespace_id,
            subject_id=subject_id,
            region_id=region_id,
        )

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
