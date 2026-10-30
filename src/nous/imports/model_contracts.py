"""Outputs owned by the actual source formation and description consumers."""

from nous_sdk.contracts.cognition import FormationDraft, ProposedMemory
from pydantic import BaseModel, Field


class Description(BaseModel):
    description: str = Field(min_length=1)
    producer_call_id: str


class MediaFormationDraft(FormationDraft):
    memories: tuple[ProposedMemory, ...] = Field(min_length=1)
