"""Atomic target changes form a dependency DAG; failed dependants do not erase valid prerequisites."""

from __future__ import annotations

from graphlib import CycleError, TopologicalSorter
from typing import TYPE_CHECKING

from nous.platform.call_context import CallContext
from nous.platform.errors import DomainError
from nous_sdk.contracts.ego_model import SeedAdoption, SeedProposal
from nous_sdk.contracts.growth import (
    GrowthAction,
    GrowthAdoption,
    GrowthDraft,
    GrowthMerge,
    GrowthProposal,
    GrowthProposalAction,
    GrowthSnapshot,
    GrowthSplit,
)
from nous_sdk.contracts.maintenance import MaintenanceGroup, MaintenanceProposal
from pydantic import JsonValue, TypeAdapter, ValidationError

from nous.cognition.growth_proposal import normalize_growth
from nous.platform.persistence import ReadView, WriteTx

if TYPE_CHECKING:
    from .service import MaintenanceService


def properties(action: GrowthAction) -> tuple[tuple[str, ...], tuple[str, ...], tuple[str, ...]]:
    if isinstance(action, GrowthAdoption):
        return (
            (action.key,),
            (
                *action.evidence_keys,
                *(binding.key for binding in action.bindings),
                *((action.target_key,) if action.target_key else ()),
            ),
            (action.target_key,) if action.target_key else (),
        )
    if isinstance(action, GrowthSplit):
        members = tuple(key for part in action.partitions for key in part.member_keys)
        return (
            (action.key, *(part.key for part in action.partitions)),
            (action.source_key, *members),
            (action.source_key, *members),
        )
    if isinstance(action, GrowthMerge):
        return (
            (action.key,),
            (action.target_key, *action.source_keys),
            (action.target_key, *action.source_keys),
        )
    return (action.key,), (action.target_key,), (action.target_key,)


def raw_properties(
    value: JsonValue, index: int
) -> tuple[tuple[str, ...], tuple[str, ...], tuple[str, ...]]:
    if not isinstance(value, dict):
        return (), (), ()
    key = value.get("key")
    keys = (key,) if isinstance(key, str) and key else ()
    target = value.get("target_key")
    targets = (target,) if isinstance(target, str) and target else ()
    evidence = value.get("evidence_keys", [])
    dependencies = list(targets)
    if isinstance(evidence, list):
        dependencies.extend(item for item in evidence if isinstance(item, str))

    def references(item: JsonValue) -> None:
        if isinstance(item, dict):
            if set(item) == {"key"} and isinstance(item["key"], str):
                dependencies.append(item["key"])
            else:
                for child in item.values():
                    references(child)
        elif isinstance(item, list):
            for child in item:
                references(child)

    references(value.get("body"))
    return keys, tuple(dependencies), targets


async def compile_proposal(
    self: MaintenanceService,
    view: ReadView | WriteTx,
    ctx: CallContext,
    document: MaintenanceProposal,
    snapshot: GrowthSnapshot,
    *,
    role: str,
    previous: tuple[MaintenanceGroup, ...] = (),
) -> tuple[MaintenanceGroup, ...]:
    parsed: list[GrowthAction | None] = []
    errors: dict[int, dict[str, JsonValue]] = {}
    metadata: list[tuple[tuple[str, ...], tuple[str, ...], tuple[str, ...]]] = []
    summary, rationale = (
        document.summary or "Proposal groups",
        document.rationale or "Retained proposal input",
    )
    for index, raw in enumerate(document.actions):
        try:
            if role == "seed":
                action = TypeAdapter(SeedAdoption).validate_python(raw)
                normalized = normalize_growth(
                    SeedProposal(summary=summary, rationale=rationale, actions=(action,))
                )
            else:
                action = TypeAdapter(GrowthProposalAction).validate_python(raw)
                normalized = normalize_growth(
                    GrowthProposal(summary=summary, rationale=rationale, actions=(action,))
                )
        except ValidationError as error:
            parsed.append(None)
            errors[index] = TypeAdapter(dict[str, JsonValue]).validate_python(
                {
                    "code": "proposal_action_invalid",
                    "message": "The concrete action does not match its declared contract",
                    "validation_errors": TypeAdapter(list[dict[str, JsonValue]]).validate_json(
                        error.json(include_input=False, include_url=False)
                    ),
                }
            )
            metadata.append(raw_properties(raw, index))
        else:
            native = normalized.actions[0]
            parsed.append(native)
            metadata.append(properties(native))
    return await compile_actions(
        self, view, ctx, document, snapshot, parsed, metadata, errors, previous
    )


async def compile_actions(
    self: MaintenanceService,
    view: ReadView | WriteTx,
    ctx: CallContext,
    document: MaintenanceProposal,
    snapshot: GrowthSnapshot,
    actions: list[GrowthAction | None],
    metadata: list[tuple[tuple[str, ...], tuple[str, ...], tuple[str, ...]]],
    errors: dict[int, dict[str, JsonValue]],
    previous: tuple[MaintenanceGroup, ...],
) -> tuple[MaintenanceGroup, ...]:
    parents = list(range(len(actions)))

    def root(index: int) -> int:
        while parents[index] != index:
            parents[index] = parents[parents[index]]
            index = parents[index]
        return index

    def join(left: int, right: int) -> None:
        parents[root(right)] = root(left)

    owners: dict[str, int] = {}
    duplicate: set[str] = set()
    for index, (keys, _, _) in enumerate(metadata):
        for key in keys:
            if key in owners:
                duplicate.add(key)
            else:
                owners[key] = index
    writers: dict[str, int] = {}
    adopted_keys = {
        key for group in previous if group.status == "adopted" for key in group.action_keys
    }
    for index, (_, _, targets) in enumerate(metadata):
        for target in targets:
            if target in owners and owners[target] != index and target not in adopted_keys:
                join(index, owners[target])
            if target in writers:
                join(index, writers[target])
            writers[target] = index
    partitions: dict[int, list[int]] = {}
    for index in range(len(actions)):
        partitions.setdefault(root(index), []).append(index)
    groups: dict[str, MaintenanceGroup] = {}
    by_index: dict[int, str] = {}
    for indices in partitions.values():
        keys = tuple(key for index in indices for key in metadata[index][0])
        key = keys[0] if keys and keys[0] not in groups else f"item:{indices[0]}"
        for index in indices:
            by_index[index] = key
        bad = [errors[index] for index in indices if index in errors]
        if duplicate.intersection(keys):
            bad.append(
                TypeAdapter(dict[str, JsonValue]).validate_python(
                    {
                        "code": "growth_key_conflict",
                        "message": "Proposal local keys are duplicated",
                        "keys": sorted(duplicate.intersection(keys)),
                    }
                )
            )
        native = tuple(action for index in indices if (action := actions[index]) is not None)
        draft = (
            GrowthDraft(
                summary=document.summary or "Proposal groups",
                rationale=document.rationale or "Retained proposal input",
                actions=native,
            )
            if not bad
            else None
        )
        groups[key] = MaintenanceGroup(
            key=key,
            draft=draft,
            raw_actions=tuple(document.actions[index] for index in indices),
            action_keys=keys,
            status="failed" if bad else "ready",
            error=TypeAdapter(dict[str, JsonValue]).validate_python(
                {
                    "code": "proposal_group_invalid",
                    "message": "One or more coupled actions require editing",
                    "actions": bad,
                }
            )
            if bad
            else None,
        )
    graph: dict[str, set[str]] = {key: set() for key in groups}
    for index, (_, dependencies, _) in enumerate(metadata):
        key = by_index[index]
        graph[key].update(
            by_index[owners[dependency]]
            for dependency in dependencies
            if dependency in owners and by_index[owners[dependency]] != key
        )
    old = {group.key: group for group in previous if group.status == "adopted"}
    for key, group in tuple(groups.items()):
        groups[key] = group.model_copy(update={"dependencies": tuple(sorted(graph[key]))})
        if key in old:
            if old[key].raw_actions != group.raw_actions:
                raise DomainError(
                    "maintenance_adopted_group_immutable",
                    "Adopted proposal actions cannot be edited",
                    group=key,
                )
            groups[key] = old[key]
    if set(old) - set(groups):
        raise DomainError(
            "maintenance_adopted_group_immutable", "Adopted proposal actions cannot be removed"
        )
    sorter = TopologicalSorter(graph)
    cycle: set[str] = set()
    try:
        sorter.prepare()
    except CycleError as error:
        cycle = set(str(key) for key in error.args[1])
    processed: set[str] = set()
    ordered: list[str] = []
    while ready := sorter.get_ready():
        for key in ready:
            group = groups[key]
            ordered.append(key)
            if group.status == "adopted":
                pass
            elif any(
                groups[dependency].status in {"failed", "blocked"} for dependency in graph[key]
            ):
                groups[key] = group.model_copy(
                    update={
                        "status": "blocked",
                        "error": {
                            "code": "proposal_dependency_failed",
                            "message": "A prerequisite group requires editing",
                            "groups": sorted(graph[key]),
                        },
                    }
                )
            elif group.draft is not None:
                closure: set[str] = set()

                def ancestors(current: str, found: set[str] = closure) -> None:
                    for dependency in graph[current]:
                        if dependency not in found:
                            ancestors(dependency)
                            found.add(dependency)

                ancestors(key)
                prefix = [
                    value
                    for value in ordered
                    if value in closure and groups[value].status != "adopted"
                ]
                combined = tuple(
                    action
                    for item in (*prefix, key)
                    for draft in (groups[item].draft,)
                    if draft is not None
                    for action in draft.actions
                )
                effective = await self.working_snapshot_in(
                    view, ctx, snapshot, tuple(groups.values())
                )
                try:
                    await self.growth.validate_growth_in(
                        view, ctx, group.draft.model_copy(update={"actions": combined}), effective
                    )
                except DomainError as error:
                    groups[key] = group.model_copy(
                        update={
                            "status": "failed",
                            "error": {
                                "code": error.code,
                                "message": str(error),
                                "context": error.details,
                            },
                        }
                    )
            processed.add(key)
            sorter.done(key)
    for key in set(groups) - processed:
        group = groups[key]
        groups[key] = group.model_copy(
            update={
                "status": "failed" if key in cycle else "blocked",
                "error": {
                    "code": "proposal_dependency_cycle",
                    "message": "Proposal dependencies contain a cycle",
                    "groups": sorted(cycle),
                },
            }
        )
        ordered.append(key)
    return tuple(groups[key] for key in ordered)
