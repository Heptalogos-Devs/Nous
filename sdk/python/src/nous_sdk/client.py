"""Asynchronous typed client for Nous domain services."""

from ._client.episodes import EpisodeClient
from ._client.facts import FactClient
from ._client.graph import GraphClient


class NousClient(GraphClient, EpisodeClient, FactClient):
    """Public client for the shared Nous service contracts."""

    def __init__(self,*args,**kwargs):
        super().__init__(*args,**kwargs)
        from .namespace import NamespaceDirectory
        self.namespaces=NamespaceDirectory(self)

    async def namespace(self,name_or_id: str):
        from .namespace import NamespaceClient
        return NamespaceClient(self,await self.namespaces.get(name_or_id))

    async def namespace_by_id(self,namespace_id: str):
        from uuid import UUID
        from .namespace import NamespaceClient
        return NamespaceClient(self,await self.namespaces.get(str(UUID(namespace_id))))
