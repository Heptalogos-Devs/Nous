"""Shared operation identity for typed HTTP request contracts."""

from uuid import uuid4

from pydantic import BaseModel, ConfigDict, Field


class OperationIdentity(BaseModel):
    model_config = ConfigDict(extra="forbid")
    operation_id: str = Field(default_factory=lambda: str(uuid4()))