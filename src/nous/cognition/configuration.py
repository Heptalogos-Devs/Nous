"""Configuration for written records, adoption and current retrieval."""

from typing import Literal

from pydantic import Field

from nous.configuration.metadata import ConfigModel, setting


class NoteConfig(ConfigModel):
    default_recall: bool = setting(False, description="Include new notes in current retrieval.")


class JournalizeConfig(ConfigModel):
    task: str = setting("journal-draft", min_length=1, description="Writing task assignment.")
    adoption: Literal["draft", "save"] = setting(
        "draft", description="Default treatment of a completed journal draft."
    )


class AnnotationConfig(ConfigModel):
    task: str = setting(
        "annotate-concepts", min_length=1, description="Annotation task assignment."
    )
    on_save: bool = setting(False, description="Propose and adopt annotations after saving.")
    known_concept_limit: int = setting(
        12, ge=0, description="Maximum existing concept candidates supplied to annotation."
    )


class EditConfig(ConfigModel):
    task: str = setting("journal-edit", min_length=1, description="Local edit task assignment.")


class RewriteConfig(ConfigModel):
    task: str = setting(
        "journal-rewrite", min_length=1, description="Complete rewrite task assignment."
    )


class ReflectionConfig(ConfigModel):
    task: str = setting(
        "journal-reflect", min_length=1, description="New reflection task assignment."
    )


class SearchConfig(ConfigModel):
    steward: Literal["off", "optional", "required"] = setting(
        "off", description="Default reading arrangement policy for cognitive queries."
    )
    include_work: bool = setting(False, description="Include current work states in retrieval.")
    active_use_half_life: float | None = setting(
        None, gt=0, description="Decay half-life in use-record distance for active memory, or none."
    )
    passages_per_record: int = setting(
        2, ge=1, description="Maximum useful passages delivered per formal document."
    )
    space: str | None = setting(
        None, min_length=1, description="Embedding space for current retrieval."
    )
    include_review: bool = setting(False, description="Include records awaiting review by default.")
    include_exploration: bool = setting(True, description="Include exploratory records by default.")
    local_candidate_max_bytes: int = setting(
        384, ge=256, description="Target maximum size for a complete-block retrieval candidate."
    )


class ListConfig(ConfigModel):
    page_size: int = setting(20, ge=1, le=200, description="Default number of records per page.")


class CognitionConfig(ConfigModel):
    default_namespace: str = setting(
        "default",
        min_length=1,
        description="Namespace name or stable ID used when none is selected.",
    )
    notes: NoteConfig = Field(default_factory=NoteConfig)
    journalize: JournalizeConfig = Field(default_factory=JournalizeConfig)
    edit: EditConfig = Field(default_factory=EditConfig)
    rewrite: RewriteConfig = Field(default_factory=RewriteConfig)
    reflection: ReflectionConfig = Field(default_factory=ReflectionConfig)
    annotation: AnnotationConfig = Field(default_factory=AnnotationConfig)
    search: SearchConfig = Field(default_factory=SearchConfig)
    list: ListConfig = Field(default_factory=ListConfig)
