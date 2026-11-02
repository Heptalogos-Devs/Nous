"""Translate A-Memorix selectors into canonical native read/selection contracts."""

from __future__ import annotations

from datetime import UTC, datetime
from decimal import Decimal
from typing import TYPE_CHECKING

from nous_sdk.contracts import (
    CallContext,
    CognitiveRef,
    DomainError,
    RecordsQuery,
    TimeFilter,
    TimeRange,
)
from nous_sdk.contracts.cognitive_identity import CognitiveKind
from nous_sdk.contracts.graph_management import GraphSearchQuery
from nous_sdk.contracts.management import SourceSelection
from nous_sdk.contracts.retention import RetentionExpectation
from pydantic import FiniteFloat, JsonValue, TypeAdapter

if TYPE_CHECKING:
    from .service import AMemorixService
    from .sources import Selection


def tokens(value: JsonValue) -> tuple[str, ...]:
    if value is None:
        return ()
    if isinstance(value, str):
        return (value,) if value.strip() else ()
    return TypeAdapter(tuple[str, ...]).validate_python(value)


async def selection(
    service: AMemorixService, ctx: CallContext, command: Selection
) -> SourceSelection:
    mode = command.mode.strip().lower() or "source"
    raw = command.selector
    if raw is None:
        values = command.model_dump(
            mode="json",
            include={
                "source",
                "sources",
                "query",
                "items",
                "hash",
                "hashes",
                "name",
                "names",
                "content",
                "paragraph_hashes",
                "entity_hashes",
                "relation_hashes",
            },
            exclude_none=True,
        )
    elif isinstance(raw, dict):
        values = raw
    elif isinstance(raw, str):
        values = {"query": raw}
    elif isinstance(raw, list):
        values = {"items": raw}
    else:
        raise DomainError(
            "source_selection_invalid", "Deletion selector requires text, identities or an object"
        )
    sources: set[str] = set()
    references: dict[str, CognitiveRef] = {}
    restoring = command.action == "restore"
    purging = command.action == "purge"

    async def collect(
        kind: CognitiveKind, hashes: tuple[str, ...], text: str | None = None, label: bool = False
    ) -> None:
        if not hashes and not text:
            return
        page = await service.app.cognition.records(
            ctx,
            RecordsQuery(
                kinds=(kind,),
                object_ids=hashes,
                text=text if not hashes else None,
                text_match="label" if label else "contains",
                purpose="management" if restoring or purging else "current",
                states=("deleted",) if restoring or purging else (),
                limit=1000,
            ),
        )
        for entry in page.items:
            record = entry.record
            if record.purged or (record.deleted and not restoring and not purging):
                continue
            references[record.reference.id] = record.reference
        if not page.items and hashes and text:
            await collect(kind, (), text, label)

    direct = tuple(
        dict.fromkeys(
            (
                *tokens(values.get("hashes")),
                *tokens(values.get("items")),
                *tokens(values.get("hash")),
            )
        )
    )
    query_values = tokens(values.get("query")) or tokens(values.get("content"))
    query = query_values[0] if query_values else None
    if mode == "source":
        sources.update(
            (
                *tokens(values.get("sources")),
                *tokens(values.get("source")),
                *tokens(values.get("items")),
                *tokens(values.get("query")),
            )
        )
    elif mode == "paragraph":
        await collect("memory", direct, query)
    elif mode == "entity":
        await collect("entity", direct)
        for name in (
            *tokens(values.get("names")),
            *tokens(values.get("name")),
            *tokens(values.get("query")),
        ):
            await collect("entity", (), name, True)
    elif mode == "relation":
        await collect("association", direct, query)
        if query and not restoring and not purging:
            graph = await service.app.graphs.search_graph(
                ctx, GraphSearchQuery(text=query, limit=1000)
            )
            for item in graph.matches:
                if item.record.reference.kind == "association":
                    references[item.record.reference.id] = item.record.reference
        elif query:
            entities = await service.app.cognition.records(
                ctx,
                RecordsQuery(
                    kinds=("entity",),
                    text=query,
                    text_match="label",
                    purpose="management",
                    limit=1000,
                ),
            )
            if entities.items:
                relations = await service.app.cognition.records(
                    ctx,
                    RecordsQuery(
                        kinds=("association",),
                        entity_ids=tuple(entry.record.reference.id for entry in entities.items),
                        states=("deleted",),
                        purpose="management",
                        limit=1000,
                    ),
                )
                references.update(
                    (entry.record.reference.id, entry.record.reference)
                    for entry in relations.items
                    if not entry.record.purged
                )
    elif mode == "mixed":
        sources.update((*tokens(values.get("sources")), *tokens(values.get("source"))))
        await collect("memory", tokens(values.get("paragraph_hashes")))
        await collect("entity", tokens(values.get("entity_hashes")))
        await collect("association", tokens(values.get("relation_hashes")))
    if (
        purging
        and not sources
        and not references
        and command.selector is None
        and not direct
        and query is None
    ):
        return SourceSelection(
            records=RecordsQuery(
                kinds=("memory", "entity", "association"),
                states=("deleted",),
                purpose="management",
                limit=command.limit,
                times=(
                    TimeFilter(
                        axis="recorded",
                        range=TimeRange(
                            end=round(
                                datetime.now(UTC).timestamp() * 1000
                                - (command.grace_hours or 0) * 3600000
                            )
                            + 1
                        ),
                    ),
                ),
            )
        )
    return SourceSelection(
        source_names=tuple(sorted(sources)),
        references=tuple(references[key] for key in sorted(references)),
        retention_expectations=relation_expectations(
            values.get("expected_relation_states"), tuple(references.values())
        ),
    )


def relation_expectations(
    value: JsonValue, references: tuple[CognitiveRef, ...]
) -> tuple[RetentionExpectation, ...]:
    if value is None:
        return ()
    states = TypeAdapter(dict[str, JsonValue]).validate_python(value)
    required = {
        "expected_lifecycle_revision",
        "expected_retention_strength",
        "expected_retention_anchor_at",
        "expected_inactive_since",
        "expected_inactive_reason",
        "expected_is_inactive",
        "expected_is_permanent",
        "expected_is_pinned",
        "expected_protected_until",
    }

    def milliseconds(raw: JsonValue) -> int | None:
        if raw is None:
            return None
        value = Decimal(str(TypeAdapter(FiniteFloat).validate_python(raw))) * 1000
        if value != value.to_integral_value():
            raise DomainError(
                "retention_precondition_failed",
                "Retention times require exact millisecond precision",
            )
        return int(value)

    result: list[RetentionExpectation] = []
    for reference in references:
        if reference.kind != "association":
            continue
        state = states.get(reference.id)
        if not isinstance(state, dict) or not required.issubset(state):
            raise DomainError(
                "retention_precondition_failed",
                "A complete retention expectation is required for every selected relation",
            )
        result.append(
            RetentionExpectation(
                reference=CognitiveRef(
                    kind=reference.kind,
                    id=reference.id,
                    revision=TypeAdapter(int).validate_python(state["expected_lifecycle_revision"]),
                ),
                strength=TypeAdapter(float).validate_python(state["expected_retention_strength"]),
                anchor_unix_ms=TypeAdapter(int).validate_python(
                    milliseconds(state["expected_retention_anchor_at"])
                ),
                inactive_since_unix_ms=milliseconds(state["expected_inactive_since"]),
                inactive_reason=TypeAdapter(str).validate_python(
                    state["expected_inactive_reason"] or ""
                ),
                inactive=TypeAdapter(bool).validate_python(state["expected_is_inactive"]),
                permanent=TypeAdapter(bool).validate_python(state["expected_is_permanent"]),
                pinned=TypeAdapter(bool).validate_python(state["expected_is_pinned"]),
                protected_until_unix_ms=milliseconds(state["expected_protected_until"]),
            )
        )
    return tuple(result)
