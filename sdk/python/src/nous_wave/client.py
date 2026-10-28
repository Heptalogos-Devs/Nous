"""Asynchronous typed client for Nous domain services."""

from ._client.management import ManagementClient


class NousClient(ManagementClient):
    """Public client for the shared Nous service contracts."""
