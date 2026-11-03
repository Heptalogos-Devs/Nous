"""Canonical exact cognitive and media target identities."""

from typing import Annotated, Literal

from pydantic import Field

from .cognition import CognitiveRef
from .common import Contract
from .media import MediaReference


class CognitiveTarget(Contract):
    kind: Literal["cognition"] = "cognition"
    reference: CognitiveRef


class MediaTarget(Contract):
    kind: Literal["media"] = "media"
    reference: MediaReference


ContextTarget = Annotated[CognitiveTarget | MediaTarget, Field(discriminator="kind")]
