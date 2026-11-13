"""Capture structural source context from Markdown parser locations."""

import re
from collections.abc import Sequence

from markdown_it import MarkdownIt
from markdown_it.token import Token

from .contracts import ContentValue


class StructuralContextSpan(ContentValue):
    """A source range needed when reading any overlapping part of a scope."""

    scope_start: int
    scope_end: int
    context_start: int
    context_end: int


def parse_structural_context(
    text: str,
    lines: Sequence[str],
    offsets: Sequence[int],
    tokens: Sequence[Token],
    byte_offset: int,
) -> tuple[
    tuple[StructuralContextSpan, ...], tuple[tuple[int, int], ...], tuple[tuple[int, int], ...]
]:
    """Record parser-established relationships without changing root block layout."""
    total = byte_offset + offsets[-1]
    spans: list[StructuralContextSpan] = []
    headings: list[tuple[int, int, int]] = []
    items: list[dict[str, int | None]] = []
    previous: dict[int, Token] = {}
    tables: list[tuple[int, int]] = []
    rows: list[tuple[int, int]] = []

    def line_start(line: int) -> int:
        return byte_offset + offsets[min(line, len(lines))]

    def add(scope_start: int, scope_end: int, start: int, end: int) -> None:
        if scope_start < scope_end and start < end:
            spans.append(
                StructuralContextSpan(
                    scope_start=scope_start,
                    scope_end=scope_end,
                    context_start=start,
                    context_end=end,
                )
            )

    def close_headings(level: int, end: int) -> None:
        while headings and headings[-1][0] >= level:
            _, start, heading_end = headings.pop()
            add(start, end, start, heading_end)

    for token in tokens:
        if token.type == "heading_open" and token.level == 0 and token.map is not None:
            start, end = map(line_start, token.map)
            level = int(token.tag[1:])
            close_headings(level, start)
            headings.append((level, start, end))
        if token.type == "list_item_open" and token.map is not None:
            start, end = map(line_start, token.map)
            items.append(
                {
                    "level": token.level,
                    "start": start,
                    "end": end,
                    "intro_start": None,
                    "intro_end": None,
                }
            )
        elif token.type == "paragraph_open" and token.map is not None and items:
            item = items[-1]
            level = item["level"]
            if item["intro_start"] is None and level is not None and token.level == level + 1:
                item["intro_start"], item["intro_end"] = map(line_start, token.map)
        elif token.type == "list_item_close" and items:
            item = items.pop()
            start, end = item["intro_start"], item["intro_end"]
            scope_end = item["end"]
            if start is not None and end is not None and scope_end is not None:
                add(end, scope_end, start, end)
        if token.type in {"bullet_list_open", "ordered_list_open"} and token.map is not None:
            prior = previous.get(token.level)
            if prior and prior.type == "paragraph_open" and prior.map is not None:
                start, end = prior.map
                gap = "".join(lines[end : token.map[0]])
                paragraph = "".join(lines[start:end]).rstrip()
                if not gap.strip() and paragraph.endswith((":", "：")):
                    add(
                        line_start(token.map[0]),
                        line_start(token.map[1]),
                        line_start(start),
                        line_start(end),
                    )
        if token.type == "fence" and token.map is not None:
            first, last = token.map
            scope_start, scope_end = line_start(first), line_start(last)
            add(scope_start, scope_end, line_start(first), line_start(first + 1))
            if last - first > 1:
                final = lines[last - 1].strip()
                while final.startswith(">"):
                    final = final[1:].lstrip()
                marker = re.escape(token.markup[0])
                if re.fullmatch(marker + "{" + str(len(token.markup)) + r",}[ \t]*", final):
                    add(scope_start, scope_end, line_start(last - 1), line_start(last))
        if token.map is not None and (token.type.endswith("_open") or token.type == "fence"):
            previous[token.level] = token
    close_headings(0, total)

    if "|" in text:
        table_tokens = MarkdownIt("commonmark").enable("table").parse(text)
        for index, table in enumerate(table_tokens):
            if table.type != "table_open" or table.map is None:
                continue
            close = next(
                (
                    position
                    for position in range(index + 1, len(table_tokens))
                    if table_tokens[position].type == "table_close"
                    and table_tokens[position].level == table.level
                ),
                len(table_tokens),
            )
            children = table_tokens[index + 1 : close]
            tables.append((line_start(table.map[0]), line_start(table.map[1])))
            rows.extend(
                (line_start(token.map[0]), line_start(token.map[1]))
                for token in children
                if token.type == "tr_open"
                and token.level == table.level + 2
                and token.map is not None
                and table.map[0] <= token.map[0] < table.map[1]
            )
            body = next(
                (
                    token
                    for token in children
                    if token.type == "tbody_open"
                    and token.level == table.level + 1
                    and token.map is not None
                ),
                None,
            )
            if body is not None and body.map is not None and body.map[0] < table.map[1]:
                add(
                    line_start(table.map[0]),
                    line_start(table.map[1]),
                    line_start(table.map[0]),
                    line_start(body.map[0]),
                )
    return tuple(spans), tuple(tables), tuple(rows)
