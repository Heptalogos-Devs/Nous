"""Complete internal revision metadata, supplied by its domain transition owner."""

from dataclasses import dataclass

from nous_sdk.contracts import RetentionState, RevisionCause


@dataclass(frozen=True)
class RevisionState:
    cause: RevisionCause
    lifecycle: RetentionState | None = None
    deleted: bool = False
    purged: bool = False

    def __post_init__(self) -> None:
        if self.purged and (not self.deleted):
            raise ValueError("A purged revision must be deleted")
