"""Public graph operations retain canonical identities and actual report provenance."""

from pydantic import TypeAdapter

from ..contracts import CognitiveRecord, MergeEntities, SplitEntity
from ..contracts.graph_management import (
    GraphChange,
    GraphChangeResult,
    GraphReadQuery,
    GraphSearchQuery,
    GraphSearchResult,
    GraphSnapshot,
)
from ..contracts.person_profiles import (
    PersonProfileChange,
    PersonProfileChangeResult,
    PersonProfileRefreshSubmission,
    PersonProfilesQuery,
    PersonProfilesResult,
    RefreshPersonProfile,
)
from ..operations import encoded
from .maintenance import MaintenanceClient


class GraphClient(MaintenanceClient):
    async def read_graph(
        self, namespace_id: str,  command: GraphReadQuery
    ) -> GraphSnapshot:
        return GraphSnapshot.model_validate(
            await self.request(
                "POST",
                self.namespace_path(namespace_id) + "/graph/query",
                body=encoded(command),
            )
        )

    async def search_graph(
        self, namespace_id: str,  command: GraphSearchQuery
    ) -> GraphSearchResult:
        return GraphSearchResult.model_validate(
            await self.request(
                "POST",
                self.namespace_path(namespace_id) + "/graph/search",
                body=encoded(command),
            )
        )

    async def person_profiles(
        self, namespace_id: str,  command: PersonProfilesQuery
    ) -> PersonProfilesResult:
        return PersonProfilesResult.model_validate(
            await self.request(
                "POST",
                self.namespace_path(namespace_id) + "/person-profiles/query",
                body=encoded(command),
            )
        )

    async def merge_entities(
        self, namespace_id: str,  command: MergeEntities
    ) -> tuple[CognitiveRecord, ...]:
        return TypeAdapter(tuple[CognitiveRecord, ...]).validate_python(
            await self.request(
                "POST",
                self.namespace_path(namespace_id) + "/entities/merge",
                body=encoded(command),
            )
        )

    async def split_entity(
        self, namespace_id: str,  command: SplitEntity
    ) -> tuple[CognitiveRecord, ...]:
        return TypeAdapter(tuple[CognitiveRecord, ...]).validate_python(
            await self.request(
                "POST",
                self.namespace_path(namespace_id) + "/entities/split",
                body=encoded(command),
            )
        )

    async def change_person_profile(
        self,
        namespace_id: str,
        
        command: PersonProfileChange,
    ) -> PersonProfileChangeResult:
        return PersonProfileChangeResult.model_validate(
            await self.request(
                "POST",
                self.namespace_path(namespace_id) + "/person-profiles/change",
                body=encoded(command),
            )
        )

    async def refresh_person_profile(
        self,
        namespace_id: str,
        
        command: RefreshPersonProfile,
    ) -> PersonProfileRefreshSubmission:
        return PersonProfileRefreshSubmission.model_validate(
            await self.request(
                "POST",
                self.namespace_path(namespace_id) + "/person-profiles/refresh",
                body=encoded(command),
            )
        )

    async def graph_snapshot(self, namespace_id: str) -> GraphSnapshot:
        return GraphSnapshot.model_validate(
            await self.request("GET", self.namespace_path(namespace_id) + "/graph")
        )

    async def manage_graph(
        self, namespace_id: str,  command: GraphChange
    ) -> GraphChangeResult:
        return GraphChangeResult.model_validate(
            await self.request(
                "POST",
                self.namespace_path(namespace_id) + "/graph",
                body=encoded(command),
            )
        )
