"""The application registers the contracts owned by its actual model consumers."""

from nous_sdk.contracts.context import SelectionResult
from nous_sdk.contracts.corrections import CorrectionDraft, CorrectionModelInput
from nous_sdk.contracts.ego_model import SeedProposal
from nous_sdk.contracts.episodes import EpisodeSegmentationDraft, EpisodeSegmentationInput
from nous_sdk.contracts.feedback import FeedbackDecision, FeedbackInput
from nous_sdk.contracts.formation import (
    GraphDraft,
    GraphResolutionDraft,
    GraphResolutionInput,
    NotesDraft,
)
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

from nous.imports.model_contracts import (
    DescriptionInput,
)
from nous.platform.models.roles import RoleDefinition, RoleRegistry
from nous.retrieval.readout import Ranking


class RoleInput(BaseModel):
    model_config = ConfigDict(extra="forbid")


class FormationInput(RoleInput):
    source_text: str
    background: str = ""


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
        RoleDefinition(
            "formation", "notes", FormationInput, NotesDraft, "formation.notes", default=True
        ),
        RoleDefinition("formation", "graph", FormationInput, GraphDraft, "formation.graph"),
        RoleDefinition(
            "formation",
            "entity_resolution",
            GraphResolutionInput,
            GraphResolutionDraft,
            "formation.entity-resolution",
        ),
        RoleDefinition(
            "description", "media", DescriptionInput, str, "media.describe", default=True
        ),
        RoleDefinition(
            "steward", "selection", SelectionInput, SelectionResult, "context-steward", default=True
        ),
        RoleDefinition("context_compress", "text", StewardInput, str, "context-compress", True),
        RoleDefinition(
            "history_topics", "groups", StewardInput, list[list[int]], "history-topics", True
        ),
        RoleDefinition("work_summary", "text", StewardInput, str, "work-summary", True),
        RoleDefinition(
            "work_questions", "questions", StewardInput, list[str], "work-questions", True
        ),
        RoleDefinition(
            "local_learning", "notes", StewardInput, list[LearningPhrase], "context-steward", True
        ),
        RoleDefinition("knowledge_update", "text", StewardInput, str, "context-steward", True),
        RoleDefinition("seed", "proposal", SeedInput, SeedProposal, "seed", True),
        RoleDefinition("journal", "entry", JournalInput, JournalDraft, "journal", True),
        RoleDefinition("reflect", "local", ReflectionInput, ReflectionDraft, "reflect", True),
        RoleDefinition("dream", "hypotheses", DreamInput, DreamDraft, "dream", True),
        RoleDefinition(
            "correction", "proposal", CorrectionModelInput, CorrectionDraft, "correction", True
        ),
        RoleDefinition("rerank", "ranking", RerankInput, Ranking, "query-rerank", True),
        RoleDefinition(
            "feedback", "classification", FeedbackInput, FeedbackDecision, "feedback", True
        ),
        RoleDefinition(
            "symbols", "interpretation", SymbolSemanticInput, SymbolDecision, "social-symbols", True
        ),
        RoleDefinition(
            "motivation",
            "interpretation",
            MotivationInput,
            MotivationInterpretation,
            "motivation",
            True,
        ),
        RoleDefinition(
            "person_profile",
            "text",
            PersonProfileModelInput,
            PersonProfileDraft,
            "person-profile",
            True,
        ),
        RoleDefinition(
            "episode",
            "segmentation",
            EpisodeSegmentationInput,
            EpisodeSegmentationDraft,
            "episode-segmentation",
            True,
        ),
    ):
        registry.register(entry)
