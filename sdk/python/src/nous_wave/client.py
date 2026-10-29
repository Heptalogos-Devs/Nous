"""Asynchronous typed client for Nous domain services."""

from ._client.graph import GraphClient


class NousClient(GraphClient):
    """Public client for the shared Nous service contracts."""
