"""Project CommonMark source links to their complete visible labels."""

from collections.abc import Callable
from functools import partial
from typing import cast

from markdown_it import MarkdownIt
from markdown_it.rules_inline import autolink, link
from markdown_it.rules_inline.state_inline import StateInline
from markdown_it.token import Token

from nous.common.errors import NousError


def _label(tokens: list[Token]) -> str:
    return "".join(
        token.content
        if token.type in {"text", "text_special", "code_inline"}
        else _label(token.children or [])
        if token.type == "image"
        else "\n"
        if token.type in {"softbreak", "hardbreak"}
        else ""
        for token in tokens
    )


def _source_positions(content: str, lines: list[str], base: int) -> list[int]:
    """Map the parser's inline source through block prefixes to original character positions."""
    positions: list[int] = []
    for number, line in enumerate(content.splitlines(keepends=True)):
        raw = lines[number]
        value = line.rstrip("\r\n")
        start = raw.find(value)
        if start < 0:
            raise NousError("content.source_map_invalid", "Cannot locate the parsed inline source")
        positions.extend(range(base + start, base + start + len(value)))
        positions.extend([base + len(raw.rstrip("\r\n"))] * (len(line) - len(value)))
        base += len(raw)
    return positions


def project_citations(markdown: str) -> str:
    """Change source-link syntax only; keep unrelated Markdown and all label content."""
    if "source:" not in markdown:
        return markdown
    environment: dict[str, object] = {}
    parser = MarkdownIt("commonmark")
    tokens = parser.parse(markdown, environment)
    lines = markdown.splitlines(keepends=True)
    offsets = [0]
    for line in lines:
        offsets.append(offsets[-1] + len(line))
    replacements: list[tuple[int, int, str]] = []
    for block in tokens:
        if block.type != "inline" or block.map is None:
            continue
        children = block.children or []
        labels: list[str] = []
        for index, token in enumerate(children):
            href = token.attrGet("href") if token.type == "link_open" else None
            if isinstance(href, str) and href.startswith("source:"):
                closing = next(
                    position
                    for position in range(index + 1, len(children))
                    if children[position].type == "link_close"
                )
                labels.append(_label(children[index + 1 : closing]))
        if not labels:
            continue
        spans: list[tuple[int, int]] = []
        fragment = block.content
        inline = MarkdownIt("commonmark")

        def source_link(
            state: StateInline,
            silent: bool,
            rule: Callable[[StateInline, bool], bool] = link,
            fragment: str = fragment,
            spans: list[tuple[int, int]] = spans,
        ) -> bool:
            start, before = state.pos, len(state.tokens)
            matched = rule(state, silent)
            if matched and not silent and state.src == fragment:
                opening = next(
                    (token for token in state.tokens[before:] if token.type == "link_open"), None
                )
                href = opening.attrGet("href") if opening else None
                if isinstance(href, str) and href.startswith("source:"):
                    spans.append((start, state.pos))
            return matched

        # Rule callbacks record ranges only. Labels come from the completed parse above,
        # after CommonMark entity, escape and emphasis processing.
        inline.inline.ruler.at("link", source_link)
        inline.inline.ruler.at("autolink", partial(source_link, rule=autolink))
        inline.inline.parse(fragment, inline, environment, [])
        low, high = block.map
        positions = _source_positions(fragment, lines[low:high], offsets[low])
        for (start, end), label in zip(spans, labels, strict=True):
            replacements.append((positions[start], positions[end - 1] + 1, label))
    references = environment.get("references", {})
    if isinstance(references, dict):
        for definition in cast(dict[str, dict[str, object]], references).values():
            if str(definition.get("href", "")).startswith("source:") and definition.get("map"):
                low, high = cast(list[int], definition["map"])
                replacements.append((offsets[low], offsets[high], ""))
    for start, end, value in sorted(replacements, reverse=True):
        markdown = markdown[:start] + value + markdown[end:]
    return markdown
