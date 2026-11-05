"""Project CommonMark source links to their complete visible labels."""

from collections.abc import Callable, Collection
from dataclasses import dataclass
from functools import partial
from typing import cast

from markdown_it import MarkdownIt
from markdown_it.common.utils import isSpace, isStrSpace
from markdown_it.rules_block import reference
from markdown_it.rules_block.state_block import StateBlock
from markdown_it.rules_inline import autolink, link
from markdown_it.rules_inline.state_inline import StateInline
from markdown_it.token import Token

from nous.common.errors import NousError


@dataclass(frozen=True)
class SourceCitation:
    key: str
    lines: tuple[int, ...]


@dataclass(frozen=True)
class ParsedCitations:
    title: str | None
    citations: tuple[SourceCitation, ...]
    spans: tuple[tuple[int, int, str], ...]


@dataclass(frozen=True)
class NormalizedCitations:
    text: str
    keys: tuple[str, ...]


@dataclass(frozen=True)
class _BareSourceLink:
    key: str
    start: int
    end: int
    replace_start: int
    replace_end: int
    replacement: str


def parse_citations(markdown: str) -> ParsedCitations:
    """Read headings and actual CommonMark source links from one parse."""
    environment: dict[str, object] = {}
    tokens = MarkdownIt("commonmark").parse(markdown, environment)
    title: str | None = None
    citations: list[SourceCitation] = []
    for index, token in enumerate(tokens):
        if title is None and token.type == "heading_open" and token.tag == "h1":
            title = tokens[index + 1].content
        for child in token.children or ():
            href = child.attrGet("href") if child.type == "link_open" else None
            if isinstance(href, str) and href.startswith("source:"):
                citations.append(SourceCitation(href[7:], tuple(token.map or ())))
    return ParsedCitations(title, tuple(citations), citation_spans(markdown, tokens, environment))


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


def _bare_source_links(markdown: str, keys: Collection[str]) -> tuple[_BareSourceLink, ...]:
    """Locate known local destinations using CommonMark's actual link rules."""
    if not keys:
        return ()
    known = set(keys)
    links: list[_BareSourceLink] = []
    definitions: list[tuple[str, _BareSourceLink]] = []
    original_positions: list[int] = []
    cursor = 0
    while cursor < len(markdown):
        original_positions.append(cursor)
        cursor += 2 if markdown[cursor : cursor + 2] == "\r\n" else 1
    original_positions.append(len(markdown))
    parser = MarkdownIt("commonmark", {"inline_definitions": True, "store_labels": True})

    def source_reference(state: StateBlock, start_line: int, end_line: int, silent: bool) -> bool:
        before = len(state.tokens)
        matched = reference(state, start_line, end_line, silent)
        if not matched or silent:
            return matched
        definition = next((token for token in state.tokens[before:] if token.type == "definition"), None)
        href = definition.meta.get("url") if definition else None
        if not isinstance(href, str) or href not in known:
            return matched
        assert definition is not None
        label = str(definition.meta["id"])
        references = cast(dict[str, dict[str, object]], state.env.get("references", {}))
        if references.get(label, {}).get("map") != [start_line, state.line]:
            return matched
        positions: list[int] = []
        fragments: list[str] = []
        for number in range(start_line, state.line):
            low = state.bMarks[number] + state.tShift[number]
            high = min(state.eMarks[number] + 1, len(state.src))
            fragments.append(state.src[low:high])
            positions.extend(range(low, high))
        fragment = "".join(fragments)
        inline_state = StateInline(fragment, parser, state.env, [])
        label_end = parser.helpers.parseLinkLabel(inline_state, 0, True)
        if label_end < 0 or fragment[label_end + 1 : label_end + 2] != ":":
            return matched
        position = label_end + 2
        while position < len(fragment) and (isSpace(ord(fragment[position])) or fragment[position] == "\n"):
            position += 1
        destination = parser.helpers.parseLinkDestination(fragment, position, len(fragment))
        if destination.ok and parser.normalizeLink(destination.str) == href:
            point = position + int(fragment[position] == "<")
            offset = original_positions[positions[point]]
            definitions.append((label, _BareSourceLink(
                href, original_positions[positions[0]],
                original_positions[positions[-1] + 1], offset, offset, "source:",
            )))
        return matched

    parser.block.ruler.at("reference", source_reference)
    environment: dict[str, object] = {}
    tokens = parser.parse(markdown, environment)
    used_references = {
        str(child.meta["label"])
        for block in tokens for child in block.children or ()
        if child.type == "link_open" and child.meta.get("label")
    }
    links.extend(target for label, target in definitions if label in used_references)
    lines = markdown.splitlines(keepends=True)
    offsets = [0]
    for line in lines:
        offsets.append(offsets[-1] + len(line))
    for block in tokens:
        if block.type != "inline" or block.map is None or not any(
            child.type == "link_open" and child.attrGet("href") in known
            for child in block.children or ()
        ):
            continue
        low, high = block.map
        fragment = block.content
        positions = _source_positions(fragment, lines[low:high], offsets[low])
        inline = MarkdownIt("commonmark")

        def source_link(
            state: StateInline,
            silent: bool,
            rule: Callable[[StateInline, bool], bool] = link,
            fragment: str = fragment,
            positions: list[int] = positions,
        ) -> bool:
            start, maximum, before = state.pos, state.posMax, len(state.tokens)
            matched = rule(state, silent)
            if not matched or silent or state.src != fragment:
                return matched
            opening = next((token for token in state.tokens[before:] if token.type == "link_open"), None)
            href = opening.attrGet("href") if opening else None
            if not isinstance(href, str) or href not in known:
                return matched
            end = state.pos
            raw_start, raw_end = positions[start], positions[end - 1] + 1
            if rule is autolink:
                label = fragment[start + 1 : end - 1]
                escaped = label.replace("\\", "\\\\").replace("[", "\\[").replace("]", "\\]")
                links.append(_BareSourceLink(
                    href, raw_start, raw_end, raw_start, raw_end,
                    f"[{escaped}](source:{href})",
                ))
                return matched
            label_end = state.md.helpers.parseLinkLabel(state, start, True)
            position = label_end + 1
            if label_end < 0 or position >= end or fragment[position] != "(":
                return matched
            position += 1
            while position < end and (isStrSpace(fragment[position]) or fragment[position] == "\n"):
                position += 1
            destination = state.md.helpers.parseLinkDestination(fragment, position, maximum)
            if destination.ok and destination.pos < end and state.md.normalizeLink(destination.str) == href:
                point = position + int(fragment[position] == "<")
                offset = positions[point]
                links.append(_BareSourceLink(href, raw_start, raw_end, offset, offset, "source:"))
            return matched

        inline.inline.ruler.at("link", source_link)
        inline.inline.ruler.at("autolink", partial(source_link, rule=autolink))
        inline.inline.parse(fragment, inline, environment, [])
    return tuple(links)


def normalize_source_links(
    markdown: str, keys: Collection[str], *, preserve: str | None = None
) -> NormalizedCitations:
    """Qualify exact known source destinations in generated text, preserving existing links."""
    inherited: set[tuple[str, str]] = {
        (link.key, preserve[link.start : link.end])
        for link in _bare_source_links(preserve, keys)
    } if preserve is not None else set()
    repairs = tuple(
        link for link in _bare_source_links(markdown, keys)
        if (link.key, markdown[link.start : link.end]) not in inherited
    )
    for repair in sorted(repairs, key=lambda item: item.replace_start, reverse=True):
        markdown = markdown[: repair.replace_start] + repair.replacement + markdown[repair.replace_end :]
    return NormalizedCitations(markdown, tuple(repair.key for repair in repairs))


def citation_spans(
    markdown: str,
    tokens: list[Token] | None = None,
    environment: dict[str, object] | None = None,
) -> tuple[tuple[int, int, str], ...]:
    """Locate source links once in original UTF-8 bytes for any number of slices."""
    if "source:" not in markdown:
        return ()
    if environment is None:
        environment = {}
    if tokens is None:
        tokens = MarkdownIt("commonmark").parse(markdown, environment)
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
    return tuple(
        (len(markdown[:start].encode("utf-8")), len(markdown[:end].encode("utf-8")), value)
        for start, end, value in sorted(replacements)
    )


def project_citation_slice(
    raw: bytes, spans: tuple[tuple[int, int, str], ...], start: int, end: int
) -> str:
    """Project complete links within a selected original-byte range."""
    selected = raw[start:end]
    for low, high, value in reversed(spans):
        if start <= low and high <= end:
            selected = selected[: low - start] + value.encode("utf-8") + selected[high - start :]
    return selected.decode("utf-8")


def project_citations(markdown: str) -> str:
    """Change source-link syntax only; keep unrelated Markdown and all label content."""
    raw = markdown.encode("utf-8")
    return project_citation_slice(raw, citation_spans(markdown), 0, len(raw))
