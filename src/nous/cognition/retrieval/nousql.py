"""Parse explicit NousQL controls while preserving ordinary natural language."""

from __future__ import annotations

import json
from dataclasses import dataclass

from lark import Lark, Tree
from lark.exceptions import LarkError

from nous.common.errors import NousError

from .contracts import (
    QueryAll,
    QueryAny,
    QueryAtom,
    QueryControls,
    QueryExpression,
    QueryTimeClause,
    Selector,
)

_PARSER = Lark(
    r"""
    start: SIGIL NAME "(" arguments? ")" | "#" "(" arguments? ")"
    arguments: argument ("," argument)*
    argument: atom+
    ?atom: QUOTED | TEXT | nested | ESCAPED
    nested: "(" arguments? ")"
    SIGIL: "$" | "@"
    NAME: /[a-zA-Z_][a-zA-Z_0-9]*/
    QUOTED: /"(?:\\.|[^"\\])*"/ | /'(?:\\.|[^'\\])*'/
    TEXT: /[^(),"'\\]+/
    ESCAPED: /\\./
""",
    parser="lalr",
    propagate_positions=True,
)


@dataclass(frozen=True)
class _Directive:
    sigil: str
    name: str
    arguments: tuple[str, ...]
    start: int
    end: int


def _argument(value: str) -> str:
    value = value.strip()
    if len(value) >= 2 and value[0] == value[-1] == '"':
        try:
            return str(json.loads(value))
        except ValueError as error:
            raise NousError("input.invalid_query", "Invalid quoted query argument") from error
    if len(value) >= 2 and value[0] == value[-1] == "'":
        return value[1:-1].replace("\\'", "'").replace("\\\\", "\\")
    return value


def _read_directive(source: str, start: int) -> _Directive | None:
    topic = source[start : start + 2] == "#("
    end_name = start + 1
    while (
        not topic
        and end_name < len(source)
        and (source[end_name].isascii() and (source[end_name].isalnum() or source[end_name] == "_"))
    ):
        end_name += 1
    name = "topic" if topic else source[start + 1 : end_name].lower()
    if (
        not topic
        and source[start] == "$"
        and name in {"explore", "diagnostics", "history"}
        and (end_name >= len(source) or source[end_name] != "(")
    ):
        return _Directive("$", name, (), start, end_name)
    if not topic and (end_name == start + 1 or end_name >= len(source) or source[end_name] != "("):
        return None
    depth, quote, escaped = 0, None, False
    end = end_name
    while end < len(source):
        char = source[end]
        if escaped:
            escaped = False
        elif char == "\\":
            escaped = True
        elif quote:
            if char == quote:
                quote = None
        elif char in "\"'":
            quote = char
        elif char == "(":
            depth += 1
        elif char == ")":
            depth -= 1
            if depth == 0:
                end += 1
                break
        end += 1
    if depth != 0 or quote:
        raise NousError(
            "input.invalid_query", "Unclosed query control", details={"position": start}
        )
    segment = source[start:end]
    try:
        tree = _PARSER.parse(segment)  # pyright: ignore[reportUnknownMemberType]
    except LarkError as error:
        raise NousError(
            "input.invalid_query", "Invalid query control", details={"position": start}
        ) from error
    arguments: list[str] = []
    root = next(
        (child for child in tree.children if isinstance(child, Tree) and child.data == "arguments"),
        None,
    )
    if root is not None:
        arguments = [
            _argument(segment[child.meta.start_pos : child.meta.end_pos])
            for child in root.children
            if isinstance(child, Tree)
        ]
    return _Directive(source[start], name, tuple(arguments), start, end)


def parse_nousql(source: str) -> QueryExpression:
    """Parse declared controls, leaving unmarked text and punctuation untouched."""
    if "\x00" in source:
        raise NousError("input.invalid_query", "Query contains a NUL character")
    text: list[str] = []
    directives: list[_Directive] = []
    i = 0
    while i < len(source):
        if source[i] == "\\" and i + 1 < len(source) and source[i + 1] in "$@#\\":
            text.append(source[i + 1])
            i += 2
            continue
        directive = _read_directive(source, i) if source[i] in "$@#" else None
        if directive is not None:
            if directive.sigil == "#":
                text.append(" ".join(directive.arguments))
            else:
                directives.append(directive)
            i = directive.end
        else:
            if source[i] != "#" or i + 1 == len(source) or not source[i + 1].isalnum():
                text.append(source[i])
            i += 1
    selectors: list[Selector] = []
    limits: list[int] = []
    return_types: list[str] = []
    prefer: list[str] = []
    avoid: list[str] = []
    times: list[QueryTimeClause] = []
    branches: QueryAny | QueryAll | None = None
    explore = diagnostics = False
    sort: str | None = None
    for item in directives:
        name, args = item.name, item.arguments
        if item.sigil == "@":
            kinds = {
                "ref": "ref",
                "r": "resource",
                "tag": "tag",
                "entity": "entity",
                "e": "entity",
                "schema": "schema",
                "object": "object",
            }
            if name not in kinds or not args:
                raise NousError(
                    "input.unsupported_selector",
                    "Selector is not available",
                    details={"selector": name},
                )
            selectors.extend(
                Selector.model_validate(
                    {"kind": kinds[name], "value": arg, "start": item.start, "end": item.end}
                )
                for arg in args
            )
        elif name in {"any", "all"}:
            if branches is not None or not args:
                raise NousError(
                    "input.invalid_query", "A query expression needs one nonempty combination"
                )
            children = tuple(parse_nousql(arg) for arg in args)
            branches = QueryAny(children=children) if name == "any" else QueryAll(children=children)
        elif name == "limit" and len(args) == 1 and args[0].isdecimal() and int(args[0]) > 0:
            limits.append(int(args[0]))
        elif name == "return" and args:
            return_types.extend(args)
        elif name == "time" and args:
            values: dict[str, str] = {}
            for arg in args[1:]:
                key, marker, value = arg.partition("=")
                if not marker or key.strip() not in {"from", "to", "at", "within", "domain"}:
                    raise NousError(
                        "input.invalid_query", "Invalid time condition", details={"argument": arg}
                    )
                values[key.strip()] = _argument(value)
            try:
                times.append(
                    QueryTimeClause.model_validate(
                        {
                            "axis": args[0],
                            "domain": values.get("domain", "utc"),
                            "from_value": values.get("from"),
                            "to_value": values.get("to"),
                            "at": values.get("at"),
                            "within": values.get("within"),
                        }
                    )
                )
            except ValueError as error:
                raise NousError("input.invalid_query", "Invalid time condition") from error
        elif name in {"prefer", "avoid"} and args:
            (prefer if name == "prefer" else avoid).extend(args)
        elif name == "explore" and len(args) <= 1:
            explore = not args or args[0] not in {"off", "false", "none"}
        elif name == "diagnostics" and len(args) <= 1:
            diagnostics = not args or args[0] not in {"off", "false", "none"}
        elif name == "sort" and args:
            sort = ",".join(args)
        else:
            raise NousError(
                "input.unsupported_query_control",
                "Query control is not available",
                details={"control": name},
            )
    if len(limits) > 1:
        raise NousError("input.invalid_query", "Only one limit may be supplied per query branch")
    controls = QueryControls(
        selectors=tuple(selectors),
        limit=limits[0] if limits else None,
        time_clauses=tuple(times),
        return_types=tuple(return_types),
        prefer=tuple(prefer),
        avoid=tuple(avoid),
        sort=sort,
        explore=explore,
        diagnostics=diagnostics,
    )
    natural = "".join(text).strip()
    if branches is not None:
        return branches.model_copy(update={"text": natural, "controls": controls})
    return QueryAtom(
        text=natural, selectors=tuple(selectors), controls=controls, start=0, end=len(source)
    )
