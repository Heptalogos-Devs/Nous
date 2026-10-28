"""Typed identity client operations."""

from __future__ import annotations

from collections.abc import Sequence
from uuid import uuid4

from pydantic import TypeAdapter

from ..operations import object_value, segment
from ..types import (
    JsonObject,
    Namespace,
    Subject,
)
from .core import ClientCore


class IdentityClient(ClientCore):
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

    async def subject(self, subject_id: str) -> Subject:
        return Subject.model_validate(
            await self.request("GET", f"/api/subjects/{segment(subject_id)}")
        )

    async def subject_diagnostics(self, subject_id: str) -> JsonObject:
        return object_value(
            await self.request("GET", f"/api/subjects/{segment(subject_id)}/diagnostics")
        )
