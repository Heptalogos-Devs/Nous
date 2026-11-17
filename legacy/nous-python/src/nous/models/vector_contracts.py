"""Stable vector identities and the semantics of their embedding space."""

from typing import Literal

from pydantic import BaseModel, ConfigDict, Field

from nous.configuration.metadata import ConfigModel
from nous.content.contracts import ArtifactRef

from .configuration import EncodingConfig


class SpaceDefinition(ConfigModel):
    """Vector semantics independent of connection resource identifiers."""

    space_id: str
    model_name: str
    deployment_revision: str
    dimensions: int
    modalities: list[Literal["text", "image", "audio", "video"]]
    normalization: Literal["none", "l2"]
    encoding: EncodingConfig


type Purpose = Literal["query", "document"]


class VectorRef(BaseModel):
    model_config = ConfigDict(frozen=True, extra="forbid")

    space: SpaceDefinition
    purpose: Purpose
    artifact: ArtifactRef
    row_index: int = Field(ge=0, strict=True)
