"""Canonical source-backed claim writes and scoped fact views."""

from pydantic import TypeAdapter

from ..contracts import CognitiveRecord, CognitiveRef
from ..contracts.facts import FactChange, FactChangeResult, FactQuery, FactRead, FactView
from ..operations import encoded
from .core import ClientCore


class FactClient(ClientCore):
    async def change_fact(
        self, namespace_id: str,  command: FactChange
    ) -> FactChangeResult:
        return FactChangeResult.model_validate(
            await self.request(
                "POST",
                self.namespace_path(namespace_id) + "/facts/change",
                body=command.model_dump(
                    mode="json", include=command.model_fields_set | {"operation_id"}
                ),
            )
        )

    async def query_facts(
        self, namespace_id: str,  command: FactQuery
    ) -> tuple[CognitiveRecord, ...]:
        return TypeAdapter(tuple[CognitiveRecord, ...]).validate_python(
            await self.request(
                "POST",
                self.namespace_path(namespace_id) + "/facts/query",
                body=encoded(command),
            )
        )

    async def fact(
        self, namespace_id: str,  reference: CognitiveRef | str
    ) -> FactView:
        return FactView.model_validate(
            await self.request(
                "POST",
                self.namespace_path(namespace_id) + "/facts/get",
                body=encoded(FactRead(reference=reference)),
            )
        )
