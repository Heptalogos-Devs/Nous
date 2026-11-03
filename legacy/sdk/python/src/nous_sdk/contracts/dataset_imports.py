"""Captured dataset conversion uses the existing catalog and optional native archive."""

from typing import Literal

from pydantic import Field

from .common import Operation
from .imports import ImportDocument, TextSplitOptions


class ImportDataset(Operation, TextSplitOptions):
    inputs: tuple[ImportDocument, ...] = Field(min_length=1)
    name: str = Field(default="dataset", min_length=1, max_length=256)
    llm_enabled: bool = False
    vectors: Literal["disabled", "metadata_allowed", "required"] = "metadata_allowed"
    export_bundle: bool = False
    dry_run: bool = False
    max_errors: int = Field(default=0, ge=0)
    preview_limit: int = Field(default=20, ge=1, le=1000)


class BackfillSourceTimes(Operation):
    limit: int = Field(default=100000, ge=1, le=10000000)
    dry_run: bool = False
    no_created_fallback: bool = False
