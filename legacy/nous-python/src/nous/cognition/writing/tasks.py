"""Writing task contracts and interpretation of saved edit proposals."""

from __future__ import annotations

from collections.abc import Mapping, Sequence
from typing import ClassVar, Literal

from pydantic import BaseModel, ConfigDict, Field, JsonValue

from nous.common.errors import NousError
from nous.content.contracts import ContentSelection, OutputContent
from nous.content.edits import (
    InsertAfter,
    InsertBefore,
    PreparedTextEdit,
    ReplaceQuote,
    TextChange,
    prepare_text_edit,
)
from nous.content.text import TextView
from nous.models.contracts import ReadingInput
from nous.models.tasks import NamedOutput, TaskDefinition

NO_MATERIAL = "!NO_MATERIAL!"


def writing_body(intent: str, output: OutputContent | None) -> str | None:
    if output is None:
        raise NousError("model.invalid_output", "Writing task returned no Markdown output")
    text = output.data.decode("utf-8")
    if not text.strip():
        raise NousError("model.empty_output", "The provider returned no task output")
    if intent == "rewrite" and text.strip() == NO_MATERIAL:
        raise NousError("model.invalid_output", "Rewrite requires a complete Markdown body")
    return None if intent in {"new", "reflection"} and text.strip() == NO_MATERIAL else text


class JournalDraftTask(TaskDefinition):
    NO_MATERIAL: ClassVar[str] = NO_MATERIAL

    def __init__(self) -> None:
        super().__init__("journal-draft", named_outputs=(NamedOutput("output", "Model Markdown."),))

    def describe_contract(self) -> dict[str, JsonValue]:
        return {**super().describe_contract(), "output_semantics": {"no_material": NO_MATERIAL}}

    @staticmethod
    def body(output: OutputContent | None) -> str | None:
        return writing_body("new", output)


class JournalReflectTask(TaskDefinition):
    def __init__(self) -> None:
        super().__init__(
            "journal-reflect", named_outputs=(NamedOutput("output", "Model reflection Markdown."),)
        )

    def describe_contract(self) -> dict[str, JsonValue]:
        return {
            **super().describe_contract(),
            "output_semantics": {"no_material": NO_MATERIAL, "adoption": "new Journal"},
        }


class JournalRewriteTask(TaskDefinition):
    def __init__(self) -> None:
        super().__init__(
            "journal-rewrite",
            named_outputs=(NamedOutput("output", "Complete model rewrite."),),
            required_roles=("revision_base",),
            text_input_mode="lossless",
        )


class ProposedEdit(BaseModel):
    model_config = ConfigDict(extra="forbid")
    target: str = Field(min_length=1)
    action: Literal["replace", "insert_before", "insert_after"]
    before: str = Field(min_length=1)
    after: str


class EditProposal(BaseModel):
    model_config = ConfigDict(extra="forbid")
    edits: list[ProposedEdit]


class JournalEditTask(TaskDefinition):
    def __init__(self) -> None:
        super().__init__(
            "journal-edit",
            EditProposal,
            named_outputs=(NamedOutput("output", "Structured edit proposal from the model."),),
            required_roles=("edit_target",),
            text_input_mode="lossless",
        )

    def validate(self, value: JsonValue, reading: ReadingInput) -> JsonValue:
        return EditProposal.model_validate(value).model_dump(mode="json")


def compose_edit(
    proposal: EditProposal,
    view: TextView,
    targets: Mapping[str, Sequence[ContentSelection]],
) -> PreparedTextEdit:
    """Apply a saved proposal only within its selected ranges of the exact base."""
    selected: list[TextChange] = []
    for item in proposal.edits:
        if item.target not in targets:
            raise NousError(
                "cognition.edit_target_unknown",
                "Edit proposal refers to an unknown target",
                details={"target": item.target},
            )
        possible: list[TextChange] = []
        for scope in targets.get(item.target, ()):
            change: TextChange = (
                ReplaceQuote(scope=scope, before=item.before, after=item.after)
                if item.action == "replace"
                else InsertBefore(scope=scope, anchor=item.before, text=item.after)
                if item.action == "insert_before"
                else InsertAfter(scope=scope, anchor=item.before, text=item.after)
            )
            try:
                prepare_text_edit(view, (change,))
            except NousError as error:
                if error.code == "content.edit_quote_missing":
                    continue
                raise
            possible.append(change)
        if len(possible) != 1:
            raise NousError(
                "content.edit_quote_ambiguous" if possible else "content.edit_quote_missing",
                "Edit quote must match exactly once within the declared target",
                details={"target": item.target},
            )
        selected.append(possible[0])
    return prepare_text_edit(view, selected)


def writing_tasks() -> Sequence[TaskDefinition]:
    return (JournalDraftTask(), JournalEditTask(), JournalRewriteTask(), JournalReflectTask())
