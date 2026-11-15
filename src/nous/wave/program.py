"""Compile a typed Wave operator graph for one snapshot-bound execution."""

from __future__ import annotations

import json
from collections.abc import Callable, Sequence
from dataclasses import dataclass
from graphlib import CycleError, TopologicalSorter
from typing import TYPE_CHECKING, Literal, cast

from pydantic import BaseModel, ConfigDict, Field, JsonValue, model_validator

from nous.common.errors import NousError
from nous.configuration.metadata import ConfigSequence
from nous.models.evaluation_contracts import EvaluationScale

from .contracts import (
    PortRef,
    ProgramNode,
    WaveProgram,
)

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
    host: bool = False


# A [] suffix marks a multi-input port. Numerical parameter schemas are supplied
# by Native together with the implementation, not duplicated in this catalog.
CATALOG = {
    "vcp.observe": Operator(
        {"signal": "signal"},
        {
            "observation": "observation",
            "enhanced": "vector",
            "local": "vector",
            "transfer": "vector",
        },
        frozenset({"vectors", "vcp"}),
    ),
    "ann.search": Operator(
        {"vector": "vector"},
        {"candidates": "candidates"},
        frozenset({"vectors"}),
    ),
    "lexical.search": Operator(
        {"text": "lexical"},
        {"candidates": "candidates"},
        frozenset({"text"}),
    ),
    "concept.direct": Operator(
        {"observation": "observation"},
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
        {"candidates": "candidates", "vector": "vector"},
        {"ranked": "ranked"},
        frozenset({"vectors"}),
    ),
    "rank.fuse": Operator({"rankings": "ranked[]"}, {"ranked": "ranked"}),
    "context.recall": Operator(
        {"signals": "context_lexical"},
        {"candidates": "candidates"},
        frozenset({"text"}),
    ),
    "rank.context": Operator(
        {"ranked": "ranked", "candidates": "candidates"},
        {"ranked": "ranked"},
    ),
    "passages.locate": Operator({"ranked": "ranked"}, {"passages": "passages"}),
    "passages.evaluate": Operator(
        {"passages": "passages"},
        {"passages": "passages"},
        pure=False,
        host=True,
    ),
    "passages.select": Operator({"passages": "passages"}, {"selected": "selection"}, host=True),
    "delivery.read": Operator(
        {"selected": "selection"},
        {"evidence": "evidence"},
        pure=False,
        host=True,
    ),
}
BUILTINS = {
    "query": {
        "raw_vector": "vector",
        "contextual_signal": "signal",
        "lexical": "lexical",
        "context_lexical": "context_lexical",
        "stages": "stages",
    },
    "scope": {"candidates": "candidates"},
}


@dataclass(frozen=True)
class CompiledProgram:
    name: str
    nodes: tuple[ProgramNode, ...]
    outputs: dict[str, PortRef]
    requirements: frozenset[str]
    native_outputs: dict[str, PortRef]

    def native_plan(self) -> dict[str, object]:
        """Encode the numerical nodes and their actual cross-language outputs."""
        return {
            "nodes": [
                node.model_dump(mode="json")
                for node in self.nodes
                if not CATALOG[node.operator].host
            ],
            "outputs": {
                key: ref.model_dump(mode="json") for key, ref in self.native_outputs.items()
            },
        }


@dataclass(frozen=True)
class ProgramSelection:
    program: CompiledProgram
    source: Literal["explicit", "default", "fallback", "exploration"]
    unavailable_capabilities: tuple[str, ...] = ()
    fallback_from: str | None = None


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
    operator_schemas = cast(dict[str, dict[str, JsonValue]], native_schemas.get("operators", {}))
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
            parameters = cast(
                dict[str, JsonValue],
                EvaluationParameters.model_validate(node.parameters).model_dump(mode="json"),
            )
        else:
            properties = cast(
                dict[str, dict[str, JsonValue]],
                operator_schemas.get(node.operator, {}).get("properties", {}),
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
            weights = cast(list[float], parameters.get("weights") or [])
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
    host_nodes = tuple(node for node in compiled if CATALOG[node.operator].host)
    native_nodes = [node for node in compiled if not CATALOG[node.operator].host]
    native_outputs: dict[str, PortRef] = {}
    for node in host_nodes:
        for value in node.inputs.values():
            for ref in value if isinstance(value, tuple) else (value,):
                if ref.node in by_id and not CATALOG[by_id[ref.node].operator].host:
                    if ref not in native_outputs.values():
                        native_outputs[f"value:{len(native_outputs)}"] = ref
    for name_, ref in outputs.items():
        if not CATALOG[by_id[ref.node].operator].host:
            native_outputs[name_] = ref
    for node in native_nodes:
        for value in node.inputs.values():
            for ref in value if isinstance(value, tuple) else (value,):
                if ref.node in by_id and CATALOG[by_id[ref.node].operator].host:
                    raise _error("Native nodes cannot consume asynchronous outputs", node=node.id)
    return CompiledProgram(
        name,
        tuple(compiled),
        outputs,
        frozenset(requirements),
        native_outputs,
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


def select_program(
    config: WaveConfig,
    name: str | None,
    *,
    explore: bool,
    capabilities: Sequence[str],
) -> ProgramSelection:
    """Select one program against explicit available retrieval capabilities."""
    source: Literal["explicit", "default", "fallback", "exploration"] = (
        "explicit" if name else "exploration" if explore else "default"
    )
    program = resolve_program(config, name or (config.exploration_program if explore else None))
    missing = program.requirements - set(capabilities)
    unavailable: tuple[str, ...] = ()
    fallback_from = None
    if missing and name is None and not explore and config.unavailable_program:
        unavailable = tuple(sorted(missing))
        fallback_from = program.name
        program = resolve_program(config, config.unavailable_program)
        source = "fallback"
        missing = program.requirements - set(capabilities)
    if missing:
        raise NousError(
            "wave.capability_unavailable",
            "Available retrieval capabilities cannot execute the requested program",
            details={"program": program.name, "missing": sorted(missing)},
        )
    return ProgramSelection(program, source, unavailable, fallback_from)


def compile_expression(
    program: CompiledProgram,
    tree: dict[str, object],
    branches: list[dict[str, object]],
    *,
    prefer: tuple[str, ...],
    avoid: tuple[str, ...],
    sort: str | None,
) -> dict[str, object]:
    """Bind each expression branch to the numerical graph's declared dependencies."""
    native = [node for node in program.nodes if not CATALOG[node.operator].host]
    terminal_ids = {ref.node for name, ref in program.native_outputs.items() if name != "thinking"}
    prefix = "expression:"
    while any(node.id.startswith(prefix) for node in native):
        prefix += ":"
    nodes: list[dict[str, object]] = []

    def reference(node: str, port: str = "ranked") -> dict[str, str]:
        return {"node": node, "port": port}

    def branch_ref(ref: PortRef, index: int) -> dict[str, str]:
        if ref.node == "query":
            return reference(f"{prefix}query:{index}", ref.port)
        if ref.node in BUILTINS:
            return ref.model_dump()
        return reference(f"{prefix}node:{index}:{ref.node}", ref.port)

    active: list[int] = []
    for index, branch in enumerate(branches):
        scope_only = not str(branch["text"]).strip() and not branch["core_rows"]
        if scope_only:
            nodes.append(
                {
                    "id": f"{prefix}scope:{index}",
                    "operator": "rank.scope",
                    "inputs": {},
                    "parameters": {},
                    "branch": index,
                }
            )
            continue
        active.append(index)
        for node in native:
            if node.id in terminal_ids:
                continue
            inputs = {
                key: [branch_ref(ref, index) for ref in value]
                if isinstance(value, tuple)
                else branch_ref(value, index)
                for key, value in node.inputs.items()
            }
            nodes.append(
                {
                    "id": f"{prefix}node:{index}:{node.id}",
                    "operator": node.operator,
                    "inputs": inputs,
                    "parameters": node.parameters,
                    "branch": index,
                }
            )

    counter = 0
    combined_ranks: dict[PortRef, dict[str, str]] = {}

    def combine_rank(source: PortRef) -> dict[str, str]:
        nonlocal counter
        if source in combined_ranks:
            return combined_ranks[source]
        ranks: list[dict[str, str]] = []
        for index, branch in enumerate(branches):
            source_ref = (
                branch_ref(source, index)
                if index in active
                else reference(f"{prefix}scope:{index}")
            )
            counter += 1
            adjusted = f"{prefix}adjust:{counter}"
            nodes.append(
                {
                    "id": adjusted,
                    "operator": "rank.adjust",
                    "branch": index,
                    "inputs": {"ranked": source_ref},
                    "parameters": {
                        key: branch[key] for key in ("name", "limit", "prefer", "avoid", "sort")
                    },
                }
            )
            ranks.append(reference(adjusted))

        def combine(value: dict[str, object]) -> dict[str, str]:
            nonlocal counter
            if value["kind"] == "term":
                return ranks[cast(int, value["branch"])]
            children = [
                combine(cast(dict[str, object], child))
                for child in cast(list[object], value["children"])
            ]
            counter += 1
            node_id = f"{prefix}combine:{counter}"
            nodes.append(
                {
                    "id": node_id,
                    "operator": "rank.combine",
                    "inputs": {"rankings": children},
                    "parameters": {
                        key: value.get(key) for key in ("kind", "limit", "prefer", "avoid", "sort")
                    },
                }
            )
            return reference(node_id)

        combined = combine(tree)
        counter += 1
        final = f"{prefix}final:{counter}"
        nodes.append(
            {
                "id": final,
                "operator": "rank.adjust",
                "inputs": {"ranked": combined},
                "parameters": {"prefer": list(prefer), "avoid": list(avoid), "sort": sort},
            }
        )
        combined_ranks[source] = reference(final)
        return combined_ranks[source]

    for node in native:
        if node.id not in terminal_ids:
            continue
        inputs: dict[str, object] = {}
        for port, value in node.inputs.items():
            if CATALOG[node.operator].inputs[port] == "ranked":
                inputs[port] = combine_rank(cast(PortRef, value))
            elif isinstance(value, tuple):
                inputs[port] = [ref.model_dump() for ref in value]
            else:
                inputs[port] = value.model_dump()
        nodes.append(
            {
                "id": node.id,
                "operator": node.operator,
                "inputs": inputs,
                "parameters": node.parameters,
            }
        )

    outputs = {key: ref.model_dump(mode="json") for key, ref in program.native_outputs.items()}
    if "thinking" in program.outputs:
        thinking_ref = program.outputs["thinking"]
        combined_thinking = f"{prefix}thinking"
        nodes.append(
            {
                "id": combined_thinking,
                "operator": "thinking.combine",
                "inputs": {"routes": [branch_ref(thinking_ref, index) for index in active]},
                "parameters": {
                    "branches": [branches[index]["name"] for index in active],
                    "missing": [
                        branches[index]["name"]
                        for index in range(len(branches))
                        if index not in active
                    ],
                },
            }
        )
        outputs["thinking"] = reference(combined_thinking, "routes")
    return {"nodes": nodes, "outputs": outputs, "branches": branches, "query_prefix": prefix}


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

    def search(name: str, source: PortRef) -> ProgramNode:
        return node(name, "ann.search", {"vector": source})

    def lane(id: str, source: str, name: str) -> ProgramNode:
        return node(id, "rank.from_lane", {"candidates": ref(source, "candidates")}, lane=name)

    semantic = search("semantic", ref("query", "raw_vector"))
    lexical = node(
        "lexical",
        "lexical.search",
        {"text": ref("query", "lexical")},
    )
    observe = node(
        "observe",
        "vcp.observe",
        {"signal": ref("query", "contextual_signal")},
    )
    fields = [search(name, ref("observe", name)) for name in ("enhanced", "local", "transfer")]
    direct = node(
        "direct",
        "concept.direct",
        {"observation": ref("observe", "observation")},
    )
    context = node(
        "context_recall",
        "context.recall",
        {"signals": ref("query", "context_lexical")},
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
                    {"ranked": ranked},
                    **({"signals": cast(list[JsonValue], ["lexical"])} if text_only else {}),
                ),
                node(
                    "select",
                    "passages.select",
                    {"passages": ref("locate", "passages")},
                ),
                node(
                    "deliver",
                    "delivery.read",
                    {"selected": ref("select", "selected")},
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
