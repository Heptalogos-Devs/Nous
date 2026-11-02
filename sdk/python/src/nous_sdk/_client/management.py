"""Typed management client operations."""

from __future__ import annotations

from collections.abc import Mapping
from uuid import uuid4

from pydantic import JsonValue, TypeAdapter

from ..contracts.configuration import ConfigurationCatalog, ConfigurationDefinition
from ..contracts.model_roles import (
    ModelContractRequest,
    ModelInvocationPreview,
    ModelPreviewRequest,
    ModelRoleContract,
)
from ..errors import NousError
from ..operations import object_value, segment
from ..types import (
    JsonObject,
    QueryParam,
)
from .requests import resolve_scope
from .sources import SourceClient


class ManagementClient(SourceClient):
    async def configuration_catalog(self) -> ConfigurationCatalog:
        return ConfigurationCatalog.model_validate(
            await self.request(
                "GET",
                "/api/manage/configuration",
                params={
                    "namespace_id": self.namespace_id or "instance",
                    
                },
            )
        )

    async def configuration_schema(self, key: str) -> ConfigurationDefinition:
        return ConfigurationDefinition.model_validate(
            await self.request(
                "GET",
                f"/api/manage/configuration/{segment(key)}/schema",
                params={
                    "namespace_id": self.namespace_id or "instance",
                    
                },
            )
        )

    async def configuration(self, request: Mapping[str, JsonValue]) -> JsonObject:
        action = request.get("action", "get")
        key = request.get("key", "models")
        if not isinstance(key, str):
            raise ValueError("configuration key must be text")
        scope_request = dict(request)
        if request.get("scope") == "instance" and "namespace_id" not in scope_request:
            scope_request["namespace_id"] = "instance"
        if request.get("scope") in {"instance", "namespace"} and 'namespace_id' not in scope_request:
            scope_request['namespace_id'] = None
        namespace = resolve_scope(self, scope_request, default_namespace="instance")
        params: dict[str, QueryParam] = {"namespace_id": namespace}
        if action in {"list", "schema"}:
            return object_value(
                await self.request(
                    "GET",
                    "/api/manage/configuration"
                    if action == "list"
                    else f"/api/manage/configuration/{segment(key)}/schema",
                    params=params,
                )
            )
        if action in {"get", "effective"}:
            return object_value(
                await self.request("GET", f"/api/manage/configuration/{segment(key)}", params=params)
            )
        if action in {"set", "reset"}:
            return object_value(
                await self.request(
                    "PUT",
                    f"/api/manage/configuration/{segment(key)}",
                    params=params,
                    body={
                        "value": None if action == "reset" else request.get("value"),
                        "scope": request.get(
                            "scope", "namespace" if params["namespace_id"] != "instance" else "instance"
                        ),
                        "expected_version": request.get("expected_version"),
                    },
                )
            )
        raise ValueError("Unknown configuration action")

    async def prompts(self, request: Mapping[str, JsonValue]) -> JsonValue:
        action = request.get("action", "get")
        prompt_id = request.get("id", "formation")
        if not isinstance(prompt_id, str):
            raise ValueError("prompt ID must be text")
        namespace = resolve_scope(self, request, default_namespace="instance")
        params: dict[str, QueryParam] = {"namespace_id": namespace}
        if action == "list":
            return await self.request("GET", "/api/manage/prompts", params=params)
        if action in {"get", "history"}:
            return await self.request(
                "GET",
                f"/api/manage/prompts/{segment(prompt_id)}" + ("/history" if action == "history" else ""),
                params=params,
            )
        if action in {"save", "put"}:
            return await self.request(
                "PUT",
                f"/api/manage/prompts/{segment(prompt_id)}",
                params=params,
                body={
                    "template": request.get("template"),
                    "operation_id": request.get("operation_id") or str(uuid4()),
                },
            )
        if action == "activate":
            return await self.request(
                "POST",
                f"/api/manage/prompts/{segment(prompt_id)}/activate",
                params=params,
                body={
                    "revision": request.get("revision"),
                    "operation_id": request.get("operation_id") or str(uuid4()),
                },
            )
        if action in {"preview", "render"}:
            return await self.request(
                "POST",
                f"/api/manage/prompts/{segment(prompt_id)}/preview",
                params=params,
                body={"variables": request.get("variables", {})},
            )
        raise ValueError("Unknown prompt action")

    async def model_contract(
        self, request: ModelContractRequest | Mapping[str, JsonValue]
    ) -> ModelRoleContract:
        command = (
            request
            if isinstance(request, ModelContractRequest)
            else ModelContractRequest.model_validate(
                {
                    key: value
                    for key, value in request.items()
                    if key not in {"namespace_id", 'namespace_id'}
                }
            )
        )
        namespace = resolve_scope(
            self, request if isinstance(request, Mapping) else {}, default_namespace="instance"
        )
        return ModelRoleContract.model_validate(
            await self.request(
                "POST",
                "/api/manage/models/contract",
                params={"namespace_id": namespace},
                body=command.model_dump(mode="json"),
            )
        )

    async def model_preview(
        self, request: ModelPreviewRequest | Mapping[str, JsonValue]
    ) -> ModelInvocationPreview:
        command = (
            request
            if isinstance(request, ModelPreviewRequest)
            else ModelPreviewRequest.model_validate(
                {
                    key: value
                    for key, value in request.items()
                    if key not in {"namespace_id", 'namespace_id'}
                }
            )
        )
        namespace = resolve_scope(
            self, request if isinstance(request, Mapping) else {}, default_namespace="instance"
        )
        return ModelInvocationPreview.model_validate(
            await self.request(
                "POST",
                "/api/manage/models/preview",
                params={"namespace_id": namespace},
                body=command.model_dump(mode="json"),
            )
        )

    async def backup(self) -> JsonObject:
        return object_value(await self.request("POST", "/api/manage/backups"))
