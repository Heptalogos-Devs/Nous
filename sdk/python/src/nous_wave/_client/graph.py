"""Public graph operations retain canonical identities and actual report provenance."""

from ..contracts.graph_management import GraphChange, GraphChangeResult, GraphSnapshot
from ..operations import encoded
from .maintenance import MaintenanceClient


class GraphClient(MaintenanceClient):
    async def graph_snapshot(self, namespace_id: str, subject_id: str) -> GraphSnapshot:
        return GraphSnapshot.model_validate(
            await self.request("GET", self.subject_path(namespace_id, subject_id) + "/graph")
        )

    async def manage_graph(
        self, namespace_id: str, subject_id: str, command: GraphChange
    ) -> GraphChangeResult:
        return GraphChangeResult.model_validate(
            await self.request(
                "POST",
                self.subject_path(namespace_id, subject_id) + "/graph",
                body=encoded(command),
            )
        )
