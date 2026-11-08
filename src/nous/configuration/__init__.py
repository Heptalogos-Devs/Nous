"""Configuration values and service, loaded only when requested."""

from typing import TYPE_CHECKING

if TYPE_CHECKING:
    from .definitions import NousConfig as NousConfig
    from .service import Configuration as Configuration
    from .values import ConfigSnapshot as ConfigSnapshot
    from .values import FrozenValues as FrozenValues

__all__ = ["ConfigSnapshot", "Configuration", "FrozenValues", "NousConfig"]


def __getattr__(name: str) -> object:
    if name == "NousConfig":
        from .definitions import NousConfig

        return NousConfig
    if name == "Configuration":
        from .service import Configuration

        return Configuration
    if name in {"ConfigSnapshot", "FrozenValues"}:
        from . import values

        return getattr(values, name)
    raise AttributeError(name)
