"""Shared identifiers for independently configured resources."""

from typing import Annotated

from pydantic import StringConstraints

ResourceId = Annotated[str, StringConstraints(pattern=r"^[a-z][a-z0-9]*(?:-[a-z0-9]+)*$")]
