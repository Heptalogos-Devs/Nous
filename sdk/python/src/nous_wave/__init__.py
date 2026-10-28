"""Nous Wave SDK; domain contracts import without initializing transports."""

from typing import TYPE_CHECKING

if TYPE_CHECKING:
    from .client import NousClient as NousClient
    from .errors import NousError as NousError
    from .files import ManagedFiles as ManagedFiles
    from .managed import AppliedContext as AppliedContext
    from .managed import HostInputMeasurement as HostInputMeasurement
    from .managed import ManagedBinding as ManagedBinding
    from .managed import PreparedContext as PreparedContext
    from .source_scope import SourceReadScope as SourceReadScope
    from .transport import ClientOptions as ClientOptions

__all__ = [
    "AppliedContext",
    "ClientOptions",
    "HostInputMeasurement",
    "ManagedBinding",
    "ManagedFiles",
    "NousClient",
    "NousError",
    "PreparedContext",
    "SourceReadScope",
]


def __getattr__(name: str) -> object:
    if name == "NousClient":
        from .client import NousClient

        return NousClient
    if name == "NousError":
        from .errors import NousError

        return NousError
    if name == "ManagedFiles":
        from .files import ManagedFiles

        return ManagedFiles
    if name == "ClientOptions":
        from .transport import ClientOptions

        return ClientOptions
    if name == "SourceReadScope":
        from .source_scope import SourceReadScope

        return SourceReadScope
    if name in {"AppliedContext", "HostInputMeasurement", "ManagedBinding", "PreparedContext"}:
        from .managed import AppliedContext, HostInputMeasurement, ManagedBinding, PreparedContext

        return {
            "AppliedContext": AppliedContext,
            "HostInputMeasurement": HostInputMeasurement,
            "ManagedBinding": ManagedBinding,
            "PreparedContext": PreparedContext,
        }[name]
    raise AttributeError(name)
