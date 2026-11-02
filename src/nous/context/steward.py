"""Program-owned retrieval, residency and bounded, single-purpose language tasks."""

from __future__ import annotations

from .selection import SelectionRequest

import asyncio
from datetime import UTC, datetime
from hashlib import sha256
from typing import Protocol

from nous_sdk.contracts import (
    CallContext,
    CognitiveRecord,
    DomainError,
    EpistemicStance,
    MediaRecord,
    QueryRequest,
)
from nous_sdk.contracts.common import ModelProvenance
from nous_sdk.contracts.context import (
    ContextConsumerPolicy,
    ContextSegment,
    ContextSession,
    ContextTarget,
    SelectionResult,
    StewardDecision,
    WorkContext,
)
from nous_sdk.contracts.history import HistorySummary
from nous_sdk.contracts.public import ReadRequest
from nous_sdk.contracts.source_graph import ExpandSources
from nous_sdk.contracts.targets import MediaTarget
from pydantic import JsonValue, TypeAdapter

from nous.cognition.records import CognitiveRecords
from nous.platform.models import ModelService
from nous.platform.persistence import Authority, ReadView, WriteTx
from nous.platform.persistence.authority import canonical_json
from nous.references.reading import ReadingService
from nous.retrieval.service import RetrievalService
from nous.retrieval.source_expansion import prefix


class StewardOwner(Protocol):
    authority: Authority
    models: ModelService
    retrieval: RetrievalService
    reading: ReadingService
    cognition: CognitiveRecords

    async def record_segment_in(
        self, view: ReadView | WriteTx, ctx: CallContext, record: CognitiveRecord
    ) -> ContextSegment: ...
    async def media_segment_in(
        self,
        view: ReadView | WriteTx,
        ctx: CallContext,
        target: MediaTarget,
        *,
        required: bool = False,
    ) -> ContextSegment: ...
    async def session_in(
        self, view: ReadView | WriteTx, ctx: CallContext, identity: str
    ) -> ContextSession: ...
    async def work_in(
        self, view: ReadView | WriteTx, ctx: CallContext, identity: str
    ) -> WorkContext: ...


def roots(segment: ContextSegment) -> tuple[str, ...]:
    values = segment.provenance.get("source_roots")
    return (
        tuple(value for value in values if isinstance(value, str))
        if isinstance(values, list)
        else ()
    )


async def source_ids(
    service: StewardOwner, ctx: CallContext, segments: list[ContextSegment]
) -> tuple[str, ...]:
    targets = tuple(
        {
            target.model_dump_json(): target
            for segment in segments
            for target in (*segment.sources, *segment.dependencies)
        }.values()
    )
    observation_ids = tuple({root for segment in segments for root in roots(segment)})
    async with service.authority.read(ctx) as view:
        return await service.reading.directory.material_ids_in(
            view, ctx, targets, known_observation_ids=observation_ids
        )


def passages(segments: list[ContextSegment], text_bytes: int) -> str:
    if not segments:
        return ""
    each = max(1, text_bytes // len(segments))
    return "\n\n".join(
        f"{index}. {prefix(segment.text, each)}" for index, segment in enumerate(segments, 1)
    )


def compress_segments(
    selected: list[ContextSegment], summary: str, producer: ModelProvenance, role: str
) -> ContextSegment:
    sources = tuple(
        {
            target.model_dump_json(): target
            for segment in selected
            for target in (*segment.sources, *segment.dependencies)
        }.values()
    )
    return ContextSegment(
        key="summary:"
        + sha256(canonical_json((sources, summary, producer.output_digest)).encode()).hexdigest(),
        text=summary,
        sources=sources,
        dependencies=sources,
        authority="steward_interpretation",
        epistemic=EpistemicStance(classification="inferred", attribution=role),
        provenance=TypeAdapter(dict[str, JsonValue]).validate_python(
            {
                "source_roots": sorted({root for segment in selected for root in roots(segment)}),
            }
        ),
        producer=producer,
    )


async def supplemental(
    service: StewardOwner, ctx: CallContext, command: SelectionRequest, limit: int
) -> list[ContextSegment]:
    if not command.usage.intent.strip():
        return []
    result = await service.retrieval.query(
        ctx,
        command.query
        or QueryRequest(
            text=command.usage.intent,
            limit=min(8, limit),
            capability_policy="best_effort",
            reranker="off",
        ),
    )
    segments: list[ContextSegment] = []
    async with service.authority.read(ctx) as view:
        for hit in result.hits:
            if isinstance(hit.object, CognitiveRecord):
                segments.append(await service.record_segment_in(view, ctx, hit.object))
            elif isinstance(hit.object, MediaRecord):
                segments.append(
                    await service.media_segment_in(
                        view, ctx, MediaTarget(reference=hit.object.reference)
                    )
                )
    return segments


async def history_inputs(
    service: StewardOwner,
    ctx: CallContext,
    targets: tuple[ContextTarget, ...],
    calls: int,
    text_bytes: int,
) -> tuple[list[ContextSegment], tuple[ContextTarget, ...], int]:
    segments: list[ContextSegment] = []
    covered: list[ContextTarget] = []
    for target in targets:
        pieces: list[str] = []
        cursor = None
        complete = False
        while calls > 0 and text_bytes > 0:
            calls -= 1
            value = await service.reading.read(
                ctx,
                ReadRequest(
                    ref=target.reference.exact(), cursor=cursor, max_chars=min(16000, text_bytes)
                ),
            )
            text = prefix(value.text, text_bytes)
            text_bytes -= len(text.encode("utf-8"))
            pieces.append(text)
            if len(text) != len(value.text):
                break
            if value.next_cursor is None:
                complete = True
                break
            cursor = value.next_cursor
        if complete:
            covered.append(target)
            segments.append(
                ContextSegment(
                    key="working:" + target.reference.exact(),
                    text="".join(pieces),
                    sources=(target,),
                    authority="source_material",
                )
            )
    return segments, tuple(covered), calls


async def residency(
    service: StewardOwner,
    ctx: CallContext,
    command: SelectionRequest,
    policy: ContextConsumerPolicy,
    selected: list[ContextSegment],
) -> tuple[tuple[ContextTarget, ...], tuple[ContextTarget, ...], str]:
    chosen = tuple(
        {
            target.model_dump_json(): target for segment in selected for target in segment.sources
        }.values()
    )
    async with service.authority.read(ctx) as view:
        try:
            session = await service.session_in(view, ctx, command.session_id)
        except DomainError as error:
            if error.code != "session_not_found":
                raise
            return (), chosen[: policy.max_residents], command.usage.intent
        active = [item for item in session.residents if item.state != "evicted"]
        known = {item.target.model_dump_json() for item in active}
        desired = {target.model_dump_json() for target in chosen}
        additions = [target for target in chosen if target.model_dump_json() not in known]
        needed = max(0, len(active) + len(additions) - policy.max_residents)
        now = datetime.now(UTC)
        available = sorted(
            (
                item
                for item in active
                if item.target.model_dump_json() not in desired
                and (item.hold_until is None or item.hold_until <= now)
            ),
            key=lambda item: item.last_meaningful_use_at or item.entered_at,
        )
        evict = tuple(item.target for item in available[:needed])
        promote = tuple(additions[: max(0, policy.max_residents - len(active) + len(evict))])
        text = command.usage.intent
        if session.active_work_context_id:
            work = await service.work_in(view, ctx, session.active_work_context_id)
            text = "\n".join(
                (
                    work.content.purpose,
                    work.content.context_text,
                    *work.content.unresolved_questions,
                )
            )
        return evict, promote, text


async def steward_select(
    service: StewardOwner,
    ctx: CallContext,
    command: SelectionRequest,
    policy: ContextConsumerPolicy,
    offered: list[ContextSegment],
) -> tuple[list[ContextSegment], StewardDecision | None]:
    if policy.steward_role is None and not (command.trigger == "budget" and command.working_groups):
        return offered, None
    maximum = min(command.max_text_bytes, policy.max_text_bytes)
    pressure = (
        len(offered) > min(command.max_items, policy.max_items)
        or sum(len(item.text.encode("utf-8")) for item in offered) > maximum
    )
    if command.trigger == "ordinary" and not pressure:
        return offered, None
    try:
        async with asyncio.timeout(policy.steward_timeout_seconds):
            return await run_tasks(service, ctx, command, policy, offered, maximum, pressure)
    except TimeoutError as error:
        raise DomainError(
            "steward_timeout", "Context maintenance reached its time budget"
        ) from error


async def run_tasks(
    service: StewardOwner,
    ctx: CallContext,
    command: SelectionRequest,
    policy: ContextConsumerPolicy,
    offered: list[ContextSegment],
    maximum: int,
    pressure: bool,
) -> tuple[list[ContextSegment], StewardDecision]:
    evict_initial, promote_initial, work = await residency(service, ctx, command, policy, [])
    goal = command.usage.purpose
    background = work
    async with service.authority.read(ctx) as view:
        try:
            session = await service.session_in(view, ctx, command.session_id)
            if session.active_work_context_id:
                active = await service.work_in(view, ctx, session.active_work_context_id)
                goal = active.content.purpose
                background = "\n".join(
                    (active.content.context_text, *active.content.unresolved_questions)
                )
        except DomainError as error:
            if error.code != "session_not_found":
                raise
    required = [item for item in offered if item.required]
    optional = [item for item in offered if not item.required]
    lookup_budget = policy.steward_max_tool_calls
    if (
        command.trigger in {"task_change", "resume", "conflict", "tool_result"}
        and command.query is None
        and lookup_budget
    ):
        lookup_budget -= 1
        optional.extend(await supplemental(service, ctx, command, policy.max_items))
    optional = list({item.key: item for item in optional}.values())[: min(32, policy.max_items)]
    if command.trigger == "conflict" and optional and lookup_budget:
        lookup_budget -= 1
        graph = (
            await service.retrieval.expand_sources(
                ctx,
                ExpandSources(
                    targets=optional[0].sources,
                    max_depth=2,
                    max_nodes=8,
                    text_bytes=min(8192, maximum),
                ),
            )
            if optional[0].sources
            else None
        )
        if graph is not None:
            for node in graph.nodes:
                if node.status == "available" and node.text:
                    optional.append(
                        ContextSegment(
                            key="source:" + node.key,
                            text=node.text,
                            sources=(node.target,) if node.target else optional[0].sources,
                            authority="source_material",
                        )
                    )
    history: list[ContextSegment] = []
    group_inputs = []
    if command.trigger == "budget":
        for group in command.working_groups:
            segments, covered, lookup_budget = await history_inputs(
                service,
                ctx,
                group.sources,
                lookup_budget,
                max(maximum, command.history_source_bytes),
            )
            if set(item.model_dump_json() for item in covered) == set(
                item.model_dump_json() for item in group.sources
            ):
                group_inputs.append((group, segments))
                history.extend(segments)
    if command.working_sources and lookup_budget:
        recent, _, lookup_budget = await history_inputs(
            service, ctx, command.working_sources, lookup_budget, min(maximum, 16000)
        )
        background += "\n\n" + "\n\n".join(item.text for item in recent)
    remaining = policy.steward_max_model_requests
    producers: list[ModelProvenance] = []
    question = command.usage.intent or command.usage.purpose
    reserved = len(group_inputs) + int(pressure) + int(command.trigger in {"pause", "resume"})
    selected = optional
    selection_note: str | None = None
    if optional and policy.steward_role is not None and remaining > reserved:
        generated = await service.models.generate(
            ctx,
            policy.steward_role,
            {
                "goal": goal,
                "background": background,
                "question": question,
                "passages": passages(optional, maximum),
                "requirements": "",
            },
            SelectionResult,
            source_ids=await source_ids(service, ctx, optional),
        )
        remaining -= 1
        producers.append(generated.producer)
        indices = generated.output.paragraphs
        selection_note = generated.output.note
        if any(index < 1 or index > len(optional) for index in indices):
            raise DomainError(
                "steward_invalid_selection", "Selection contains an unavailable paragraph number"
            )
        selected = [optional[index - 1] for index in dict.fromkeys(indices)]
    group_summaries: list[HistorySummary] = []
    for group, segments in group_inputs:
        if not remaining:
            break
        parts = [
            part.strip() for item in segments for part in item.text.split("\n\n") if part.strip()
        ]
        topics = [list(range(len(parts)))]
        ids = await source_ids(service, ctx, segments)
        if group.label is None and len(parts) > 1:
            if remaining < 2:
                continue
            grouped = await service.models.generate(
                ctx,
                "history_topics",
                {
                    "question": group.purpose,
                    "passages": "\n\n".join(f"{i + 1}. {text}" for i, text in enumerate(parts)),
                },
                list[list[int]],
                source_ids=ids,
            )
            remaining -= 1
            producers.append(grouped.producer)
            flattened = [index for topic in grouped.output for index in topic]
            if sorted(flattened) != list(range(1, len(parts) + 1)) or any(
                not topic for topic in grouped.output
            ):
                continue
            topics = [[index - 1 for index in topic] for topic in grouped.output]
        if remaining < len(topics) or group.token_budget < len(topics) * 64:
            continue
        tokens = group.token_budget // len(topics)
        approximate = (
            "约两三句话" if tokens < 256 else "约200到300字" if tokens < 700 else "用数段话概括"
        )
        instruction = "\n".join(
            (
                "为继续以下工作概括这个话题的已结束对话。",
                "用途：" + group.purpose,
                "当前工作：" + goal,
                "背景与待办：" + background,
                "接下来要继续：" + group.continuation,
                "本组主题：" + (group.label or "当前这一组对话所谈事项"),
                "篇幅：" + approximate,
            )
        )
        summaries = []
        for index, topic in enumerate(topics):
            generated = await service.models.generate(
                ctx,
                "context_compress",
                {"question": instruction, "passages": "\n\n".join(parts[part] for part in topic)},
                str,
                source_ids=ids,
                max_output_tokens=tokens,
            )
            remaining -= 1
            producers.append(generated.producer)
            summaries.append(generated.output)
        summary = "\n\n".join(summaries)
        measurement = await service.models.measure_native(
            ctx,
            "context_compress",
            {"messages": [{"role": "user", "content": summary}], "max_output_tokens": 1},
        )
        if measurement.input_tokens > group.token_budget:
            continue
        group_summaries.append(
            HistorySummary(
                group_id=group.group_id,
                text=summary,
                covered_sources=group.sources,
                producer=producers[-1],
            )
        )
    available_bytes = max(0, maximum - sum(len(item.text.encode("utf-8")) for item in required))
    if (
        selected
        and remaining
        and (
            sum(len(item.text.encode("utf-8")) for item in selected) > available_bytes
            or len(selected) + len(required) > min(command.max_items, policy.max_items)
        )
    ):
        generated = await service.models.generate(
            ctx,
            "context_compress",
            {
                "question": "用一小段话整理与这个问题有关的资料：\n" + question,
                "passages": passages(selected, maximum),
            },
            str,
            source_ids=await source_ids(service, ctx, selected),
        )
        remaining -= 1
        producers.append(generated.producer)
        summary = generated.output
        if len(summary.encode("utf-8")) > available_bytes and remaining:
            generated = await service.models.generate(
                ctx,
                "context_compress",
                {"question": "再概括成更短的一小段话。", "passages": summary},
                str,
                source_ids=await source_ids(service, ctx, selected),
            )
            remaining -= 1
            producers.append(generated.producer)
            summary = generated.output
        if len(summary.encode("utf-8")) <= available_bytes:
            selected = [
                compress_segments(selected, summary, generated.producer, "context_compress")
            ]
    evict, promote, work = await residency(service, ctx, command, policy, selected)
    work_summary = None
    unresolved: tuple[str, ...] = ()
    if command.trigger in {"pause", "resume"} and work and remaining:
        generated = await service.models.generate(
            ctx,
            "work_summary",
            {"question": question, "passages": work},
            str,
            source_ids=await source_ids(service, ctx, [*required, *selected]),
        )
        remaining -= 1
        producers.append(generated.producer)
        work_summary = generated.output
    if command.trigger in {"pause", "resume", "task_change"} and work and remaining:
        generated = await service.models.generate(
            ctx,
            "work_questions",
            {"question": question, "passages": work},
            list[str],
            source_ids=await source_ids(service, ctx, [*required, *selected]),
        )
        remaining -= 1
        producers.append(generated.producer)
        unresolved = tuple(generated.output[:32])
    bounded = list(required)
    used = sum(len(item.text.encode("utf-8")) for item in bounded)
    for item in selected:
        size = len(item.text.encode("utf-8"))
        if len(bounded) < min(command.max_items, policy.max_items) and used + size <= maximum:
            bounded.append(item)
            used += size
    sources = tuple(
        {
            target.model_dump_json(): target
            for item in [*bounded, *history]
            for target in (*item.sources, *item.dependencies)
        }.values()
    )
    decision = StewardDecision(
        selection_note=selection_note,
        evict=evict,
        promote=promote,
        work_summary=work_summary,
        group_summaries=tuple(group_summaries),
        unresolved=unresolved,
        sources=sources,
        producers=tuple(producers),
    )
    return bounded, decision
