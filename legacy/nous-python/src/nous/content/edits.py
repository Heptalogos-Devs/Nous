"""Exact changes to an immutable UTF-8 text view."""

from __future__ import annotations

import difflib
import hashlib
from collections.abc import Sequence
from dataclasses import dataclass
from typing import Annotated, Literal

from pydantic import Field

from nous.common.errors import NousError

from .contracts import ArtifactRef, ContentSelection, ContentValue
from .text import BlockRef, TextView


class ReplaceQuote(ContentValue):
    action: Literal["replace_quote"] = "replace_quote"
    scope: BlockRef | ContentSelection
    before: str
    after: str


class InsertBefore(ContentValue):
    action: Literal["insert_before"] = "insert_before"
    scope: BlockRef | ContentSelection
    anchor: str
    text: str


class InsertAfter(ContentValue):
    action: Literal["insert_after"] = "insert_after"
    scope: BlockRef | ContentSelection
    anchor: str
    text: str


class ReplaceSelection(ContentValue):
    action: Literal["replace_selection"] = "replace_selection"
    selection: ContentSelection
    expected_text: str
    replacement: str


type TextChange = Annotated[
    ReplaceQuote | InsertBefore | InsertAfter | ReplaceSelection, Field(discriminator="action")
]


@dataclass(frozen=True)
class ResolvedEdit:
    start_byte: int
    end_byte: int
    expected_bytes: bytes
    replacement_bytes: bytes


@dataclass(frozen=True)
class PreparedTextEdit:
    body_ref: ArtifactRef
    base_digest: str
    changes: tuple[ResolvedEdit, ...]
    utf8_bytes: bytes
    diff: str


def _selection_bounds(view: TextView, selection: ContentSelection) -> tuple[int, int]:
    try:
        return view.selection_bounds(selection)
    except NousError as error:
        raise NousError("content.edit_scope_invalid", error.message, cause=error) from error


def _scope_bounds(view: TextView, scope: BlockRef | ContentSelection) -> tuple[int, int]:
    if isinstance(scope, BlockRef):
        if scope.body != view.body.artifact or scope.parser_profile != view.parser_profile:
            raise NousError("content.edit_scope_invalid", "Block belongs to a different text view")
        if view.block(scope.key).ref != scope:
            raise NousError("content.edit_scope_invalid", "Block does not match the text view")
        return _selection_bounds(view, scope.content)
    return _selection_bounds(view, scope)


def resolve_edits(view: TextView, changes: Sequence[TextChange]) -> tuple[ResolvedEdit, ...]:
    """Resolve every change against the same body, then reject ambiguous coordinates."""
    raw = view.text.encode("utf-8")
    offset = view.byte_offset
    resolved: list[ResolvedEdit] = []
    for change in changes:
        if isinstance(change, ReplaceSelection):
            start, end = _selection_bounds(view, change.selection)
            expected = change.expected_text.encode("utf-8")
            if raw[start - offset : end - offset] != expected:
                raise NousError(
                    "content.edit_mismatch", "Selected text does not match the expected text"
                )
            replacement = change.replacement.encode("utf-8")
        else:
            scope_start, scope_end = _scope_bounds(view, change.scope)
            needle = (change.before if isinstance(change, ReplaceQuote) else change.anchor).encode(
                "utf-8"
            )
            if not needle:
                raise NousError(
                    "content.edit_anchor_empty", "An edit quote or anchor must not be empty"
                )
            region = raw[scope_start - offset : scope_end - offset]
            matches: list[int] = []
            position = region.find(needle)
            while position >= 0:
                matches.append(scope_start + position)
                position = region.find(needle, position + 1)
            if len(matches) != 1:
                raise NousError(
                    "content.edit_quote_ambiguous" if matches else "content.edit_quote_missing",
                    "Edit quote or anchor must match exactly once in the selected scope",
                    details={
                        "matches": [{"start_byte": p, "end_byte": p + len(needle)} for p in matches]
                    },
                )
            start = matches[0]
            if isinstance(change, ReplaceQuote):
                end, expected, replacement = (
                    start + len(needle),
                    needle,
                    change.after.encode("utf-8"),
                )
            else:
                if isinstance(change, InsertAfter):
                    start += len(needle)
                end, expected, replacement = start, b"", change.text.encode("utf-8")
        resolved.append(ResolvedEdit(start, end, expected, replacement))

    ordered = sorted(resolved, key=lambda item: (item.start_byte, item.end_byte))
    for previous, current in zip(ordered, ordered[1:], strict=False):
        insertion = (
            previous.start_byte == previous.end_byte or current.start_byte == current.end_byte
        )
        if current.start_byte < previous.end_byte or (
            insertion and current.start_byte <= previous.end_byte
        ):
            raise NousError(
                "content.edit_overlap", "Edit ranges overlap or share an insertion boundary"
            )
    return tuple(ordered)


def apply_resolved_edits(view: TextView, changes: Sequence[ResolvedEdit]) -> PreparedTextEdit:
    raw = view.text.encode("utf-8")
    offset = view.byte_offset
    result = raw
    for change in reversed(changes):
        start, end = change.start_byte - offset, change.end_byte - offset
        if not 0 <= start <= end <= len(raw) or raw[start:end] != change.expected_bytes:
            raise NousError(
                "content.edit_mismatch", "Resolved edit no longer matches the base body"
            )
        result = result[:start] + change.replacement_bytes + result[end:]
    try:
        updated = result.decode("utf-8")
    except UnicodeDecodeError as error:
        raise NousError(
            "content.edit_scope_invalid", "Edit does not preserve valid UTF-8"
        ) from error
    diff = "".join(
        difflib.unified_diff(
            view.text.splitlines(keepends=True),
            updated.splitlines(keepends=True),
            fromfile="before",
            tofile="after",
        )
    )
    return PreparedTextEdit(
        view.body.artifact, hashlib.sha256(raw).hexdigest(), tuple(changes), result, diff
    )


def prepare_text_edit(view: TextView, changes: Sequence[TextChange]) -> PreparedTextEdit:
    return apply_resolved_edits(view, resolve_edits(view, changes))
