"""Decode the actual Episode dashboard arguments without inventing Native state."""

from pydantic import AliasChoices, Field

from ..contracts.common import Contract


class EpisodeAdminInput(Contract):
    action: str
    operation_id: str | None = None
    query: str = ""
    episode_id: str = ""
    person_id: str = Field(default="", validation_alias=AliasChoices("person_id", "person"))
    source: str = ""
    sources: tuple[str, ...] = ()
    all: bool = False
    limit: int = Field(default=20, ge=1, le=1000)
    paragraph_limit: int = Field(default=100, ge=1, le=1000)
    time_start: str | float | None = Field(
        default=None, validation_alias=AliasChoices("time_start", "time_from")
    )
    time_end: str | float | None = Field(
        default=None, validation_alias=AliasChoices("time_end", "time_to")
    )
    max_retry: int = Field(default=3, ge=0)
    dry_run: bool = True
