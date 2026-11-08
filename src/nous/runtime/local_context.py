"""A local CLI request may borrow, but never own, a running host's Runtime."""

from contextvars import ContextVar
from typing import TYPE_CHECKING

if TYPE_CHECKING:
    from nous.transports.cli.local_control import HostCall

current_call: ContextVar[HostCall | None] = ContextVar("nous_host_call", default=None)
