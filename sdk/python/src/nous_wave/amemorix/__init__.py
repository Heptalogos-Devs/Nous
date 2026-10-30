"""Host adapter exports stay lazy so shared operation contracts remain lightweight."""

from typing import TYPE_CHECKING

if TYPE_CHECKING:
    from .client import AMemorixAdapter as AMemorixAdapter

__all__ = ["AMemorixAdapter"]


def __getattr__(name: str) -> object:
    if name == "AMemorixAdapter":
        from .client import AMemorixAdapter

        return AMemorixAdapter
    raise AttributeError(name)
