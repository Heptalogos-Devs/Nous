"""Count and select exact source ranges against one delivery budget."""

from collections.abc import Mapping, Sequence
from functools import lru_cache
from pathlib import Path
from typing import Literal

from tokenizers import Tokenizer

from nous.common.errors import NousError

from .contracts import ContentSelection, SourceSelection
from .selections import subtract_selection, union_selections
from .text import TextView


@lru_cache(maxsize=4)
def _tokenizer(serialized: bytes) -> Tokenizer:
    value = Tokenizer.from_str(serialized.decode("utf-8"))
    value.no_truncation()
    value.no_padding()
    return value


class SelectionBudget:
    """Account for the union of ranges actually delivered from immutable bodies."""

    def __init__(
        self,
        views: Mapping[str, TextView],
        *,
        amount: int | None,
        unit: Literal["characters", "bytes", "tokens"],
        tokenizer_path: Path | None = None,
    ) -> None:
        self.views = dict(views)
        self.amount = amount
        self.unit = unit
        self.tokenizer: Tokenizer | None = None
        if unit == "tokens":
            if tokenizer_path is None:
                raise NousError(
                    "content.tokenizer_unavailable", "A tokenizer is required for token budgets"
                )
            try:
                serialized = tokenizer_path.read_bytes()
            except Exception as error:
                raise NousError(
                    "content.tokenizer_invalid", "Cannot load the configured tokenizer", cause=error
                ) from error
            try:
                self.tokenizer = _tokenizer(serialized)
            except Exception as error:
                raise NousError(
                    "content.tokenizer_invalid", "Cannot load the configured tokenizer", cause=error
                ) from error
        self.covered: tuple[ContentSelection, ...] = ()
        self.used = 0
        self._costs: dict[ContentSelection, int] = {}

    def _normalize(self, value: ContentSelection) -> ContentSelection:
        try:
            view = self.views[value.artifact.id]
        except KeyError as error:
            raise NousError("content.invalid_selection", "Text view is unavailable") from error
        start, end = view.selection_bounds(value)
        return ContentSelection(
            artifact=value.artifact, selection=SourceSelection(start_byte=start, end_byte=end)
        )

    def _cost(self, value: ContentSelection) -> int:
        if value not in self._costs:
            view = self.views[value.artifact.id]
            start, end = view.selection_bounds(value)
            data = view.raw[start - view.byte_offset : end - view.byte_offset]
            if self.unit == "bytes":
                result = len(data)
            else:
                text = data.decode("utf-8")
                result = len(text) if self.unit == "characters" else self._tokens(text)
            self._costs[value] = result
        return self._costs[value]

    def _tokens(self, text: str) -> int:
        if self.tokenizer is None:
            raise NousError(
                "content.tokenizer_unavailable", "A tokenizer is required for token budgets"
            )
        return len(self.tokenizer.encode(text, add_special_tokens=False).ids)

    def increment(
        self, values: Sequence[ContentSelection]
    ) -> tuple[tuple[ContentSelection, ...], int]:
        """Return uncovered ranges and their change to the total union cost."""
        normalized = tuple(self._normalize(value) for value in values)
        combined = union_selections((*self.covered, *normalized))
        additional: list[ContentSelection] = []
        for value in normalized:
            additional.extend(subtract_selection(value, (*self.covered, *additional)))
        return tuple(additional), sum(self._cost(value) for value in combined) - self.used

    def add(self, values: Sequence[ContentSelection]) -> tuple[ContentSelection, ...] | None:
        """Accept exact ranges if they fit; None leaves the budget unchanged."""
        additional, cost = self.increment(values)
        if self.amount is not None and self.used + cost > self.amount:
            return None
        self.covered = union_selections((*self.covered, *additional))
        self.used += cost
        return additional

    def fit_prefix(self, value: ContentSelection) -> tuple[tuple[ContentSelection, ...], bool]:
        """Return the readable prefix; charge only ranges not already covered."""
        selected = self._normalize(value)
        if self.add((selected,)) is not None:
            return (selected,), False
        view = self.views[selected.artifact.id]
        start, _ = view.selection_bounds(selected)
        best: ContentSelection | None = None
        for boundary in view.complete_prefix_ends(selected):
            prefix = ContentSelection(
                artifact=selected.artifact,
                selection=SourceSelection(start_byte=start, end_byte=boundary),
            )
            _, cost = self.increment((prefix,))
            if self.amount is None or self.used + cost <= self.amount:
                best = prefix
        if best is None:
            return (), True
        accepted = self.add((best,))
        assert accepted is not None
        return (best,), True
