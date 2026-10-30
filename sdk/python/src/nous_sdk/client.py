"""Asynchronous typed client for Nous domain services."""

from ._client.episodes import EpisodeClient
from ._client.facts import FactClient
from ._client.graph import GraphClient


class NousClient(GraphClient, EpisodeClient, FactClient):
    """Public client for the shared Nous service contracts."""
