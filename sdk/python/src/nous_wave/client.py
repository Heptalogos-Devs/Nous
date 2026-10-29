"""Asynchronous typed client for Nous domain services."""

from ._client.maintenance import MaintenanceClient


class NousClient(MaintenanceClient):
    """Public client for the shared Nous service contracts."""
