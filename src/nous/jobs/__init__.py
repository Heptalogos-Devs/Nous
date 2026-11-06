"""Durable maintenance jobs and their runtime lifecycle."""

from typing import TYPE_CHECKING

from .contracts import JobOutcome as JobOutcome
from .contracts import JobRef as JobRef
from .contracts import JobView as JobView

if TYPE_CHECKING:
    from .client import open_job_client as open_job_client

__all__ = ["JobOutcome", "JobRef", "JobView", "open_job_client"]


def __getattr__(name: str) -> object:
    if name == "open_job_client":
        from .client import open_job_client

        return open_job_client
    raise AttributeError(name)
