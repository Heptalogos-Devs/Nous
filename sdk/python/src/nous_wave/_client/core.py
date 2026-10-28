"""Typed core client operations."""

from __future__ import annotations

from collections.abc import Mapping
from copy import copy
from pathlib import Path
from typing import Self

from pydantic import JsonValue

from ..operations import TypedOperations
from ..source_scope import SourceReadScope
from ..transport import ClientOptions, TokenProvider, Transport, token_file
from ..types import (
    QueryParam,
)


class ClientCore(TypedOperations):
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
