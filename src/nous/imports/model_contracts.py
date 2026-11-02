"""Media description input supplied to the registered model role."""

from pydantic import BaseModel, ConfigDict


class DescriptionInput(BaseModel):
    model_config = ConfigDict(extra="forbid")

    source_text: str = ""
    background: str = ""
