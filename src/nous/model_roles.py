"""The application registers the contracts owned by its actual model consumers."""

from nous_wave.contracts.cognition import FormationDraft
from nous_wave.contracts.context import ContextSegment, ContextUsage, StewardProposal
from nous_wave.contracts.episodes import EpisodeSegmentationDraft, EpisodeSegmentationInput
from nous_wave.contracts.growth import GrowthProposal
from nous_wave.contracts.person_profiles import PersonProfileDraft, PersonProfileModelInput
from pydantic import BaseModel, ConfigDict, Field, JsonValue

from nous.imports.model_contracts import MediaFormationDraft
from nous.platform.models.roles import RoleDefinition, RoleRegistry
from nous.retrieval.readout import Ranking


class RoleInput(BaseModel):
    model_config = ConfigDict(extra="forbid")


class FormationInput(RoleInput):
    source_text: str
    source: dict[str, JsonValue]


class SavedDescription(RoleInput):
    description: str
    producer_call_id: str


class MediaFormationInput(FormationInput):
    source_context: str
    saved_description: SavedDescription | None = None


class StewardOption(RoleInput):
    id: str
    segment: ContextSegment


class StewardBudget(RoleInput):
    items: int = Field(ge=1)
    text_bytes: int = Field(ge=1)


class StewardInput(RoleInput):
    usage: ContextUsage
    required: tuple[ContextSegment, ...]
    optional: tuple[StewardOption, ...]
    budget: StewardBudget


class MaintenanceInput(RoleInput):
    maintenance_run_id: str
    instruction: str
    snapshot: dict[str, JsonValue]
    body_schema: dict[str, JsonValue]
    previous_proposal: dict[str, JsonValue] | None = None
    validation_error: dict[str, JsonValue] | None = None


class RerankDocument(RoleInput):
    key: str
    text: str


class RerankInput(RoleInput):
    query: str
    documents: tuple[RerankDocument, ...]


def append_description(prompt: dict[str, object], value: BaseModel) -> dict[str, object]:
    parsed = MediaFormationInput.model_validate(value)
    if parsed.saved_description is None:
        return prompt
    return {
        **prompt,
        "description_call_id": parsed.saved_description.producer_call_id,
        "rendered": str(prompt["rendered"])
        + "\n已保存的描述：\n"
        + parsed.saved_description.description,
    }


def register(registry: RoleRegistry) -> None:
    for entry in (
        RoleDefinition("formation", "text", FormationInput, FormationDraft, default=True),
        RoleDefinition(
            "formation",
            "media",
            MediaFormationInput,
            MediaFormationDraft,
            invocation_purpose="media.formation",
            transform_prompt=append_description,
        ),
        RoleDefinition(
            "description",
            "media",
            MediaFormationInput,
            str,
            default=True,
            invocation_purpose="media.description",
        ),
        RoleDefinition("steward", "selection", StewardInput, StewardProposal, default=True),
        RoleDefinition("maintenance", "proposal", MaintenanceInput, GrowthProposal, default=True),
        RoleDefinition("rerank", "ranking", RerankInput, Ranking, default=True),
        RoleDefinition(
            "person_profile", "text", PersonProfileModelInput, PersonProfileDraft, default=True
        ),
        RoleDefinition(
            "episode",
            "segmentation",
            EpisodeSegmentationInput,
            EpisodeSegmentationDraft,
            default=True,
        ),
    ):
        registry.register(entry)
