"""Nous SDK; domain contracts import without initializing transports."""

from typing import TYPE_CHECKING

if TYPE_CHECKING:
    from .client import NousClient as NousClient
    from .errors import NousError as NousError
    from .files import ManagedFiles as ManagedFiles
    from .source_scope import SourceReadScope as SourceReadScope
    from .transport import ClientOptions as ClientOptions

__all__ = [
    "ClientOptions",
    "ManagedFiles",
    "NousClient",
    "NamespaceClient",
    "NousError",
    "SourceReadScope",
]


def __getattr__(name: str) -> object:
    if name == "NamespaceClient":
        from .namespace import NamespaceClient
        return NamespaceClient
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
    raise AttributeError(name)
