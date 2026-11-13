"""Compile a typed Wave operator graph for one snapshot-bound execution."""

from __future__ import annotations

import hashlib
import json
from collections.abc import Callable, Mapping
from dataclasses import dataclass
from graphlib import CycleError, TopologicalSorter
from typing import TYPE_CHECKING, Literal, cast

from pydantic import BaseModel, ConfigDict, Field, JsonValue, TypeAdapter, model_validator

from nous.common.errors import NousError
from nous.configuration.metadata import ConfigSequence
from nous.models.evaluation_contracts import EvaluationScale

from .contracts import CandidateSet, DeliveryBudget, ProgramRunFacts, ProgramRunNode
from .serialization import content_signature

if TYPE_CHECKING:
    from .configuration import WaveConfig


class PortRef(BaseModel):
    model_config = ConfigDict(extra="forbid", frozen=True)
    node: str = Field(min_length=1)
    port: str = Field(min_length=1)


class ProgramNode(BaseModel):
    model_config = ConfigDict(extra="forbid", frozen=True)
    id: str = Field(min_length=1)
    operator: str = Field(min_length=1)
    inputs: dict[str, PortRef | ConfigSequence[PortRef]] = Field(default_factory=dict)
    parameters: dict[str, JsonValue] = Field(default_factory=dict)


class WaveProgram(BaseModel):
    model_config = ConfigDict(extra="forbid", frozen=True)
    nodes: ConfigSequence[ProgramNode]
    outputs: dict[str, PortRef]


class EvaluationParameters(BaseModel):
    model_config = ConfigDict(extra="forbid", frozen=True)
    method: Literal["rank", "evaluate"] = "rank"
    policy: Literal["optional", "required"] = "optional"
    numeric_order: Literal["higher", "lower"] = "higher"
    category_order: ConfigSequence[str] = ()
    task: str | None = None
    purpose: str = "Select passages relevant to the question."
    rubric: tuple[str, ...] = ()
    scale: EvaluationScale = Field(
        default_factory=lambda: EvaluationScale(name="relative_relevance", minimum=0.0, maximum=1.0)
    )

    @model_validator(mode="after")
    def category_priority(self) -> EvaluationParameters:
        if self.category_order and (
            len(self.category_order) != len(self.scale.labels)
            or set(self.category_order) != set(self.scale.labels)
        ):
            raise ValueError("category_order must contain every declared label exactly once")
        return self


@dataclass(frozen=True)
class Operator:
    inputs: dict[str, str]
    outputs: dict[str, str]
    requirements: frozenset[str] = frozenset()
    pure: bool = True


# A [] suffix marks a multi-input port. Numerical parameter schemas are supplied
# by Native together with the implementation, not duplicated in this catalog.
CATALOG = {
    "vcp.observe": Operator(
        {"signal": "signal", "snapshot": "snapshot"},
        {
            "observation": "observation",
            "enhanced": "vector",
            "local": "vector",
            "transfer": "vector",
        },
        frozenset({"vectors", "vcp"}),
    ),
    "ann.search": Operator(
        {"vector": "vector", "snapshot": "snapshot", "allowed": "scope"},
        {"candidates": "candidates"},
        frozenset({"vectors"}),
    ),
    "lexical.search": Operator(
        {"text": "lexical", "snapshot": "snapshot", "allowed": "scope"},
        {"candidates": "candidates"},
        frozenset({"text"}),
    ),
    "concept.direct": Operator(
        {"observation": "observation", "snapshot": "snapshot", "allowed": "scope"},
        {"candidates": "candidates"},
        frozenset({"vcp"}),
    ),
    "candidates.union": Operator({"sets": "candidates[]"}, {"candidates": "candidates"}),
    "vcp.calibrate": Operator(
        {"observation": "observation", "candidates": "candidates"},
        {"pool": "calibrated"},
        frozenset({"vcp"}),
    ),
    "river.readout": Operator(
        {"observation": "observation", "pool": "calibrated"},
        {"ranked": "ranked"},
        frozenset({"vcp"}),
    ),
    "dtsc.readout": Operator(
        {"observation": "observation", "candidates": "candidates"},
        {"ranked": "ranked"},
        frozenset({"vcp"}),
    ),
    "vcp.think": Operator(
        {"observation": "observation", "stages": "stages", "ranked": "ranked"},
        {"routes": "links"},
        frozenset({"vcp"}),
    ),
    "rank.from_lane": Operator({"candidates": "candidates"}, {"ranked": "ranked"}),
    "rank.cosine": Operator(
        {"candidates": "candidates", "vector": "vector", "snapshot": "snapshot"},
        {"ranked": "ranked"},
        frozenset({"vectors"}),
    ),
    "rank.fuse": Operator({"rankings": "ranked[]"}, {"ranked": "ranked"}),
    "context.recall": Operator(
        {"signals": "context_lexical", "snapshot": "snapshot", "allowed": "scope"},
        {"candidates": "candidates"},
        frozenset({"text"}),
    ),
    "rank.context": Operator(
        {"ranked": "ranked", "candidates": "candidates"},
        {"ranked": "ranked"},
    ),
    "passages.locate": Operator(
        {"ranked": "ranked", "intent": "intent", "snapshot": "snapshot"}, {"passages": "passages"}
    ),
    "passages.evaluate": Operator(
        {"passages": "passages", "intent": "intent"}, {"passages": "passages"}, pure=False
    ),
    "passages.select": Operator(
        {"passages": "passages", "delivery": "delivery"}, {"selected": "selection"}
    ),
    "delivery.read": Operator(
        {"selected": "selection", "materials": "materials"}, {"evidence": "evidence"}, pure=False
    ),
}
BUILTINS = {
    "query": {
        "intent": "intent",
        "raw_vector": "vector",
        "contextual_signal": "signal",
        "lexical": "lexical",
        "context_lexical": "context_lexical",
        "stages": "stages",
    },
    "snapshot": {"handle": "snapshot"},
    "scope": {"allowed": "scope", "materials": "materials", "candidates": "candidates"},
    "delivery": {"request": "delivery"},
}


@dataclass(frozen=True)
class CompiledProgram:
    name: str
    definition: WaveProgram
    nodes: tuple[ProgramNode, ...]
    outputs: dict[str, PortRef]
    requirements: frozenset[str]
    signature: str


def _error(message: str, **details: object) -> NousError:
    return NousError("wave.invalid_program", message, details=details)


def compile_program(name: str, definition: WaveProgram) -> CompiledProgram:
    nodes = {node.id: node for node in definition.nodes}
    if len(nodes) != len(definition.nodes):
        raise _error("Program node IDs must be unique", program=name)
    if set(nodes) & set(BUILTINS):
        raise _error("Program node ID is reserved", nodes=sorted(set(nodes) & set(BUILTINS)))
    graph: dict[str, set[str]] = {}
    requirements: set[str] = set()
    from jsonschema import Draft202012Validator
    from jsonschema import ValidationError as SchemaValidationError

    from .configuration import native_profiles

    _, native_schemas = native_profiles()
    operator_schemas = TypeAdapter(dict[str, dict[str, JsonValue]]).validate_python(
        native_schemas.get("operators", {})
    )
    for node in definition.nodes:
        operator = CATALOG.get(node.operator)
        if operator is None:
            raise _error("Unknown operator", node=node.id, operator=node.operator)
        if set(node.inputs) != set(operator.inputs):
            raise _error(
                "Operator inputs do not match its ports",
                node=node.id,
                expected=sorted(operator.inputs),
                actual=sorted(node.inputs),
            )
        if node.operator == "passages.evaluate":
            try:
                EvaluationParameters.model_validate(node.parameters)
            except ValueError as error:
                raise _error(
                    "Invalid evaluation parameters", node=node.id, reason=str(error)
                ) from error
        elif node.operator in operator_schemas:
            schema = operator_schemas[node.operator]
            validate = cast(Callable[[object], None], Draft202012Validator(schema).validate)
            try:
                validate(node.parameters)
            except SchemaValidationError as error:
                raise _error(
                    "Invalid operator parameters", node=node.id, reason=str(error)
                ) from error
        elif node.parameters:
            raise _error(
                "Native operator parameter schema unavailable", node=node.id, operator=node.operator
            )
        dependencies: set[str] = set()
        for port, required in operator.inputs.items():
            value = node.inputs[port]
            multiple = required.endswith("[]")
            if multiple != isinstance(value, tuple):
                raise _error("Operator input multiplicity differs", node=node.id, port=port)
            refs = value if isinstance(value, tuple) else (value,)
            if multiple and not refs:
                raise _error("Operator input cannot be empty", node=node.id, port=port)
            for ref in refs:
                if ref.node in BUILTINS:
                    available = BUILTINS[ref.node]
                elif ref.node in nodes:
                    parent = CATALOG.get(nodes[ref.node].operator)
                    if parent is None:
                        raise _error("Unknown upstream operator", node=ref.node)
                    available = parent.outputs
                    dependencies.add(ref.node)
                else:
                    raise _error(
                        "Unknown upstream node", node=node.id, port=port, upstream=ref.node
                    )
                actual = available.get(ref.port)
                if actual != required.removesuffix("[]"):
                    raise _error(
                        "Operator port type differs",
                        node=node.id,
                        port=port,
                        expected=required.removesuffix("[]"),
                        actual=actual,
                        upstream=ref.model_dump(),
                    )
        graph[node.id] = dependencies
    for name_, ref in definition.outputs.items():
        if ref.node not in nodes or ref.port not in CATALOG[nodes[ref.node].operator].outputs:
            raise _error("Unknown program output", output=name_, node=ref.node, port=ref.port)
    if (
        not {"evidence"}.issubset(definition.outputs)
        or set(definition.outputs) - {"evidence", "thinking"}
        or CATALOG[nodes[definition.outputs["evidence"].node].operator].outputs[
            definition.outputs["evidence"].port
        ]
        != "evidence"
    ):
        raise _error("Program must expose one evidence output", program=name)
    if "thinking" in definition.outputs:
        thinking = definition.outputs["thinking"]
        if CATALOG[nodes[thinking.node].operator].outputs[thinking.port] != "links":
            raise _error("Thinking output must contain routes", program=name)
    try:
        order = tuple(TopologicalSorter(graph).static_order())
    except CycleError as error:
        raise _error("Program contains a cycle", nodes=error.args[1]) from error
    needed = {ref.node for ref in definition.outputs.values()}
    pending = list(needed)
    while pending:
        for dependency in graph[pending.pop()]:
            if dependency not in needed:
                needed.add(dependency)
                pending.append(dependency)
    order = tuple(node_id for node_id in order if node_id in needed)
    # Preserve node identities and eliminate only truly identical deterministic work.
    aliases: dict[str, str] = {}
    seen: dict[str, str] = {}
    compiled: list[ProgramNode] = []
    for node_id in order:
        node = nodes[node_id]
        requirements.update(CATALOG[node.operator].requirements)
        if node.operator == "passages.evaluate":
            parameters = TypeAdapter(dict[str, JsonValue]).validate_python(
                EvaluationParameters.model_validate(node.parameters).model_dump(mode="json")
            )
        else:
            properties = TypeAdapter(dict[str, dict[str, JsonValue]]).validate_python(
                operator_schemas.get(node.operator, {}).get("properties", {})
            )
            parameters: dict[str, JsonValue] = {
                key: value["default"] for key, value in properties.items() if "default" in value
            }
            parameters.update(node.parameters)
        if node.operator == "passages.locate":
            signals = parameters.get("signals")
            if isinstance(signals, list) and "semantic" in signals:
                requirements.add("vectors")
        inputs = {
            key: tuple(
                ref.model_copy(update={"node": aliases.get(ref.node, ref.node)}) for ref in value
            )
            if isinstance(value, tuple)
            else value.model_copy(update={"node": aliases.get(value.node, value.node)})
            for key, value in node.inputs.items()
        }
        node = node.model_copy(update={"inputs": inputs, "parameters": parameters})
        identity = json.dumps(
            {
                "operator": node.operator,
                "inputs": {
                    k: [r.model_dump() for r in v] if isinstance(v, tuple) else v.model_dump()
                    for k, v in inputs.items()
                },
                "parameters": node.parameters,
            },
            sort_keys=True,
            separators=(",", ":"),
        )
        if CATALOG[node.operator].pure and identity in seen:
            aliases[node.id] = seen[identity]
        else:
            seen[identity] = node.id
            aliases[node.id] = node.id
            compiled.append(node)
    outputs = {
        name_: ref.model_copy(update={"node": aliases.get(ref.node, ref.node)})
        for name_, ref in definition.outputs.items()
    }
    canonical = definition.model_dump_json()
    return CompiledProgram(
        name,
        definition,
        tuple(compiled),
        outputs,
        frozenset(requirements),
        hashlib.sha256(canonical.encode()).hexdigest(),
    )


def resolve_program(config: WaveConfig, name: str | None = None) -> CompiledProgram:
    selected = name or config.default_program
    definition = config.programs.get(selected)
    if definition is None:
        raise NousError(
            "wave.program_not_found",
            "Wave program is not configured",
            details={"program": selected},
        )
    return compile_program(selected, definition)


def program_run_facts(
    program: CompiledProgram,
    *,
    requested_program: str | None,
    selection: Literal["explicit", "default", "fallback", "exploration"],
    options: Mapping[str, object],
    context_options: Mapping[str, object],
    evaluation_options: Mapping[str, object],
    evaluation_tasks: Mapping[str, object],
    budget: DeliveryBudget | None,
    space_id: str | None,
    space_signature: str | None,
    space_definition: Mapping[str, object] | None,
    candidate_set: CandidateSet | None,
    expression_kind: Literal["term", "any", "all"] | None,
    expression_terms: int,
) -> ProgramRunFacts:
    """Freeze the effective graph and its actual bounded numerical settings."""
    operators = {node.operator for node in program.nodes}
    option_names = {
        "top_k",
        "grouping",
        "passages_per_group",
        "candidates_per_group",
        "group_scan_limit",
        "preference_weight",
        "dedup_threshold",
        "include_counterevidence",
        "explore",
    }
    if (
        operators & {"context.recall", "candidates.union", "rank.context"}
        or expression_kind is not None
        or any(
            node.operator == "rank.fuse" and node.parameters.get("k") is None
            for node in program.nodes
        )
    ):
        option_names.add("rrf_k")
    if "vcp.observe" in operators:
        option_names.add("pipeline")
    if any(
        node.operator == "ann.search" and node.parameters.get("search_effort") is None
        for node in program.nodes
    ):
        option_names.add("ann_search_effort")
    if "dtsc.readout" in operators:
        option_names.add("dtsc")
    if "vcp.think" in operators:
        option_names.add("thinking_min_closure")
    selected_options = {key: value for key, value in options.items() if key in option_names}
    selected_options["candidate_set"] = (
        {"count": len(candidate_set.entries), "rank_scope": candidate_set.rank_scope}
        if candidate_set is not None
        else None
    )
    selected_options["expression"] = (
        {"kind": expression_kind, "terms": expression_terms} if expression_kind else None
    )
    if operators & {"context.recall", "passages.locate", "passages.evaluate"}:
        selected_options["context"] = dict(context_options)
    if "passages.evaluate" in operators:
        selected_options["evaluation"] = {
            "limits": dict(evaluation_options),
            "tasks": dict(evaluation_tasks),
        }
    readout = TypeAdapter(dict[str, JsonValue]).validate_python(options.get("readout", {}))
    lane_limits = {
        "raw_vector": "queryK",
        "enhanced": "denoisedK",
        "local": "localFieldK",
        "transfer": "transferFieldK",
    }
    readout_names = {"maxUnionCandidates"}
    if operators & {"lexical.search", "context.recall"}:
        readout_names.add("bm25K")
    if "concept.direct" in operators:
        readout_names.add("anchorK")
    for node in program.nodes:
        if node.operator == "ann.search":
            vector = node.inputs["vector"]
            assert isinstance(vector, PortRef)
            lookup = lane_limits.get(vector.port)
            if lookup is not None:
                readout_names.add(lookup)
    if operators & {"vcp.calibrate", "river.readout"}:
        selected_options["readout"] = readout
    else:
        selected_options["readout"] = {key: readout[key] for key in readout_names if key in readout}
    nodes: list[ProgramRunNode] = []
    for node in program.nodes:
        parameters: dict[str, object] = dict(node.parameters)
        inputs = dict(node.inputs)
        if node.operator == "rank.fuse":
            if parameters.get("k") is None:
                parameters["k"] = options["rrf_k"]
            references = node.inputs["rankings"]
            assert isinstance(references, tuple)
            configured = TypeAdapter(list[float]).validate_python(parameters.get("weights") or [])
            if configured and len(configured) != len(references):
                raise _error("Fusion weights must match ranking inputs", node=node.id)
            weights = configured or [1.0] * len(references)
            unique: dict[PortRef, float] = {}
            for reference, weight in zip(references, weights, strict=True):
                unique.setdefault(reference, weight)
            inputs["rankings"] = tuple(unique)
            parameters["weights"] = list(unique.values())
        elif node.operator == "passages.select":
            parameters["budget"] = budget.model_dump(mode="json") if budget else None
        elif node.operator == "passages.evaluate":
            method = parameters.get("method")
            if parameters.get("task") is None:
                parameters["task"] = "rank-passages" if method == "rank" else "evaluate-passages"
            if method == "rank":
                for key in ("numeric_order", "category_order", "purpose", "rubric", "scale"):
                    parameters.pop(key, None)
            else:
                scale = TypeAdapter(dict[str, JsonValue]).validate_python(parameters.get("scale"))
                parameters.pop("numeric_order" if scale.get("labels") else "category_order", None)
        elif node.operator in {
            "ann.search",
            "lexical.search",
            "concept.direct",
            "candidates.union",
        }:
            key = "candidate_limit"
            if parameters.get(key) is None:
                if node.operator == "ann.search":
                    vector = node.inputs["vector"]
                    assert isinstance(vector, PortRef)
                    lookup = lane_limits.get(vector.port)
                else:
                    lookup = {
                        "lexical.search": "bm25K",
                        "concept.direct": "anchorK",
                        "candidates.union": "maxUnionCandidates",
                    }[node.operator]
                if lookup is not None:
                    parameters[key] = readout.get(lookup)
            if parameters.get("candidates_per_group") is None:
                parameters["candidates_per_group"] = options["candidates_per_group"]
            if node.operator == "ann.search" and parameters.get("search_effort") is None:
                parameters["search_effort"] = options["ann_search_effort"]
        nodes.append(
            ProgramRunNode(
                id=node.id,
                operator=node.operator,
                inputs=TypeAdapter(dict[str, JsonValue]).validate_python(
                    {
                        key: [ref.model_dump(mode="json") for ref in value]
                        if isinstance(value, tuple)
                        else value.model_dump(mode="json")
                        for key, value in inputs.items()
                    }
                ),
                parameters=TypeAdapter(dict[str, JsonValue]).validate_python(parameters),
            )
        )
    facts = {
        "name": program.name,
        "definition_signature": program.signature,
        "requested_program": requested_program,
        "selection": selection,
        "requirements": sorted(program.requirements),
        "nodes": [node.model_dump(mode="json") for node in nodes],
        "outputs": {key: value.model_dump(mode="json") for key, value in program.outputs.items()},
        "options": selected_options,
        "budget": budget.model_dump(mode="json") if budget else None,
        "space_id": space_id,
        "space_signature": space_signature,
        "space_definition": space_definition,
    }
    return ProgramRunFacts.model_validate({**facts, "signature": content_signature(facts)})


def native_segment(program: CompiledProgram) -> tuple[str, dict[str, object]]:
    """Extract the snapshot-bound graph through its localization result."""
    locators = [node for node in program.nodes if node.operator == "passages.locate"]
    if len(locators) != 1:
        raise _error("A query must have one localization node", program=program.name)
    locator = locators[0]
    by_id = {node.id: node for node in program.nodes}
    needed = {locator.id}
    if "thinking" in program.outputs:
        needed.add(program.outputs["thinking"].node)
    pending = list(needed)
    while pending:
        for value in by_id[pending.pop()].inputs.values():
            for ref in value if isinstance(value, tuple) else (value,):
                if ref.node in by_id and ref.node not in needed:
                    needed.add(ref.node)
                    pending.append(ref.node)
    nodes = [node for node in program.nodes if node.id in needed]
    if any(
        node.operator in {"passages.evaluate", "passages.select", "delivery.read"} for node in nodes
    ):
        raise _error("Localization cannot depend on a delivery stage", node=locator.id)
    outputs: dict[str, object] = {"evidence": {"node": locator.id, "port": "passages"}}
    if "thinking" in program.outputs:
        outputs["thinking"] = program.outputs["thinking"].model_dump(mode="json")
    return locator.id, {
        "nodes": [node.model_dump(mode="json") for node in nodes],
        "outputs": outputs,
    }


def default_programs() -> dict[str, WaveProgram]:
    def ref(node: str, port: str) -> PortRef:
        return PortRef(node=node, port=port)

    def node(
        id: str,
        operator: str,
        inputs: dict[str, PortRef | tuple[PortRef, ...]],
        **parameters: JsonValue,
    ) -> ProgramNode:
        return ProgramNode(id=id, operator=operator, inputs=inputs, parameters=parameters)

    snap, allowed = ref("snapshot", "handle"), ref("scope", "allowed")
    lexical = node(
        "lexical",
        "lexical.search",
        {"text": ref("query", "lexical"), "snapshot": snap, "allowed": allowed},
    )
    raw = node(
        "semantic",
        "ann.search",
        {"vector": ref("query", "raw_vector"), "snapshot": snap, "allowed": allowed},
    )
    observe = node(
        "observe", "vcp.observe", {"signal": ref("query", "contextual_signal"), "snapshot": snap}
    )
    enhanced = node(
        "enhanced",
        "ann.search",
        {"vector": ref("observe", "enhanced"), "snapshot": snap, "allowed": allowed},
    )
    local = node(
        "local",
        "ann.search",
        {"vector": ref("observe", "local"), "snapshot": snap, "allowed": allowed},
    )
    transfer = node(
        "transfer",
        "ann.search",
        {"vector": ref("observe", "transfer"), "snapshot": snap, "allowed": allowed},
    )
    direct = node(
        "direct",
        "concept.direct",
        {"observation": ref("observe", "observation"), "snapshot": snap, "allowed": allowed},
    )
    context_recall = node(
        "context_recall",
        "context.recall",
        {"signals": ref("query", "context_lexical"), "snapshot": snap, "allowed": allowed},
    )
    union = node(
        "union",
        "candidates.union",
        {
            "sets": tuple(
                ref(n, "candidates")
                for n in (
                    "semantic",
                    "lexical",
                    "enhanced",
                    "local",
                    "transfer",
                    "direct",
                    "context_recall",
                )
            )
        },
    )
    calibrate = node(
        "calibrate",
        "vcp.calibrate",
        {"observation": ref("observe", "observation"), "candidates": ref("union", "candidates")},
    )

    def finish(
        stages: list[ProgramNode],
        rank: ProgramNode,
        *,
        text_only: bool = False,
        context_in_union: bool = False,
        context_enabled: bool = True,
    ) -> WaveProgram:
        context_nodes = (
            []
            if context_in_union or not context_enabled
            else [
                context_recall,
                node(
                    "context",
                    "rank.context",
                    {
                        "ranked": ref(rank.id, "ranked"),
                        "candidates": ref("context_recall", "candidates"),
                    },
                ),
            ]
        )
        ranked = (
            ref(rank.id, "ranked")
            if context_in_union or not context_enabled
            else ref("context", "ranked")
        )
        return WaveProgram(
            nodes=tuple(
                stages
                + [
                    rank,
                    *context_nodes,
                    node(
                        "locate",
                        "passages.locate",
                        {
                            "ranked": ranked,
                            "intent": ref("query", "intent"),
                            "snapshot": snap,
                        },
                        **({"signals": cast(list[JsonValue], ["lexical"])} if text_only else {}),
                    ),
                    node(
                        "select",
                        "passages.select",
                        {
                            "passages": ref("locate", "passages"),
                            "delivery": ref("delivery", "request"),
                        },
                    ),
                    node(
                        "deliver",
                        "delivery.read",
                        {
                            "selected": ref("select", "selected"),
                            "materials": ref("scope", "materials"),
                        },
                    ),
                ]
            ),
            outputs={"evidence": ref("deliver", "evidence")},
        )

    text = finish(
        [lexical],
        node("rank", "rank.from_lane", {"candidates": ref("lexical", "candidates")}, lane="bm25"),
        text_only=True,
    )
    lexical_only = finish(
        [lexical],
        node("rank", "rank.from_lane", {"candidates": ref("lexical", "candidates")}, lane="bm25"),
        text_only=True,
        context_enabled=False,
    )
    dense = finish(
        [raw],
        node(
            "rank",
            "rank.from_lane",
            {"candidates": ref("semantic", "candidates")},
            lane="query_knn",
        ),
    )
    tagmemo = finish(
        [observe, enhanced],
        node(
            "rank",
            "rank.from_lane",
            {"candidates": ref("enhanced", "candidates")},
            lane="denoised_field_knn",
        ),
    )
    shared = [observe, raw, lexical, enhanced, local, transfer, direct, context_recall, union]
    river = finish(
        shared + [calibrate],
        node(
            "rank",
            "river.readout",
            {"observation": ref("observe", "observation"), "pool": ref("calibrate", "pool")},
        ),
        context_in_union=True,
    )
    dtsc = finish(
        shared,
        node(
            "rank",
            "dtsc.readout",
            {
                "observation": ref("observe", "observation"),
                "candidates": ref("union", "candidates"),
            },
        ),
        context_in_union=True,
    )
    hybrid = finish(
        [
            raw,
            lexical,
            node(
                "dense_rank",
                "rank.from_lane",
                {"candidates": ref("semantic", "candidates")},
                lane="query_knn",
            ),
            node(
                "text_rank",
                "rank.from_lane",
                {"candidates": ref("lexical", "candidates")},
                lane="bm25",
            ),
        ],
        node(
            "rank",
            "rank.fuse",
            {"rankings": (ref("dense_rank", "ranked"), ref("text_rank", "ranked"))},
        ),
    )
    research = finish(
        shared
        + [
            calibrate,
            node(
                "river_rank",
                "river.readout",
                {"observation": ref("observe", "observation"), "pool": ref("calibrate", "pool")},
            ),
            node(
                "cosine_rank",
                "rank.cosine",
                {
                    "candidates": ref("union", "candidates"),
                    "vector": ref("query", "raw_vector"),
                    "snapshot": snap,
                },
            ),
        ],
        node(
            "rank",
            "rank.fuse",
            {"rankings": (ref("river_rank", "ranked"), ref("cosine_rank", "ranked"))},
        ),
        context_in_union=True,
    )
    fixed = finish(
        [],
        node("rank", "rank.from_lane", {"candidates": ref("scope", "candidates")}, lane="provided"),
    )

    def thinking(program: WaveProgram) -> WaveProgram:
        locator = next(item for item in program.nodes if item.operator == "passages.locate")
        ranked = locator.inputs["ranked"]
        assert isinstance(ranked, PortRef)
        stage = node(
            "think",
            "vcp.think",
            {
                "observation": ref("observe", "observation"),
                "stages": ref("query", "stages"),
                "ranked": ranked,
            },
        )
        return program.model_copy(
            update={
                "nodes": (*program.nodes, stage),
                "outputs": {**program.outputs, "thinking": ref("think", "routes")},
            }
        )

    return {
        "text_only": text,
        "lexical": lexical_only,
        "dense": dense,
        "tagmemo": thinking(tagmemo),
        "rivermemo": thinking(river),
        "hybrid": hybrid,
        "dtsc": thinking(dtsc),
        "research": thinking(research),
        "fixed_set": fixed,
    }
