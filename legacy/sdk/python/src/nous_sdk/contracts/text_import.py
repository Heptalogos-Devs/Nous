"""Record admission and semantic formation retain separate completion boundaries."""

from typing import Literal

from pydantic import Field, model_validator

from .common import Contract, FormationPurpose, Operation
from .imports import (
    ImportDocument,
    ImportResult,
    ObservationFormation,
    ObservationFormSubmission,
)

TextImportCompletion = Literal["accepted", "formed"]


class TextImport(Operation):
    inputs: tuple[ImportDocument, ...] = Field(min_length=1, max_length=1000)
    formation_purpose: FormationPurpose | None = None
    completion: TextImportCompletion = "accepted"
    origin: dict[str, str] | None = None
    vectors: Literal["disabled", "metadata_allowed", "required"] | None = None

    @model_validator(mode="after")
    def completion_requires_formation(self) -> "TextImport":
        if self.completion == "formed" and self.formation_purpose is None:
            raise ValueError("formed import completion requires a formation purpose")
        return self


class TextImportSubmission(Contract):
    request_id: str
    job_id: str
    completion: TextImportCompletion


class TextImportResult(Contract):
    request_id: str
    job_id: str
    completion: TextImportCompletion
    admission: ImportResult
    formation_jobs: tuple[ObservationFormSubmission, ...]
    formations: tuple[ObservationFormation, ...] = ()
