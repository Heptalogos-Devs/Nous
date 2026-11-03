"""Configuration values and service, loaded only when requested."""

from typing import TYPE_CHECKING

if TYPE_CHECKING:
    from .definitions import NousConfig as NousConfig
    from .service import ConfigSnapshot as ConfigSnapshot
    from .service import Configuration as Configuration
    from .service import FrozenValues as FrozenValues

__all__ = ["ConfigSnapshot", "Configuration", "FrozenValues", "NousConfig"]


def __getattr__(name: str) -> object:
    if name == "NousConfig":
        from .definitions import NousConfig

        return NousConfig
    if name in {"ConfigSnapshot", "Configuration", "FrozenValues"}:
        from . import service

        return getattr(service, name)
    raise AttributeError(name)
