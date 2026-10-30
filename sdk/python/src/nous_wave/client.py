"""Asynchronous typed client for Nous domain services."""

from ._client.episodes import EpisodeClient
from ._client.graph import GraphClient


class NousClient(GraphClient, EpisodeClient):
    """Public client for the shared Nous service contracts."""
