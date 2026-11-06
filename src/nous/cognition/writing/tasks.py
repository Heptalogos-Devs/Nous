"""Dedicated writing contracts and deterministic interpretation of proposed local edits."""

from __future__ import annotations

import difflib
import json
from collections.abc import Sequence
from typing import TYPE_CHECKING, ClassVar, Literal

from pydantic import BaseModel, ConfigDict, Field, JsonValue

from nous.common.errors import NousError
from nous.common.operations import add_error_context
from nous.content.contracts import ArtifactRef, ContentSelection, OutputContent
from nous.content.edits import (
    InsertAfter,
    InsertBefore,
    ReplaceQuote,
    TextChange,
    prepare_text_edit,
)
from nous.models.contracts import ReadingInput
from nous.models.tasks import TaskDefinition

if TYPE_CHECKING:
    from nous.content.artifacts import ArtifactStore

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
        super().__init__(
            "journal-draft",
            named_outputs=(
                ("output", "Markdown with known source destinations qualified when needed."),
                (
                    "model_output",
                    "Original model Markdown when source-link normalization changed it.",
                ),
            ),
        )

    def describe_contract(self) -> dict[str, JsonValue]:
        return {**super().describe_contract(), "output_semantics": {"no_material": NO_MATERIAL}}

    @staticmethod
    def body(output: OutputContent | None) -> str | None:
        return writing_body("new", output)


class JournalReflectTask(TaskDefinition):
    def __init__(self) -> None:
        super().__init__(
            "journal-reflect",
            named_outputs=(
                (
                    "output",
                    "Reflection Markdown with known source destinations qualified when needed.",
                ),
                (
                    "model_output",
                    "Original model Markdown when source-link normalization changed it.",
                ),
            ),
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
            named_outputs=(
                ("output", "Complete rewritten Markdown."),
                (
                    "model_output",
                    "Original model Markdown when source-link normalization changed it.",
                ),
                ("diff", "Program-produced exact before/after difference."),
            ),
            program_outputs=("diff",),
            required_roles=("revision_base",),
            text_input_mode="lossless",
        )

    async def outputs(
        self,
        value: JsonValue,
        reading: ReadingInput,
        artifacts: ArtifactStore,
        *,
        structured: bool,
        producer: dict[str, JsonValue] | None = None,
    ) -> dict[str, OutputContent]:
        outputs = await super().outputs(
            value, reading, artifacts, structured=structured, producer=producer
        )
        bases = {
            part.content.artifact.id: part.content.artifact
            for source in reading.sources
            if source.group and source.group.role == "revision_base"
            for part in source.group.parts
        }
        if len(bases) != 1:
            raise NousError("cognition.rewrite_base_required", "Rewrite requires one frozen body")
        previous = await artifacts.read_text(next(iter(bases.values())))
        updated = outputs["output"].data.decode("utf-8")
        outputs["diff"] = OutputContent(
            data="".join(
                difflib.unified_diff(
                    previous.splitlines(keepends=True),
                    updated.splitlines(keepends=True),
                    fromfile="before",
                    tofile="after",
                )
            ).encode(),
            media_type="text/plain",
            display_name="changes.diff",
        )
        return outputs


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
            named_outputs=(
                ("proposal", "Actual structured proposal from the model."),
                ("output", "Complete Markdown assembled from the immutable base."),
                ("edits", "Validated UTF-8 changes resolved by the common edit algorithm."),
                ("diff", "Exact before/after difference."),
            ),
            program_outputs=("output", "edits", "diff"),
            required_roles=("edit_target",),
            text_input_mode="lossless",
        )

    def validate(self, value: JsonValue, reading: ReadingInput) -> JsonValue:
        proposal = EditProposal.model_validate(value)
        allowed = {
            key
            for source in reading.sources
            if source.group and source.group.role == "edit_target"
            for key in (source.key, source.group.slot_key)
        }
        unknown = {item.target for item in proposal.edits} - allowed
        if unknown:
            raise ValueError(f"Unknown editable targets: {sorted(unknown)}")
        return proposal.model_dump(mode="json")

    async def outputs(
        self,
        value: JsonValue,
        reading: ReadingInput,
        artifacts: ArtifactStore,
        *,
        structured: bool,
        producer: dict[str, JsonValue] | None = None,
    ) -> dict[str, OutputContent]:
        proposal = EditProposal.model_validate(value)
        targets: dict[str, list[ContentSelection]] = {}
        bodies: dict[str, ArtifactRef] = {}
        for source in reading.sources:
            if source.group is None or source.group.role != "edit_target":
                continue
            for part in source.group.parts:
                for content in tuple(origin.origin for origin in part.origins) or (part.content,):
                    bodies[content.artifact.id] = content.artifact
                    for key in (source.key, source.group.slot_key):
                        if content not in targets.setdefault(key, []):
                            targets[key].append(content)
        if len(bodies) != 1:
            raise NousError("cognition.edit_base_required", "Editing requires one immutable body")
        view = await artifacts.text_view(next(iter(bodies.values())))
        selected: list[TextChange] = []
        try:
            for item in proposal.edits:
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
                        "content.edit_quote_ambiguous"
                        if possible
                        else "content.edit_quote_missing",
                        "Edit quote must match exactly once within the declared target",
                        details={"target": item.target},
                    )
                selected.append(possible[0])
            prepared = prepare_text_edit(view, selected)
        except NousError as error:
            add_error_context(
                error,
                {
                    "stage": "writing.interpret_edit",
                    "raw_output": proposal.model_dump(mode="json"),
                    "model_succeeded": True,
                },
            )
            raise
        normalized: list[JsonValue] = [
            {
                "start_byte": item.start_byte,
                "end_byte": item.end_byte,
                "before": item.expected_bytes.decode("utf-8"),
                "after": item.replacement_bytes.decode("utf-8"),
            }
            for item in prepared.changes
        ]
        return {
            "proposal": OutputContent(
                data=proposal.model_dump_json(indent=2).encode(),
                media_type="application/json",
                display_name="proposal.json",
            ),
            "output": OutputContent(data=prepared.utf8_bytes),
            "edits": OutputContent(
                data=json.dumps(normalized, ensure_ascii=False, indent=2).encode(),
                media_type="application/json",
                display_name="edits.json",
            ),
            "diff": OutputContent(
                data=prepared.diff.encode(), media_type="text/plain", display_name="changes.diff"
            ),
        }


def writing_tasks() -> Sequence[TaskDefinition]:
    return (JournalDraftTask(), JournalEditTask(), JournalRewriteTask(), JournalReflectTask())
