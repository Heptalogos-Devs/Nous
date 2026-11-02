"""The application registers the contracts owned by its actual model consumers."""

from nous_sdk.contracts.cognition import FormationDraft
from nous_sdk.contracts.context import SelectionResult
from nous_sdk.contracts.corrections import CorrectionDraft, CorrectionModelInput
from nous_sdk.contracts.ego_model import SeedProposal
from nous_sdk.contracts.episodes import EpisodeSegmentationDraft, EpisodeSegmentationInput
from nous_sdk.contracts.feedback import FeedbackDecision, FeedbackInput
from nous_sdk.contracts.maintenance_tasks import (
    DreamDraft,
    JournalDraft,
    LearningPhrase,
    ReflectionDraft,
)
from nous_sdk.contracts.motivation import MotivationInterpretation
from nous_sdk.contracts.person_profiles import PersonProfileDraft, PersonProfileModelInput
from nous_sdk.contracts.social import SymbolDecision, SymbolSemanticInput
from pydantic import BaseModel, ConfigDict, JsonValue

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


class SelectionInput(RoleInput):
    goal: str
    background: str
    question: str
    passages: str
    requirements: str = ""


class StewardInput(RoleInput):
    question: str
    passages: str


class SeedInput(RoleInput):
    maintenance_run_id: str
    instruction: str
    snapshot: dict[str, JsonValue]
    previous_proposal: dict[str, JsonValue] | None = None
    validation_error: dict[str, JsonValue] | None = None


class JournalInput(RoleInput):
    instruction: str
    style: str
    materials: str
    period: str = ""


class ReflectionInput(RoleInput):
    question: str
    materials: str


class DreamInput(RoleInput):
    theme: str
    materials: str


class RerankDocument(RoleInput):
    key: str
    text: str


class RerankInput(RoleInput):
    query: str
    documents: tuple[RerankDocument, ...]


class MotivationInput(RoleInput):
    desire: str
    predicate: str
    subject_name: str
    evidence: tuple[dict[str, JsonValue], ...]


def register(registry: RoleRegistry) -> None:
    for entry in (
        RoleDefinition("formation", "text", FormationInput, FormationDraft, default=True),
        RoleDefinition(
            "formation",
            "media",
            MediaFormationInput,
            MediaFormationDraft,
            invocation_purpose="media.formation",
        ),
        RoleDefinition(
            "description",
            "media",
            MediaFormationInput,
            str,
            default=True,
            invocation_purpose="media.description",
        ),
        RoleDefinition("steward", "selection", SelectionInput, SelectionResult, default=True),
        RoleDefinition("context_compress", "text", StewardInput, str, default=True),
        RoleDefinition("history_topics", "groups", StewardInput, list[list[int]], default=True),
        RoleDefinition("work_summary", "text", StewardInput, str, default=True),
        RoleDefinition("work_questions", "questions", StewardInput, list[str], default=True),
        RoleDefinition("local_learning", "notes", StewardInput, list[LearningPhrase], default=True),
        RoleDefinition("knowledge_update", "text", StewardInput, str, default=True),
        RoleDefinition("seed", "proposal", SeedInput, SeedProposal, default=True),
        RoleDefinition("journal", "entry", JournalInput, JournalDraft, default=True),
        RoleDefinition("reflect", "local", ReflectionInput, ReflectionDraft, default=True),
        RoleDefinition("dream", "hypotheses", DreamInput, DreamDraft, default=True),
        RoleDefinition(
            "correction", "proposal", CorrectionModelInput, CorrectionDraft, default=True
        ),
        RoleDefinition("rerank", "ranking", RerankInput, Ranking, default=True),
        RoleDefinition("feedback", "classification", FeedbackInput, FeedbackDecision, default=True),
        RoleDefinition(
            "symbols", "interpretation", SymbolSemanticInput, SymbolDecision, default=True
        ),
        RoleDefinition(
            "motivation", "interpretation", MotivationInput, MotivationInterpretation, default=True
        ),
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
