"""Physical content identities contain no domain metadata."""

from pydantic import BaseModel, ConfigDict, Field


class BlobRef(BaseModel):
    model_config = ConfigDict(frozen=True, extra="forbid")
    digest: str = Field(pattern=r"^[a-f0-9]{64}$")
    byte_length: int = Field(ge=0)
