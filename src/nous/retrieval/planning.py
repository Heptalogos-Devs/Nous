"""NousQL binding and time interpretation; pure syntax belongs to native."""

from __future__ import annotations

import json
import re
from datetime import datetime
from typing import Literal, NotRequired, TypedDict

from nous_native import parse_nousql
from nous_sdk.contracts import (
    CallContext,
    CognitiveRecord,
    DomainError,
    MediaRecord,
    QueryRequest,
    TimeFilter,
    TimeRange,
)

from .expressions import source as expression_source

AXES: dict[str, Literal["event", "observation", "formation", "validity", "recorded"]] = {
    "occurred": "event",
    "observed": "observation",
    "formed": "formation",
    "valid": "validity",
    "recorded": "recorded",
    "event": "event",
    "observation": "observation",
    "formation": "formation",
    "validity": "validity",
}


class Cue(TypedDict):
    kind: Literal["concept", "selector"]
    text: NotRequired[str]
    selector: NotRequired[str]
    locators: NotRequired[list[str]]


class Preference(TypedDict):
    negative: bool
    text: str | None
    recent: str | None


class Sort(TypedDict):
    axis: str
    direction: str
    domain: str


class Composition(TypedDict):
    operation: str
    children: list[str]


class BoundPlan(TypedDict):
    canonical: str
    selectors: list[Cue]
    preferences: list[Preference]
    sort: Sort | None
    materialize: bool
    diagnostics: str
    exclude_sources: list[str]
    asof_time: NotRequired[int]
    composition: NotRequired[Composition]
    effort: NotRequired[str]
    explore: NotRequired[bool]
    root_controls: list[str]
    no_matches: NotRequired[bool]


def coordinate(value: str | int, domain: str = "unix_ms") -> int:
    if isinstance(value, int):
        return value
    if domain != "unix_ms":
        return int(value)
    parsed = datetime.fromisoformat(str(value).replace("Z", "+00:00"))
    if parsed.tzinfo is None:
        raise DomainError(
            "timezone_required", "Absolute query timestamps require an explicit timezone"
        )
    return int(parsed.timestamp() * 1000)


def duration(value: str | int) -> int:
    match = re.fullmatch(r"(\d+(?:\.\d+)?)(ms|s|m|h|d|w)", str(value))
    if not match:
        raise DomainError(
            "invalid_duration", "Relative duration requires a number and ms/s/m/h/d/w"
        )
    return int(
        float(match[1])
        * {"ms": 1, "s": 1000, "m": 60000, "h": 3600000, "d": 86400000, "w": 604800000}[match[2]]
    )


def bind(ctx: CallContext, request: QueryRequest) -> tuple[QueryRequest, BoundPlan]:
    source = request.text
    if request.expression is not None:
        source = expression_source(request.expression, request.budget) + " " + source
    parsed = parse_nousql(source)
    syntax = json.loads(parsed.ast_json)
    update: dict[str, object] = {}
    extra: BoundPlan = {
        "canonical": parsed.canonical,
        "selectors": syntax.get("cues", []),
        "preferences": syntax.get("preferences", []),
        "sort": None,
        "materialize": False,
        "diagnostics": "full",
        "exclude_sources": [],
        "root_controls": [],
    }
    times = list(request.times)
    filters = request.filters.model_dump(mode="json")
    for directive in syntax.get("directives", []):
        name, args, named = (
            directive["name"],
            directive.get("positional", []),
            directive.get("named", {}),
        )
        validate_directive(name, args, named)
        if name in {"asof", "history", "limit", "effort", "diagnostics", "materialize"}:
            extra["root_controls"].append(name)
        if name == "return":
            kinds = tuple(value for value in args if value not in {"cognition", "media"})
            update["domains"] = tuple(
                domain
                for domain in ("cognition", "media")
                if domain in args or domain == "cognition" and kinds
            )
            update["kinds"] = (
                tuple(value for value in request.kinds if value in kinds)
                if request.kinds and kinds
                else kinds or request.kinds
            )
            if request.kinds and kinds and not update["kinds"]:
                extra["no_matches"] = True
        elif name in {"mode", "limit", "current"}:
            if name == "current":
                levels = ("none", "prefer", "required")
                update[name] = max((request.current, args[0]), key=levels.index)
            else:
                update[name] = args[0]
        elif name == "authority":
            if filters["authority"] is not None and filters["authority"] != args[0]:
                extra["no_matches"] = True
            filters["authority"] = args[0]
            update["filters"] = filters
        elif name == "history":
            update["history"] = True
        elif name == "source":
            update["source_type"] = args[0]
            if request.source_type is not None and request.source_type != args[0]:
                extra["no_matches"] = True
        elif name == "exclude":
            if "source" in named:
                extra["exclude_sources"].append(named["source"])
        elif name in {"cognitiveRole", "formationMode", "evidenceClass", "modality"}:
            field = {
                "cognitiveRole": "cognitive_roles",
                "formationMode": "formation_modes",
                "evidenceClass": "epistemic_classes",
                "modality": "modalities",
            }[name]
            prior = filters[field]
            selected = [value for value in args if not prior or value in prior]
            if not selected:
                extra["no_matches"] = True
            filters[field] = selected
            update["filters"] = filters
        elif name == "time":
            axis = AXES.get(args[0])
            if axis is None:
                raise DomainError("invalid_axis", "Unknown time axis")
            domain = named.get("domain", "unix_ms")
            start = end = None
            if "within" in named:
                if ctx.clock.domain != domain or domain != "unix_ms":
                    raise DomainError(
                        "time_domain_uncalibrated",
                        "Relative duration requires a calibrated clock in the requested domain",
                    )
                start, end = (
                    ctx.clock.coordinate - duration(named["within"]),
                    ctx.clock.coordinate + 1,
                )
            elif "at" in named:
                start = coordinate(named["at"], domain)
                end = start + 1
            else:
                start = coordinate(named["from"], domain) if "from" in named else None
                end = coordinate(named["to"], domain) if "to" in named else None
            times.append(
                TimeFilter(axis=axis, range=TimeRange(domain=domain, start=start, end=end))
            )
        elif name == "sort":
            extra["sort"] = {
                "axis": AXES[args[0]],
                "direction": args[1] if len(args) > 1 else "desc",
                "domain": named.get("domain", "unix_ms"),
            }
        elif name == "asof":
            if "ago" in named and ctx.clock.domain != "unix_ms":
                raise DomainError("time_domain_uncalibrated", "$asof(ago) requires a unix_ms clock")
            extra["asof_time"] = (
                ctx.clock.coordinate - duration(named["ago"])
                if "ago" in named
                else coordinate(args[0])
            )
        elif name in {"any", "all"}:
            extra["composition"] = {"operation": name, "children": args}
        elif name in {"effort", "diagnostics"}:
            extra[name] = args[0]
            if name == "effort":
                ceiling = {"light": 64, "normal": 256, "deep": 1000, "maximum": 10000}[args[0]]
                update["budget"] = request.budget.model_copy(
                    update={
                        "candidates": min(request.budget.candidates, ceiling)
                        if "candidates" in request.budget.model_fields_set
                        else ceiling
                    }
                )
        elif name == "materialize":
            extra["materialize"] = True
        elif name == "explore":
            extra["explore"] = True
        else:
            raise DomainError("unsupported_directive", f"Unsupported NousQL directive ${name}")
    update["times"] = tuple(times)
    if extra.get("explore") and not any(
        directive["name"] == "mode" for directive in syntax.get("directives", [])
    ):
        update["mode"] = "vcp"
    update["expression"] = None
    text = syntax.get("intent", "")
    text = " ".join(
        (text, *(cue["text"] for cue in syntax.get("cues", []) if cue["kind"] == "concept"))
    )
    update["text"] = text.strip()
    # Revalidate after language binding, so a directive cannot bypass request limits.
    bound = QueryRequest.model_validate({**request.model_dump(mode="json"), **update})
    return bound, extra


def validate_directive(name: str, args: list[object], named: dict[str, object]) -> None:
    flags = {"history", "materialize", "explore"}
    single = {"mode", "limit", "source", "effort", "diagnostics", "current", "authority"}
    choices = {
        "mode": {"lexical", "dense", "hybrid", "vcp"},
        "effort": {"light", "normal", "deep", "maximum"},
        "diagnostics": {"none", "summary", "full"},
        "current": {"none", "prefer", "required"},
        "authority": {
            "subject_cognition",
            "external_current_authority",
            "evidence",
            "interpretation",
            "resource_descriptor",
        },
    }
    valid = True
    if name in flags:
        valid = not args and not named
    elif name in single:
        valid = len(args) == 1 and not named
        if valid and name in choices:
            valid = isinstance(args[0], str) and args[0] in choices[name]
    elif name in {
        "return",
        "cognitiveRole",
        "formationMode",
        "evidenceClass",
        "modality",
        "any",
        "all",
    }:
        valid = bool(args) and not named and all(isinstance(value, str) for value in args)
    elif name == "sort":
        valid = (
            len(args) in {1, 2}
            and args[0] in AXES
            and (len(args) == 1 or args[1] in {"asc", "desc"})
            and not set(named) - {"domain"}
        )
    elif name == "exclude":
        valid = not args and set(named) == {"source"} and isinstance(named["source"], str)
    elif name == "asof":
        valid = (len(args) == 1 and not named) or (not args and set(named) == {"ago"})
    elif name == "time":
        valid = (
            len(args) == 1
            and args[0] in AXES
            and not set(named) - {"domain", "within", "at", "from", "to"}
        )
        selectors = set(named) - {"domain"}
        valid = valid and (selectors in ({"within"}, {"at"}, {"from"}, {"to"}, {"from", "to"}))
    if not valid:
        raise DomainError("invalid_query", f"Invalid arguments for ${name}")


def time_matches(record: CognitiveRecord | MediaRecord, filters: tuple[TimeFilter, ...]) -> bool:
    for condition in filters:
        value = getattr(record.times, condition.axis)
        if value is None:
            return False
        if condition.axis == "validity":
            if condition.range.domain != value.domain:
                return False
            if (
                value.end is not None
                and condition.range.start is not None
                and value.end <= condition.range.start
            ):
                return False
            if (
                condition.range.end is not None
                and value.start is not None
                and condition.range.end <= value.start
            ):
                return False
        elif not condition.range.contains(value):
            return False
    return True
