"""Complete internal revision metadata, supplied by its domain transition owner."""

from dataclasses import dataclass

from nous_wave.contracts import MemoryLifecycle, RevisionCause


@dataclass(frozen=True)
class RevisionState:
    cause: RevisionCause
    lifecycle: MemoryLifecycle | None = None
    deleted: bool = False
    purged: bool = False
