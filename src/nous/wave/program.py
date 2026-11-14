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

from .contracts import (
    CandidateSet,
    DeliveryBudget,
    PortRef,
    ProgramNode,
    ProgramRunFacts,
    WaveProgram,
)
from .serialization import content_signature

if TYPE_CHECKING:
    from .configuration import WaveConfig


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
    native_plan: dict[str, object]
    delivery_steps: tuple[ProgramNode, ...]


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
        if node.operator == "rank.fuse":
            references = inputs["rankings"]
            assert isinstance(references, tuple)
            weights = TypeAdapter(list[float]).validate_python(parameters.get("weights") or [])
            if weights and len(weights) != len(references):
                raise _error("Fusion weights must match ranking inputs", node=node.id)
            unique: dict[PortRef, float] = {}
            for index, reference in enumerate(references):
                unique.setdefault(reference, weights[index] if weights else 1.0)
            inputs["rankings"] = tuple(unique)
            parameters["weights"] = list(unique.values()) if weights else []
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
    by_id = {node.id: node for node in compiled}
    locators = [node for node in compiled if node.operator == "passages.locate"]
    if len(locators) != 1:
        raise _error("A query must have one localization node", program=name)
    locator = locators[0]
    steps: list[ProgramNode] = []
    current = outputs["evidence"]
    ports = {
        "delivery.read": ("evidence", "selected"),
        "passages.select": ("selected", "passages"),
        "passages.evaluate": ("passages", "passages"),
    }
    while current.node != locator.id:
        node = by_id[current.node]
        expected = ports.get(node.operator)
        if expected is None or current.port != expected[0]:
            raise _error("Invalid delivery chain", node=node.id)
        steps.append(node)
        source = node.inputs[expected[1]]
        if isinstance(source, tuple):
            raise _error("Invalid delivery input", node=node.id)
        current = source
    if current.port != "passages":
        raise _error("Invalid localization output", node=locator.id)
    needed = {locator.id}
    if "thinking" in outputs:
        needed.add(outputs["thinking"].node)
    pending = list(needed)
    while pending:
        for source in by_id[pending.pop()].inputs.values():
            for ref in source if isinstance(source, tuple) else (source,):
                if ref.node in by_id and ref.node not in needed:
                    needed.add(ref.node)
                    pending.append(ref.node)
    native_nodes = [node for node in compiled if node.id in needed]
    if any(node.operator in ports for node in native_nodes):
        raise _error("Localization cannot depend on a delivery stage", node=locator.id)
    native_outputs: dict[str, object] = {"evidence": {"node": locator.id, "port": "passages"}}
    if "thinking" in outputs:
        native_outputs["thinking"] = outputs["thinking"].model_dump(mode="json")
    native_plan: dict[str, object] = {
        "nodes": [node.model_dump(mode="json") for node in native_nodes],
        "outputs": native_outputs,
    }
    canonical = definition.model_dump_json()
    return CompiledProgram(
        name,
        definition,
        tuple(compiled),
        outputs,
        frozenset(requirements),
        hashlib.sha256(canonical.encode()).hexdigest(),
        native_plan,
        tuple(reversed(steps)),
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


def delivery_budget(
    program: CompiledProgram, requested: DeliveryBudget | None = None
) -> DeliveryBudget | None:
    """Resolve the actual budget once from the request and selected program."""
    if requested is not None:
        return requested
    node = next((item for item in program.nodes if item.operator == "passages.select"), None)
    value = node.parameters.get("budget") if node else None
    return DeliveryBudget.model_validate(value) if value is not None else None


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
    """Record the compiled graph and the settings that its execution consumes."""
    operators = {node.operator for node in program.nodes}
    names = {
        "top_k",
        "grouping",
        "passages_per_group",
        "candidates_per_group",
        "group_scan_limit",
        "rrf_k",
        "preference_weight",
        "dedup_threshold",
        "include_counterevidence",
        "explore",
        "readout",
    }
    if "ann.search" in operators:
        names.add("ann_search_effort")
    if "vcp.observe" in operators:
        names.add("pipeline")
    if "dtsc.readout" in operators:
        names.add("dtsc")
    if "vcp.think" in operators:
        names.add("thinking_min_closure")
    selected = {key: value for key, value in options.items() if key in names}
    selected["candidate_set"] = (
        {"count": len(candidate_set.entries), "rank_scope": candidate_set.rank_scope}
        if candidate_set is not None
        else None
    )
    selected["expression"] = (
        {"kind": expression_kind, "terms": expression_terms} if expression_kind else None
    )
    if operators & {"context.recall", "passages.locate", "passages.evaluate"}:
        selected["context"] = dict(context_options)
    if "passages.evaluate" in operators:
        selected["evaluation"] = {
            "limits": dict(evaluation_options),
            "tasks": dict(evaluation_tasks),
        }
    facts = {
        "name": program.name,
        "definition_signature": program.signature,
        "requested_program": requested_program,
        "selection": selection,
        "requirements": sorted(program.requirements),
        "nodes": [node.model_dump(mode="json") for node in program.nodes],
        "outputs": {key: value.model_dump(mode="json") for key, value in program.outputs.items()},
        "options": selected,
        "budget": budget.model_dump(mode="json") if budget else None,
        "space_id": space_id,
        "space_signature": space_signature,
        "space_definition": space_definition,
    }
    return ProgramRunFacts.model_validate({**facts, "signature": content_signature(facts)})


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

    snapshot, allowed = ref("snapshot", "handle"), ref("scope", "allowed")

    def search(name: str, source: PortRef) -> ProgramNode:
        return node(
            name, "ann.search", {"vector": source, "snapshot": snapshot, "allowed": allowed}
        )

    def lane(id: str, source: str, name: str) -> ProgramNode:
        return node(id, "rank.from_lane", {"candidates": ref(source, "candidates")}, lane=name)

    semantic = search("semantic", ref("query", "raw_vector"))
    lexical = node(
        "lexical",
        "lexical.search",
        {"text": ref("query", "lexical"), "snapshot": snapshot, "allowed": allowed},
    )
    observe = node(
        "observe",
        "vcp.observe",
        {"signal": ref("query", "contextual_signal"), "snapshot": snapshot},
    )
    fields = [search(name, ref("observe", name)) for name in ("enhanced", "local", "transfer")]
    direct = node(
        "direct",
        "concept.direct",
        {"observation": ref("observe", "observation"), "snapshot": snapshot, "allowed": allowed},
    )
    context = node(
        "context_recall",
        "context.recall",
        {"signals": ref("query", "context_lexical"), "snapshot": snapshot, "allowed": allowed},
    )
    union = node(
        "union",
        "candidates.union",
        {
            "sets": tuple(
                ref(name, "candidates")
                for name in (
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
    calibration = node(
        "calibrate",
        "vcp.calibrate",
        {"observation": ref("observe", "observation"), "candidates": ref("union", "candidates")},
    )
    shared = [observe, semantic, lexical, *fields, direct, context, union]

    def finish(
        stages: list[ProgramNode],
        rank: ProgramNode,
        *,
        contextual: bool = True,
        in_union: bool = False,
        text_only: bool = False,
        thinking: bool = False,
    ) -> WaveProgram:
        steps = [*stages, rank]
        ranked = ref(rank.id, "ranked")
        if contextual and not in_union:
            steps.extend(
                [
                    context,
                    node(
                        "context",
                        "rank.context",
                        {"ranked": ranked, "candidates": ref("context_recall", "candidates")},
                    ),
                ]
            )
            ranked = ref("context", "ranked")
        steps.extend(
            [
                node(
                    "locate",
                    "passages.locate",
                    {"ranked": ranked, "intent": ref("query", "intent"), "snapshot": snapshot},
                    **({"signals": cast(list[JsonValue], ["lexical"])} if text_only else {}),
                ),
                node(
                    "select",
                    "passages.select",
                    {"passages": ref("locate", "passages"), "delivery": ref("delivery", "request")},
                ),
                node(
                    "deliver",
                    "delivery.read",
                    {"selected": ref("select", "selected"), "materials": ref("scope", "materials")},
                ),
            ]
        )
        outputs = {"evidence": ref("deliver", "evidence")}
        if thinking:
            steps.append(
                node(
                    "think",
                    "vcp.think",
                    {
                        "observation": ref("observe", "observation"),
                        "stages": ref("query", "stages"),
                        "ranked": ranked,
                    },
                )
            )
            outputs["thinking"] = ref("think", "routes")
        return WaveProgram(nodes=tuple(steps), outputs=outputs)

    river_rank = node(
        "rank",
        "river.readout",
        {"observation": ref("observe", "observation"), "pool": ref("calibrate", "pool")},
    )
    dtsc_rank = node(
        "rank",
        "dtsc.readout",
        {"observation": ref("observe", "observation"), "candidates": ref("union", "candidates")},
    )
    hybrid_inputs = [
        lane("dense_rank", "semantic", "query_knn"),
        lane("text_rank", "lexical", "bm25"),
    ]
    hybrid_rank = node(
        "rank", "rank.fuse", {"rankings": tuple(ref(item.id, "ranked") for item in hybrid_inputs)}
    )
    research_inputs = [
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
                "snapshot": snapshot,
            },
        ),
    ]
    research_rank = node(
        "rank", "rank.fuse", {"rankings": tuple(ref(item.id, "ranked") for item in research_inputs)}
    )
    return {
        "text_only": finish([lexical], lane("rank", "lexical", "bm25"), text_only=True),
        "lexical": finish(
            [lexical], lane("rank", "lexical", "bm25"), text_only=True, contextual=False
        ),
        "dense": finish([semantic], lane("rank", "semantic", "query_knn")),
        "tagmemo": finish(
            [observe, fields[0]], lane("rank", "enhanced", "denoised_field_knn"), thinking=True
        ),
        "rivermemo": finish([*shared, calibration], river_rank, in_union=True, thinking=True),
        "hybrid": finish([semantic, lexical, *hybrid_inputs], hybrid_rank),
        "dtsc": finish(shared, dtsc_rank, in_union=True, thinking=True),
        "research": finish(
            [*shared, calibration, *research_inputs], research_rank, in_union=True, thinking=True
        ),
        "fixed_set": finish([], lane("rank", "scope", "provided")),
    }
